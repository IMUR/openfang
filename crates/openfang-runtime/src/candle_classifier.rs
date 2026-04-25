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
use candle_transformers::models::distilbert::{DistilBertModel, Config as DistilBertConfig};
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
            let is_distilbert = config_str.contains("\"dim\"") && config_str.contains("\"n_layers\"");

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

            info!(model_id = model_id_owned, "NLI classifier model loaded");

            Ok::<_, ClassifierError>(NliModel {
                model: model_type,
                classifier,
                tokenizer,
                device: device_clone,
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

    // Forward pass through the encoder
    let hidden = match &nli.model {
        NliModelType::Bert(m) => m
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))
            .map_err(|e| ClassifierError::Inference(format!("BERT forward: {e}")))?,
        NliModelType::DistilBert(m) => m
            .forward(&input_ids, &attention_mask)
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

    // Entailment class index = 2 (CONTRADICTION=0, NEUTRAL=1, ENTAILMENT=2)
    // This is the standard label order for MNLI-fine-tuned models.
    let entailment_probs = probs
        .narrow(1, 2, 1)
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
