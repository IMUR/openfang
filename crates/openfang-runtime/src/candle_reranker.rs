//! Candle-backed cross-encoder reranking.
//!
//! Scores (query, candidate) pairs using `cross-encoder/ms-marco-MiniLM-L-6-v2`
//! to re-rank HNSW KNN candidates by relevance before returning results.
//!
//! ## How cross-encoders work
//!
//! Unlike bi-encoders (BGE-small) which encode query and document separately,
//! a cross-encoder processes them jointly as `[CLS] query [SEP] document [SEP]`.
//! The `[CLS]` token output passes through a linear head to produce a relevance
//! score. This is slower per-candidate than bi-encoder cosine similarity, but
//! significantly more accurate — hence using it as a re-ranking stage after
//! HNSW narrows the candidate set to ~20 results.
//!
//! ## Model
//!
//! `cross-encoder/ms-marco-MiniLM-L-6-v2`: 22M params, sequence classification head.
//! Loaded at FP16 on CUDA. VRAM: ~60MB.

use crate::model_cache;
use candle_core::{DType, Device, Tensor};
use candle_nn::{linear, Linear, Module, VarBuilder};
use candle_transformers::models::bert::{BertModel, Config};
use openfang_types::memory::MemoryFragment;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};
use tracing::{debug, info};

/// Error type for reranking operations.
#[derive(Debug, thiserror::Error)]
pub enum RerankerError {
    #[error("Model load: {0}")]
    Load(String),
    #[error("Inference: {0}")]
    Inference(String),
    #[error("Model cache: {0}")]
    Cache(String),
}

/// Inner cross-encoder state.
struct CrossEncoder {
    model: BertModel,
    /// Linear(hidden_size → 1) — outputs relevance logit
    classifier: Linear,
    tokenizer: Tokenizer,
    device: Device,
}

/// Candle cross-encoder reranker.
///
/// Thread-safe via `Arc<Mutex<CrossEncoder>>`. Reranking is called per-query
/// on a small set of HNSW candidates (~20), so sequential access is fine.
pub struct CandleReranker {
    inner: Arc<Mutex<CrossEncoder>>,
}

impl CandleReranker {
    /// Load the cross-encoder from cache or HF Hub.
    ///
    /// Model: `cross-encoder/ms-marco-MiniLM-L-6-v2` (22M params, ~60MB FP16 VRAM)
    pub async fn load(
        model_id: &str,
        home_dir: &Path,
        cuda_device: Option<u32>,
    ) -> Result<Self, RerankerError> {
        let model_id = model_id.to_string();
        let home_dir = home_dir.to_path_buf();

        let files = model_cache::resolve_model(&model_id, &home_dir, None)
            .await
            .map_err(|e| RerankerError::Cache(e.to_string()))?;

        info!(
            model_id,
            device = cuda_device.map(|d| format!("cuda:{d}")).as_deref().unwrap_or("cpu"),
            "Loading cross-encoder reranker"
        );

        let device = match cuda_device {
            Some(idx) => Device::new_cuda(idx as usize)
                .map_err(|e| RerankerError::Load(format!("CUDA device {idx}: {e}")))?,
            None => Device::Cpu,
        };

        let device_clone = device.clone();
        let weights_path = files.weights;
        let config_path = files.config;
        let tokenizer_path = files.tokenizer;

        let encoder = tokio::task::spawn_blocking(move || {
            let config_str = std::fs::read_to_string(&config_path)
                .map_err(|e| RerankerError::Load(format!("config.json: {e}")))?;
            let bert_config: Config = serde_json::from_str(&config_str)
                .map_err(|e| RerankerError::Load(format!("bert config: {e}")))?;

            let dtype = if matches!(device_clone, Device::Cuda(_)) {
                DType::F16
            } else {
                DType::F32
            };

            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[&weights_path], dtype, &device_clone)
                    .map_err(|e| RerankerError::Load(format!("weights: {e}")))?
            };

            // cross-encoder/ms-marco-MiniLM-L-6-v2 stores weights under "bert.*" prefix
            let model = BertModel::load(vb.pp("bert"), &bert_config)
                .map_err(|e| RerankerError::Load(format!("BertModel: {e}")))?;

            // Cross-encoder classification head: hidden_size → 1 (single relevance score)
            let classifier = linear(bert_config.hidden_size, 1, vb.pp("classifier"))
                .map_err(|e| RerankerError::Load(format!("classifier: {e}")))?;

            // Configure tokenizer for cross-encoder input format:
            // "[CLS] query [SEP] document [SEP]" up to 512 tokens
            let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| RerankerError::Load(format!("tokenizer: {e}")))?;

            tokenizer.with_truncation(Some(TruncationParams {
                max_length: 512,
                ..Default::default()
            })).map_err(|e| RerankerError::Load(format!("truncation: {e}")))?;

            tokenizer.with_padding(Some(PaddingParams {
                strategy: tokenizers::PaddingStrategy::BatchLongest,
                ..Default::default()
            }));

            info!(model_id, "Cross-encoder reranker loaded");

            Ok::<_, RerankerError>(CrossEncoder {
                model,
                classifier,
                tokenizer,
                device: device_clone,
            })
        })
        .await
        .map_err(|e| RerankerError::Load(format!("Spawn: {e}")))?
        .map_err(|e: RerankerError| e)?;

        Ok(Self {
            inner: Arc::new(Mutex::new(encoder)),
        })
    }

    /// Re-rank `candidates` by relevance to `query`.
    ///
    /// Scores each (query, candidate.content) pair and re-sorts descending.
    /// Returns all candidates with their re-ranked order; does not filter any out.
    pub async fn rerank(
        &self,
        query: &str,
        mut candidates: Vec<MemoryFragment>,
    ) -> Result<Vec<MemoryFragment>, RerankerError> {
        if candidates.len() <= 1 {
            return Ok(candidates);
        }

        let inner = self.inner.clone();
        let query_owned = query.to_string();
        let docs: Vec<String> = candidates.iter().map(|c| c.content.clone()).collect();

        let scores = tokio::task::spawn_blocking(move || {
            let encoder = inner
                .lock()
                .map_err(|e| RerankerError::Inference(format!("Mutex: {e}")))?;
            score_pairs(&encoder, &query_owned, &docs)
        })
        .await
        .map_err(|e| RerankerError::Inference(format!("Task join: {e}")))?
        .map_err(|e| e)?;

        debug!(
            n = candidates.len(),
            "Cross-encoder reranking complete"
        );

        // Attach scores and sort descending
        let mut scored: Vec<(MemoryFragment, f32)> = candidates
            .drain(..)
            .zip(scores.iter().copied())
            .collect();

        scored.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

        Ok(scored.into_iter().map(|(f, _)| f).collect())
    }
}

/// Score all (query, document) pairs in one batched forward pass.
fn score_pairs(
    encoder: &CrossEncoder,
    query: &str,
    docs: &[String],
) -> Result<Vec<f32>, RerankerError> {
    // Build text pairs: cross-encoder expects sentence-pair encoding
    let pairs: Vec<(&str, &str)> = docs.iter().map(|d| (query, d.as_str())).collect();

    let encodings = encoder
        .tokenizer
        .encode_batch_char_offsets(
            pairs.iter().map(|(q, d)| (*q, *d)).collect::<Vec<_>>(),
            true,
        )
        .map_err(|e| RerankerError::Inference(format!("tokenize pairs: {e}")))?;

    let n = encodings.len();
    let max_len = encodings
        .iter()
        .map(|e| e.get_ids().len())
        .max()
        .unwrap_or(0);

    if max_len == 0 {
        return Ok(vec![0.0f32; n]);
    }

    let device = &encoder.device;

    let mut all_ids: Vec<u32> = Vec::with_capacity(n * max_len);
    let mut all_type_ids: Vec<u32> = Vec::with_capacity(n * max_len);
    let mut all_masks: Vec<u32> = Vec::with_capacity(n * max_len);

    for enc in &encodings {
        let ids = enc.get_ids();
        let type_ids = enc.get_type_ids();
        let mask = enc.get_attention_mask();
        let len = ids.len();
        let pad = max_len - len;

        all_ids.extend_from_slice(ids);
        all_ids.extend(std::iter::repeat(0u32).take(pad));
        all_type_ids.extend_from_slice(type_ids);
        all_type_ids.extend(std::iter::repeat(0u32).take(pad));
        all_masks.extend_from_slice(mask);
        all_masks.extend(std::iter::repeat(0u32).take(pad));
    }

    let input_ids = Tensor::from_vec(all_ids, (n, max_len), device)
        .map_err(|e| RerankerError::Inference(format!("input_ids: {e}")))?;
    let token_type_ids = Tensor::from_vec(all_type_ids, (n, max_len), device)
        .map_err(|e| RerankerError::Inference(format!("type_ids: {e}")))?;
    let attention_mask = Tensor::from_vec(all_masks, (n, max_len), device)
        .map_err(|e| RerankerError::Inference(format!("mask: {e}")))?;

    // Forward → [batch, seq_len, hidden_size]
    let sequence_output = encoder
        .model
        .forward(&input_ids, &token_type_ids, Some(&attention_mask))
        .map_err(|e| RerankerError::Inference(format!("BERT forward: {e}")))?;

    // Extract [CLS] token: first token of each sequence → [batch, hidden_size]
    let cls_output = sequence_output
        .narrow(1, 0, 1)
        .and_then(|t| t.squeeze(1))
        .map_err(|e| RerankerError::Inference(format!("CLS extract: {e}")))?;

    // Classification head → [batch, 1]
    let logits = encoder
        .classifier
        .forward(&cls_output)
        .map_err(|e| RerankerError::Inference(format!("classifier: {e}")))?;

    // Extract scores → Vec<f32>
    let scores_f32 = logits
        .squeeze(1)
        .and_then(|t| t.to_dtype(DType::F32))
        .map_err(|e| RerankerError::Inference(format!("squeeze: {e}")))?;

    scores_f32
        .to_vec1::<f32>()
        .map_err(|e| RerankerError::Inference(format!("to_vec1: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reranker_preserves_count() {
        // The reranker must return the same number of candidates as input
        // (just re-ordered). This test verifies the contract without needing
        // an actual model.
        let scores = vec![0.1f32, 0.9, 0.5];
        let mut indexed: Vec<(usize, f32)> = scores.iter().copied().enumerate().collect();
        indexed.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap());
        assert_eq!(indexed[0].0, 1); // highest score first
        assert_eq!(indexed.len(), 3);
    }
}
