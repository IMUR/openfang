//! Candle-backed NER (Named Entity Recognition) driver.
//!
//! Runs `dslim/bert-base-NER` on CUDA to extract named entities from
//! agent memory content. Extracted entities are written to the knowledge
//! graph on every `remember()` call, populating the currently-empty graph.
//!
//! ## BIO tagging scheme
//!
//! dslim/bert-base-NER uses the standard CoNLL BIO scheme:
//! - `B-PER` / `I-PER` — Person
//! - `B-ORG` / `I-ORG` — Organization
//! - `B-LOC` / `I-LOC` — Location
//! - `B-MISC` / `I-MISC` — Miscellaneous (mapped to Concept)
//! - `O` — Outside (not an entity)
//!
//! ## Model
//!
//! `dslim/bert-base-NER`: 110M BERT-base with a token classification head.
//! Loaded at FP16 on CUDA. VRAM: ~270MB.

use crate::model_cache;
use candle_core::{DType, Device, Tensor};
use candle_nn::{linear, Linear, Module, VarBuilder};
use candle_transformers::models::bert::{BertModel, Config};
use openfang_types::memory::EntityType;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokenizers::Tokenizer;
use tracing::{debug, info};

/// An entity extracted by NER.
#[derive(Debug, Clone)]
pub struct ExtractedEntity {
    /// Surface form text of the entity.
    pub text: String,
    /// Canonical entity type.
    pub entity_type: EntityType,
    /// Confidence score (0.0 – 1.0).
    pub confidence: f32,
}

/// Error type for NER operations.
#[derive(Debug, thiserror::Error)]
pub enum NerError {
    #[error("Model load: {0}")]
    Load(String),
    #[error("Inference: {0}")]
    Inference(String),
    #[error("Model cache: {0}")]
    Cache(String),
}

/// Inner state for the NER driver.
struct BertNer {
    model: BertModel,
    classifier: Linear,
    tokenizer: Tokenizer,
    device: Device,
    /// Label map: index → BIO tag string (e.g. `"B-PER"`, `"O"`)
    id2label: Vec<String>,
}

/// Candle NER driver: extracts named entities in-process on CUDA.
///
/// Thread-safe via `Arc<Mutex<BertNer>>`. Only one NER inference runs at a
/// time (sequential calls), which is fine since NER is called per-memory write,
/// not per-query.
pub struct CandleNerDriver {
    inner: Arc<Mutex<BertNer>>,
}

impl CandleNerDriver {
    /// Load the NER model from cache or HF Hub.
    ///
    /// Model: `dslim/bert-base-NER` (standard CoNLL NER, 110M params, ~270MB FP16 VRAM)
    pub async fn load(
        model_id: &str,
        home_dir: &Path,
        cuda_device: Option<u32>,
    ) -> Result<Self, NerError> {
        let model_id = model_id.to_string();
        let home_dir = home_dir.to_path_buf();

        let files = model_cache::resolve_model(&model_id, &home_dir, None)
            .await
            .map_err(|e| NerError::Cache(e.to_string()))?;

        info!(
            model_id,
            device = cuda_device.map(|d| format!("cuda:{d}")).as_deref().unwrap_or("cpu"),
            "Loading BERT-NER model"
        );

        let device = match cuda_device {
            Some(idx) => Device::new_cuda(idx as usize)
                .map_err(|e| NerError::Load(format!("CUDA device {idx}: {e}")))?,
            None => Device::Cpu,
        };

        let device_clone = device.clone();
        let weights_path = files.weights;
        let config_path = files.config;
        let tokenizer_path = files.tokenizer;

        let ner = tokio::task::spawn_blocking(move || {
            // Parse config — need both the model config and the label map
            let config_str = std::fs::read_to_string(&config_path)
                .map_err(|e| NerError::Load(format!("config.json: {e}")))?;

            let config_json: serde_json::Value = serde_json::from_str(&config_str)
                .map_err(|e| NerError::Load(format!("config parse: {e}")))?;

            let bert_config: Config = serde_json::from_value(config_json.clone())
                .map_err(|e| NerError::Load(format!("bert config: {e}")))?;

            // Extract id2label map from config
            let id2label = extract_id2label(&config_json);
            let num_labels = id2label.len().max(9); // CoNLL-2003 has 9 labels

            let dtype = if matches!(device_clone, Device::Cuda(_)) {
                DType::F16
            } else {
                DType::F32
            };

            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[&weights_path], dtype, &device_clone)
                    .map_err(|e| NerError::Load(format!("weights load: {e}")))?
            };

            let model = BertModel::load(vb.clone(), &bert_config)
                .map_err(|e| NerError::Load(format!("BertModel: {e}")))?;

            // Token classification head: Linear(hidden_size, num_labels)
            let classifier = linear(bert_config.hidden_size, num_labels, vb.pp("classifier"))
                .map_err(|e| NerError::Load(format!("classifier head: {e}")))?;

            let tokenizer = Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| NerError::Load(format!("tokenizer: {e}")))?;

            info!(model_id, labels = num_labels, "BERT-NER model loaded");

            Ok::<_, NerError>(BertNer {
                model,
                classifier,
                tokenizer,
                device: device_clone,
                id2label,
            })
        })
        .await
        .map_err(|e| NerError::Load(format!("Spawn error: {e}")))?
        .map_err(|e: NerError| e)?;

        Ok(Self {
            inner: Arc::new(Mutex::new(ner)),
        })
    }

    /// Extract named entities from `text`.
    ///
    /// Returns entities with confidence ≥ `min_confidence` (default 0.7).
    /// Short texts (< 3 chars) return empty immediately.
    pub async fn extract_entities(
        &self,
        text: &str,
        min_confidence: f32,
    ) -> Result<Vec<ExtractedEntity>, NerError> {
        if text.trim().len() < 3 {
            return Ok(vec![]);
        }

        let inner = self.inner.clone();
        let text_owned = text.to_string();

        tokio::task::spawn_blocking(move || {
            let ner = inner
                .lock()
                .map_err(|e| NerError::Inference(format!("Mutex: {e}")))?;
            run_ner(&ner, &text_owned, min_confidence)
        })
        .await
        .map_err(|e| NerError::Inference(format!("Task join: {e}")))?
    }
}

/// Run NER inference and decode BIO tags to entity spans.
fn run_ner(
    ner: &BertNer,
    text: &str,
    min_confidence: f32,
) -> Result<Vec<ExtractedEntity>, NerError> {
    let encoding = ner
        .tokenizer
        .encode(text, true)
        .map_err(|e| NerError::Inference(format!("tokenize: {e}")))?;

    let ids = encoding.get_ids();
    let offsets = encoding.get_offsets();
    let n = ids.len();

    if n == 0 {
        return Ok(vec![]);
    }

    let device = &ner.device;

    let input_ids = Tensor::from_vec(ids.to_vec(), (1, n), device)
        .map_err(|e| NerError::Inference(format!("input_ids: {e}")))?;
    let token_type_ids = input_ids
        .zeros_like()
        .map_err(|e| NerError::Inference(format!("type_ids: {e}")))?;

    // Forward pass → [1, seq_len, hidden_size]
    let sequence_output = ner
        .model
        .forward(&input_ids, &token_type_ids, None)
        .map_err(|e| NerError::Inference(format!("BERT forward: {e}")))?;

    // Token classification → [1, seq_len, num_labels]
    let logits = ner
        .classifier
        .forward(&sequence_output)
        .map_err(|e| NerError::Inference(format!("classifier forward: {e}")))?;

    // Softmax to get per-token label probabilities → [1, seq_len, num_labels]
    let probs = softmax_last_dim(&logits)
        .map_err(|e| NerError::Inference(format!("softmax: {e}")))?;

    // Extract to CPU f32 [seq_len, num_labels]
    let probs_f32 = probs
        .squeeze(0)
        .and_then(|t| t.to_dtype(DType::F32))
        .map_err(|e| NerError::Inference(format!("squeeze/cast: {e}")))?;

    let probs_data = probs_f32
        .to_vec2::<f32>()
        .map_err(|e| NerError::Inference(format!("to_vec2: {e}")))?;

    debug!(seq_len = probs_data.len(), "NER inference complete");

    // Decode BIO tags to entity spans
    decode_bio_tags(&probs_data, &ner.id2label, offsets, text, min_confidence)
}

/// Decode per-token BIO probabilities to entity spans.
fn decode_bio_tags(
    probs: &[Vec<f32>],
    id2label: &[String],
    offsets: &[(usize, usize)],
    text: &str,
    min_confidence: f32,
) -> Result<Vec<ExtractedEntity>, NerError> {
    let mut entities: Vec<ExtractedEntity> = Vec::new();

    // Track current entity span
    let mut current_entity: Option<(String, EntityType, f32, usize)> = None; // (text, type, conf, token_count)

    for (i, token_probs) in probs.iter().enumerate() {
        // Find argmax label
        let (label_idx, &confidence) = token_probs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap_or((0, &0.0f32));

        let label = id2label.get(label_idx).map(|s| s.as_str()).unwrap_or("O");

        // Get the text span for this token
        let (start, end) = offsets.get(i).copied().unwrap_or((0, 0));
        let token_text = if start < end && end <= text.len() {
            &text[start..end]
        } else {
            ""
        };

        if label.starts_with("B-") {
            // Flush any current entity
            if let Some((entity_text, entity_type, conf, _)) = current_entity.take() {
                if conf >= min_confidence && !entity_text.trim().is_empty() {
                    entities.push(ExtractedEntity {
                        text: entity_text.trim().to_string(),
                        entity_type,
                        confidence: conf,
                    });
                }
            }
            // Start new entity
            let etype = label_suffix_to_entity_type(&label[2..]);
            current_entity = Some((token_text.to_string(), etype, confidence, 1));
        } else if label.starts_with("I-") {
            if let Some((ref mut entity_text, _, ref mut conf, ref mut count)) =
                current_entity
            {
                // Accumulate token text (with space separator for word-piece tokens)
                if !token_text.starts_with("##") {
                    entity_text.push(' ');
                }
                entity_text.push_str(token_text.trim_start_matches('#'));
                // Average confidence
                *conf = (*conf * *count as f32 + confidence) / (*count + 1) as f32;
                *count += 1;
            } else {
                // I- without preceding B- — treat as new entity
                let etype = label_suffix_to_entity_type(&label[2..]);
                current_entity = Some((token_text.to_string(), etype, confidence, 1));
            }
        } else {
            // O label — flush current entity
            if let Some((entity_text, entity_type, conf, _)) = current_entity.take() {
                if conf >= min_confidence && !entity_text.trim().is_empty() {
                    entities.push(ExtractedEntity {
                        text: entity_text.trim().to_string(),
                        entity_type,
                        confidence: conf,
                    });
                }
            }
        }
    }

    // Flush remaining entity
    if let Some((entity_text, entity_type, conf, _)) = current_entity.take() {
        if conf >= min_confidence && !entity_text.trim().is_empty() {
            entities.push(ExtractedEntity {
                text: entity_text.trim().to_string(),
                entity_type,
                confidence: conf,
            });
        }
    }

    // Deduplicate by text (case-insensitive)
    entities.dedup_by(|a, b| a.text.to_lowercase() == b.text.to_lowercase());

    Ok(entities)
}

/// Map BIO tag suffix to `EntityType`.
fn label_suffix_to_entity_type(suffix: &str) -> EntityType {
    match suffix {
        "PER" => EntityType::Person,
        "ORG" => EntityType::Organization,
        "LOC" => EntityType::Location,
        "MISC" => EntityType::Concept,
        _ => EntityType::Custom(suffix.to_string()),
    }
}

/// Extract the `id2label` map from a HuggingFace model config JSON.
///
/// Returns labels in index order. Falls back to standard CoNLL-2003 labels
/// if not present in config.
fn extract_id2label(config: &serde_json::Value) -> Vec<String> {
    if let Some(map) = config.get("id2label").and_then(|v| v.as_object()) {
        let mut pairs: Vec<(usize, String)> = map
            .iter()
            .filter_map(|(k, v)| {
                let idx: usize = k.parse().ok()?;
                let label = v.as_str()?.to_string();
                Some((idx, label))
            })
            .collect();
        pairs.sort_by_key(|(k, _)| *k);
        return pairs.into_iter().map(|(_, v)| v).collect();
    }

    // Default CoNLL-2003 labels
    vec![
        "O".to_string(),
        "B-MISC".to_string(),
        "I-MISC".to_string(),
        "B-PER".to_string(),
        "I-PER".to_string(),
        "B-ORG".to_string(),
        "I-ORG".to_string(),
        "B-LOC".to_string(),
        "I-LOC".to_string(),
    ]
}

/// Compute softmax along the last dimension.
fn softmax_last_dim(tensor: &Tensor) -> candle_core::Result<Tensor> {
    let max = tensor.max_keepdim(candle_core::D::Minus1)?;
    let diff = tensor.broadcast_sub(&max)?;
    let exp = diff.exp()?;
    let sum = exp.sum_keepdim(candle_core::D::Minus1)?;
    exp.broadcast_div(&sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_label_suffix_mapping() {
        assert!(matches!(
            label_suffix_to_entity_type("PER"),
            EntityType::Person
        ));
        assert!(matches!(
            label_suffix_to_entity_type("ORG"),
            EntityType::Organization
        ));
        assert!(matches!(
            label_suffix_to_entity_type("LOC"),
            EntityType::Location
        ));
        assert!(matches!(
            label_suffix_to_entity_type("MISC"),
            EntityType::Concept
        ));
    }

    #[test]
    fn test_extract_id2label_default() {
        let labels = extract_id2label(&serde_json::json!({}));
        assert_eq!(labels[0], "O");
        assert_eq!(labels[3], "B-PER");
        assert_eq!(labels.len(), 9);
    }

    #[test]
    fn test_extract_id2label_from_config() {
        let config = serde_json::json!({
            "id2label": {
                "0": "O",
                "1": "B-PER",
                "2": "I-PER"
            }
        });
        let labels = extract_id2label(&config);
        assert_eq!(labels, vec!["O", "B-PER", "I-PER"]);
    }
}
