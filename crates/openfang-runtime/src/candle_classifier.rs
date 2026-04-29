//! Candle-backed zero-shot memory classification driver.
//!
//! Uses an NLI (Natural Language Inference) model to classify agent memories
//! into scope and category buckets without requiring fine-tuning on task-specific
//! data.  Each candidate label is tested as an entailment hypothesis; the label
//! with the highest entailment score wins.
//!
//! ## Zero-shot NLI approach
//!
//! Input format: `[CLS] text [SEP] hypothesis [SEP]`
//! Model output: 3-class logits → `[contradiction, neutral, entailment]`
//! Classification: softmax over logits, entailment score (index 2) = confidence.
//!
//! ## Default model
//!
//! `typeform/distilbert-base-uncased-mnli` — 67M params, ~135 MB FP32.
//! Loaded as a BERT sequence-classification model (architecture-compatible).
//! The model's `id2label` is `{0: CONTRADICTION, 1: NEUTRAL, 2: ENTAILMENT}`.
//!
//! ## Scope hypotheses
//!
//! | Scope       | Hypothesis                                                  |
//! |-------------|-------------------------------------------------------------|
//! | episodic    | This text describes a specific conversation or event        |
//! | semantic    | This text contains general knowledge or a summary           |
//! | procedural  | This text describes how to do something or a process        |
//! | declarative | This text states a personal preference or explicit fact     |
//!
//! ## Category hypotheses
//!
//! | Category    | Hypothesis                                    |
//! |-------------|-----------------------------------------------|
//! | fact        | This text states a factual claim              |
//! | preference  | This text expresses a preference or opinion   |
//! | instruction | This text contains an instruction or command  |
//! | observation | This text describes an observation or outcome |
//! | question    | This text asks a question                     |

use crate::model_cache;
use candle_core::{DType, Device, Tensor};
use candle_nn::{linear, Linear, Module, VarBuilder};
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use candle_transformers::models::distilbert::{Config as DistilBertConfig, DistilBertModel};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};
use tracing::{debug, info};

/// Scope hypotheses for zero-shot NLI classification (label, hypothesis).
const SCOPE_HYPOTHESES: &[(&str, &str)] = &[
    (
        "episodic",
        "This text describes a specific conversation or event.",
    ),
    (
        "semantic",
        "This text contains general knowledge or a distilled summary.",
    ),
    (
        "procedural",
        "This text describes how to do something or a step-by-step process.",
    ),
    (
        "declarative",
        "This text states a personal preference, explicit fact, or direct user instruction.",
    ),
];

/// Category hypotheses for zero-shot NLI classification (label, hypothesis).
const CATEGORY_HYPOTHESES: &[(&str, &str)] = &[
    ("fact", "This text states a factual claim about the world."),
    (
        "preference",
        "This text expresses a personal preference, taste, or opinion.",
    ),
    (
        "instruction",
        "This text contains a command or explicit instruction to follow.",
    ),
    (
        "observation",
        "This text records an observed outcome or piece of evidence.",
    ),
    (
        "question",
        "This text poses a question seeking information.",
    ),
];

/// Result of memory classification.
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    /// Classified memory scope (e.g. `"episodic"`, `"declarative"`).
    pub scope: String,
    /// Classified memory category (e.g. `"fact"`, `"instruction"`).
    pub category: String,
    /// Confidence of the scope classification (entailment score, 0.0–1.0).
    pub scope_confidence: f32,
    /// Confidence of the category classification.
    pub category_confidence: f32,
}

/// Error type for classification operations.
#[derive(Debug, thiserror::Error)]
pub enum ClassifierError {
    #[error("Model load: {0}")]
    Load(String),
    #[error("Inference: {0}")]
    Inference(String),
    #[error("Model cache: {0}")]
    Cache(String),
}

enum NliModelType {
    Bert(BertModel),
    DistilBert(DistilBertModel),
}

/// Inner NLI classifier state.
struct NliModel {
    model: NliModelType,
    /// Linear(hidden_size → 3) for NLI: contradiction / neutral / entailment
    classifier: Linear,
    tokenizer: Tokenizer,
    device: Device,
    /// Index of the ENTAILMENT class in the model's label ordering.
    /// Read from config.json `id2label`; falls back to 2 (standard MNLI order).
    entailment_idx: usize,
}

/// Candle zero-shot memory classification driver.
///
/// Thread-safe via `Arc<Mutex<NliModel>>`. Classification is called per-memory
/// write; sequential access is acceptable at that rate.
pub struct CandleClassifier {
    inner: Arc<Mutex<NliModel>>,
}

impl CandleClassifier {
    /// Load the NLI model from cache or HF Hub.
    ///
    /// Default model: `typeform/distilbert-base-uncased-mnli`
    /// (67M params, ~135 MB FP32 / ~68 MB FP16 on CUDA).
    pub async fn load(
        model_id: &str,
        home_dir: &Path,
        cuda_device: Option<u32>,
    ) -> Result<Self, ClassifierError> {
        let model_id_owned = model_id.to_string();
        let home_dir = home_dir.to_path_buf();

        let files = model_cache::resolve_model(&model_id_owned, &home_dir, None)
            .await
            .map_err(|e| ClassifierError::Cache(e.to_string()))?;

        info!(
            model_id,
            device = cuda_device
                .map(|d| format!("cuda:{d}"))
                .as_deref()
                .unwrap_or("cpu"),
            "Loading NLI classification model"
        );

        let device = match cuda_device {
            Some(idx) => Device::new_cuda(idx as usize)
                .map_err(|e| ClassifierError::Load(format!("CUDA device {idx}: {e}")))?,
            None => Device::Cpu,
        };

        let device_clone = device.clone();
        let weights_path = files.weights;
        let config_path = files.config;
        let tokenizer_path = files.tokenizer;

        let nli = tokio::task::spawn_blocking(move || {
            let config_str = std::fs::read_to_string(&config_path)
                .map_err(|e| ClassifierError::Load(format!("config.json: {e}")))?;
            let is_distilbert =
                config_str.contains("\"dim\"") && config_str.contains("\"n_layers\"");

            let dtype = if matches!(device_clone, Device::Cuda(_)) {
                DType::F16
            } else {
                DType::F32
            };

            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[&weights_path], dtype, &device_clone)
                    .map_err(|e| ClassifierError::Load(format!("weights load: {e}")))?
            };

            // Re-load models with correct varbuilder
            let (model_type, hidden_size) = if is_distilbert {
                let db_config: DistilBertConfig = serde_json::from_str(&config_str)
                    .map_err(|e| ClassifierError::Load(format!("distilbert config: {e}")))?;
                let m = DistilBertModel::load(vb.pp("distilbert"), &db_config)
                    .or_else(|_| DistilBertModel::load(vb.clone(), &db_config))
                    .map_err(|e| ClassifierError::Load(format!("DistilBertModel: {e}")))?;
                (NliModelType::DistilBert(m), db_config.dim)
            } else {
                let bert_config: BertConfig = serde_json::from_str(&config_str)
                    .map_err(|e| ClassifierError::Load(format!("bert config: {e}")))?;
                let m = BertModel::load(vb.pp("bert"), &bert_config)
                    .or_else(|_| BertModel::load(vb.clone(), &bert_config))
                    .map_err(|e| ClassifierError::Load(format!("BertModel: {e}")))?;
                (NliModelType::Bert(m), bert_config.hidden_size)
            };

            // NLI classification head: hidden_size → 3
            let classifier = linear(hidden_size, 3, vb.pp("classifier"))
                .map_err(|e| ClassifierError::Load(format!("classifier head: {e}")))?;

            let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| ClassifierError::Load(format!("tokenizer: {e}")))?;

            tokenizer
                .with_truncation(Some(TruncationParams {
                    max_length: 512,
                    ..Default::default()
                }))
                .map_err(|e| ClassifierError::Load(format!("truncation: {e}")))?;

            tokenizer.with_padding(Some(PaddingParams {
                strategy: tokenizers::PaddingStrategy::BatchLongest,
                ..Default::default()
            }));

            // Resolve entailment index from model config's id2label.
            let id2label: HashMap<String, String> = serde_json::from_str(&config_str)
                .map(|v: serde_json::Value| {
                    v.get("id2label")
                        .and_then(|m| m.as_object())
                        .map(|obj| {
                            obj.iter()
                                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                                .collect::<HashMap<String, String>>()
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let entailment_idx = id2label
                .iter()
                .find(|(_, v)| v.to_uppercase() == "ENTAILMENT")
                .and_then(|(k, _)| k.parse::<usize>().ok())
                .unwrap_or(2);
            info!(
                entailment_idx,
                "Resolved entailment label index from config"
            );

            info!(model_id = model_id_owned, "NLI classifier model loaded");

            Ok::<_, ClassifierError>(NliModel {
                model: model_type,
                classifier,
                tokenizer,
                device: device_clone,
                entailment_idx,
            })
        })
        .await
        .map_err(|e| ClassifierError::Load(format!("Spawn: {e}")))?
        .map_err(|e: ClassifierError| e)?;

        Ok(Self {
            inner: Arc::new(Mutex::new(nli)),
        })
    }

    /// Classify `text` into a scope and category using zero-shot NLI entailment.
    ///
    /// Returns `None` if the text is too short to classify meaningfully (< 10 chars).
    /// Each candidate label is scored via an NLI forward pass; the best-scoring label
    /// wins. Scope and category are scored independently.
    pub async fn classify(&self, text: &str) -> Result<ClassificationResult, ClassifierError> {
        if text.trim().len() < 10 {
            return Ok(ClassificationResult {
                scope: "episodic".to_string(),
                category: "observation".to_string(),
                scope_confidence: 0.0,
                category_confidence: 0.0,
            });
        }

        let inner = self.inner.clone();
        let text_owned = text.to_string();

        tokio::task::spawn_blocking(move || {
            let nli = inner
                .lock()
                .map_err(|e| ClassifierError::Inference(format!("Mutex: {e}")))?;

            // Score all scope hypotheses
            let scope_scores = nli_score_all(&nli, &text_owned, SCOPE_HYPOTHESES)?;
            let (best_scope_idx, &best_scope_conf) = scope_scores
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .ok_or_else(|| ClassifierError::Inference("Empty scope scores".into()))?;

            // Score all category hypotheses
            let cat_scores = nli_score_all(&nli, &text_owned, CATEGORY_HYPOTHESES)?;
            let (best_cat_idx, &best_cat_conf) = cat_scores
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .ok_or_else(|| ClassifierError::Inference("Empty category scores".into()))?;

            debug!(
                scope = SCOPE_HYPOTHESES[best_scope_idx].0,
                scope_conf = best_scope_conf,
                category = CATEGORY_HYPOTHESES[best_cat_idx].0,
                cat_conf = best_cat_conf,
                "NLI classification complete"
            );

            Ok(ClassificationResult {
                scope: SCOPE_HYPOTHESES[best_scope_idx].0.to_string(),
                category: CATEGORY_HYPOTHESES[best_cat_idx].0.to_string(),
                scope_confidence: best_scope_conf,
                category_confidence: best_cat_conf,
            })
        })
        .await
        .map_err(|e| ClassifierError::Inference(format!("Task join: {e}")))?
        .map_err(|e| e)
    }
}

/// Score each hypothesis against the premise text using NLI entailment.
///
/// Returns a vector of entailment probabilities (one per hypothesis), in the
/// same order as `hypotheses`.
fn nli_score_all(
    nli: &NliModel,
    premise: &str,
    hypotheses: &[(&str, &str)],
) -> Result<Vec<f32>, ClassifierError> {
    let pairs: Vec<(&str, &str)> = hypotheses.iter().map(|(_, h)| (premise, *h)).collect();

    let encodings = nli
        .tokenizer
        .encode_batch_char_offsets(
            pairs.iter().map(|(p, h)| (*p, *h)).collect::<Vec<_>>(),
            true,
        )
        .map_err(|e| ClassifierError::Inference(format!("tokenize: {e}")))?;

    let n = encodings.len();
    let max_len = encodings
        .iter()
        .map(|e| e.get_ids().len())
        .max()
        .unwrap_or(0);

    if max_len == 0 {
        return Ok(vec![0.0f32; n]);
    }

    let device = &nli.device;

    let mut all_ids: Vec<u32> = Vec::with_capacity(n * max_len);
    let mut all_type_ids: Vec<u32> = Vec::with_capacity(n * max_len);
    let mut all_masks: Vec<u32> = Vec::with_capacity(n * max_len);

    for enc in &encodings {
        let ids = enc.get_ids();
        let type_ids = enc.get_type_ids();
        let mask = enc.get_attention_mask();

        all_ids.extend_from_slice(ids);
        all_type_ids.extend_from_slice(type_ids);
        all_masks.extend_from_slice(mask);

        // Pad to max_len
        let pad = max_len - ids.len();
        all_ids.extend(std::iter::repeat(0u32).take(pad));
        all_type_ids.extend(std::iter::repeat(0u32).take(pad));
        all_masks.extend(std::iter::repeat(0u32).take(pad));
    }

    let input_ids = Tensor::from_vec(all_ids, (n, max_len), device)
        .map_err(|e| ClassifierError::Inference(format!("input_ids tensor: {e}")))?;
    let token_type_ids = Tensor::from_vec(all_type_ids, (n, max_len), device)
        .map_err(|e| ClassifierError::Inference(format!("token_type_ids tensor: {e}")))?;
    let attention_mask = Tensor::from_vec(all_masks, (n, max_len), device)
        .map_err(|e| ClassifierError::Inference(format!("attention_mask tensor: {e}")))?;

    // DistilBERT attention mask preparation:
    // Tokenizer gives [batch, seq_len] U32: 1=real, 0=padding.
    // candle's masked_fill treats non-zero as "fill with -inf".
    // So we need padding=1, real=0. We invert by subtracting from ones
    // (staying in U32 to avoid the F32 where_cond error in candle-transformers
    // 0.10.2). Reshape to 4D (batch, 1, 1, seq_len) because DistilBertModel
    // does NOT auto-expand a 2D mask — it tries to broadcast directly to
    // [batch, heads, q_len, k_len] and fails.
    let attention_mask_for_distilbert = if matches!(&nli.model, NliModelType::DistilBert(_)) {
        let ones = Tensor::ones_like(&attention_mask)
            .map_err(|e| ClassifierError::Inference(format!("ones: {e}")))?;
        let inverted = ones
            .broadcast_sub(&attention_mask)
            .map_err(|e| ClassifierError::Inference(format!("mask sub: {e}")))?;
        inverted
            .reshape((n, 1, 1, max_len))
            .map_err(|e| ClassifierError::Inference(format!("mask reshape 4D: {e}")))?
    } else {
        attention_mask.clone()
    };

    // Forward pass through the encoder
    let hidden = match &nli.model {
        NliModelType::Bert(m) => m
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))
            .map_err(|e| ClassifierError::Inference(format!("BERT forward: {e}")))?,
        NliModelType::DistilBert(m) => m
            .forward(&input_ids, &attention_mask_for_distilbert)
            .map_err(|e| ClassifierError::Inference(format!("DistilBERT forward: {e}")))?,
    };

    // Extract [CLS] token (index 0) for each item in the batch
    let cls = hidden
        .narrow(1, 0, 1)
        .map_err(|e| ClassifierError::Inference(format!("CLS narrow: {e}")))?
        .squeeze(1)
        .map_err(|e| ClassifierError::Inference(format!("CLS squeeze: {e}")))?;

    // Classification head → logits shape: (n, 3)
    let logits = nli
        .classifier
        .forward(&cls)
        .map_err(|e| ClassifierError::Inference(format!("classifier head: {e}")))?;

    // Softmax over the 3 NLI classes
    let probs = candle_nn::ops::softmax(&logits, 1)
        .map_err(|e| ClassifierError::Inference(format!("softmax: {e}")))?;

    // Use the entailment index resolved from the model's id2label at load time.
    let entailment_probs = probs
        .narrow(1, nli.entailment_idx, 1)
        .map_err(|e| ClassifierError::Inference(format!("entailment narrow: {e}")))?
        .squeeze(1)
        .map_err(|e| ClassifierError::Inference(format!("entailment squeeze: {e}")))?;

    // Convert to f32 Vec
    let probs_f32 = entailment_probs
        .to_dtype(DType::F32)
        .map_err(|e| ClassifierError::Inference(format!("dtype cast: {e}")))?
        .to_vec1::<f32>()
        .map_err(|e| ClassifierError::Inference(format!("to_vec1: {e}")))?;

    Ok(probs_f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::Device;

    /// Contract test: DistilBERT mask preparation must produce shape (n, 1, 1, max_len) in U32.
    ///
    /// This tests the mask preparation boundary that was broken in production:
    /// - Tokenizer outputs [batch, seq_len] U32 mask (1=real, 0=padding)
    /// - We invert it (padding=1, real=0) staying in U32
    /// - Then reshape to (batch, 1, 1, seq_len) for DistilBertModel::forward()
    ///
    /// The reshape is needed because candle's DistilBertModel does NOT expand a 2D
    /// mask internally (unlike BertModel which uses get_extended_attention_mask).
    /// Without the reshape, DistilBertModel tries to broadcast [batch, seq_len] to
    /// [batch, heads, q_len, k_len] and fails.
    #[test]
    fn distilbert_mask_shape_contract() {
        let device = Device::Cpu;
        let n = 4;
        let max_len = 54;

        // Simulate tokenizer output: U32 [batch, seq_len], 1=real, 0=padding
        // Batch of 4: two sequences of length 54, two with padding
        let mask_values: Vec<u32> = (0..n * max_len)
            .map(|i| {
                let seq_idx = i / max_len;
                let pos = i % max_len;
                if seq_idx < 2 {
                    1 // first two sequences: all real tokens
                } else if pos < 20 {
                    1 // last two: 20 real tokens, rest padding
                } else {
                    0
                }
            })
            .collect();

        let attention_mask = Tensor::from_vec(mask_values, (n, max_len), &device)
            .expect("failed to create attention mask tensor");

        // Assert preconditions from tokenizer
        assert_eq!(attention_mask.shape().dims(), &[n, max_len]);
        assert_eq!(attention_mask.dtype(), DType::U32);

        // --- Replicate the DistilBERT mask preparation from nli_score_all ---
        let ones = Tensor::ones_like(&attention_mask).expect("ones");
        let inverted = ones.broadcast_sub(&attention_mask).expect("mask inversion");
        let reshaped = inverted.reshape((n, 1, 1, max_len)).expect("4D reshape");

        // Contract assertions
        assert_eq!(
            reshaped.shape().dims(),
            &[n, 1, 1, max_len],
            "DistilBERT mask must be 4D: (batch, 1, 1, seq_len)"
        );
        assert_eq!(
            reshaped.dtype(),
            DType::U32,
            "Mask dtype must remain U32 (avoids F32 where_cond bug in candle 0.10.2)"
        );

        // Verify inversion correctness: padding positions should be 1, real tokens 0
        let flat = reshaped
            .flatten_all()
            .expect("flatten")
            .to_vec1::<u32>()
            .expect("to_vec1");

        // First two sequences (all real): every position should be 0 after inversion
        for i in 0..(2 * max_len) {
            assert_eq!(
                flat[i], 0u32,
                "Real token at flat index {} should be 0 after inversion",
                i
            );
        }

        // Third sequence: first 20 real → 0, rest padding → 1
        for pos in 0..max_len {
            let idx = 2 * max_len + pos;
            let expected = if pos < 20 { 0u32 } else { 1u32 };
            assert_eq!(
                flat[idx], expected,
                "Seq 2 pos {}: expected {} after inversion",
                pos, expected
            );
        }
    }

    /// Verify that the 4D mask shape is compatible with the attention score dimensions.
    /// The mask [n, 1, 1, max_len] must broadcast against [n, heads, q_len, k_len].
    /// We test this by expanding the mask to the full attention shape.
    #[test]
    fn distilbert_mask_broadcasts_to_attention_shape() {
        let device = Device::Cpu;
        let n = 4;
        let max_len = 54;
        let num_heads = 12;

        let mask_values: Vec<u32> = vec![1u32; n * max_len];
        let attention_mask = Tensor::from_vec(mask_values, (n, max_len), &device).unwrap();

        let ones = Tensor::ones_like(&attention_mask).unwrap();
        let inverted = ones.broadcast_sub(&attention_mask).unwrap();
        let reshaped = inverted.reshape((n, 1, 1, max_len)).unwrap();

        // Verify the 4D shape is correct for broadcasting to [batch, heads, q_len, k_len]
        assert_eq!(reshaped.shape().dims(), &[n, 1, 1, max_len]);
        assert_eq!(reshaped.dtype(), DType::U32);

        // Expand the mask to the full attention shape to confirm broadcast compatibility.
        // This is what candle's DistilBertModel attention layer needs to do internally.
        let attention_shape = (n, num_heads, max_len, max_len);
        let expanded = reshaped
            .expand(attention_shape)
            .expect("4D mask must be expandable to [n, heads, q_len, k_len]");
        assert_eq!(expanded.shape().dims(), &[n, num_heads, max_len, max_len]);
    }
}
