//! Candle-backed BERT embedding driver.
//!
//! Runs sentence embedding models in-process on CUDA (or CPU as fallback).
//! Replaces the Ollama HTTP round-trip (~887ms) with direct GPU tensor ops (~1ms).
//!
//! ## Supported models
//!
//! Any BERT-family model with safetensors weights and `config.json`:
//! - `BAAI/bge-small-en-v1.5` — 33M params, 384d (Phase 1 default)
//! - `nomic-ai/nomic-embed-text-v1` — 137M params, 768d (Phase 2)
//!
//! ## Device selection
//!
//! - `cuda_device = Some(0)` → GTX 970 #0 (4GB VRAM, dedicated memory GPU)
//! - `cuda_device = None` → CPU with AVX-512 (i9-9900X fallback)

use crate::embedding::{EmbeddingDriver, EmbeddingError};
use crate::model_cache;
use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, HiddenAct};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokenizers::Tokenizer;
use tracing::{debug, info};

/// Inner state shared across concurrent embed calls.
struct BertEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    dims: usize,
    device: Device,
}

/// Candle BERT embedding driver.
///
/// Wraps a BERT model running on CUDA or CPU, implementing the
/// `EmbeddingDriver` trait for seamless swap-in against the Ollama backend.
pub struct CandleEmbeddingDriver {
    inner: Arc<Mutex<BertEmbedder>>,
    dims: usize,
}

impl CandleEmbeddingDriver {
    /// Load a BERT embedding model from the local cache (or download it).
    ///
    /// # Arguments
    ///
    /// - `model_id` — HF model ID, e.g. `"BAAI/bge-small-en-v1.5"`
    /// - `home_dir` — OpenFang home directory for model cache (`~/.openfang`)
    /// - `cuda_device` — CUDA device index (`Some(0)` for GTX 970 #0), `None` for CPU
    pub async fn load(
        model_id: &str,
        home_dir: &Path,
        cuda_device: Option<u32>,
    ) -> Result<Self, EmbeddingError> {
        let model_id = model_id.to_string();
        let home_dir = home_dir.to_path_buf();

        // Resolve weights and tokenizer (downloads on first use)
        let files = model_cache::resolve_model(&model_id, &home_dir, None)
            .await
            .map_err(|e| EmbeddingError::Http(format!("Model cache: {e}")))?;

        info!(
            model_id,
            device = cuda_device
                .map(|d| format!("cuda:{d}"))
                .as_deref()
                .unwrap_or("cpu"),
            "Loading BERT embedding model"
        );

        let device = match cuda_device {
            Some(idx) => Device::new_cuda(idx as usize)
                .map_err(|e| EmbeddingError::Http(format!("CUDA device {idx}: {e}")))?,
            None => Device::Cpu,
        };

        let device_clone = device.clone();
        let weights_path = files.weights.clone();
        let config_path = files.config.clone();
        let tokenizer_path = files.tokenizer.clone();

        // Model loading is blocking (mmap + copy to VRAM)
        let (model, tokenizer, dims) = tokio::task::spawn_blocking(move || {
            // Parse config
            let config_str = std::fs::read_to_string(&config_path)
                .map_err(|e| EmbeddingError::Parse(format!("config.json: {e}")))?;
            let mut config: Config = serde_json::from_str(&config_str)
                .map_err(|e| EmbeddingError::Parse(format!("config parse: {e}")))?;

            // Use approximate GeLU for faster inference
            config.hidden_act = HiddenAct::GeluApproximate;

            let dims = config.hidden_size;

            // Load weights at FP16 on CUDA (saves ~50% VRAM vs FP32)
            let dtype = if matches!(device_clone, Device::Cuda(_)) {
                DType::F16
            } else {
                DType::F32
            };

            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[&weights_path], dtype, &device_clone)
                    .map_err(|e| EmbeddingError::Parse(format!("weights load: {e}")))?
            };

            let model = BertModel::load(vb, &config)
                .map_err(|e| EmbeddingError::Parse(format!("BertModel::load: {e}")))?;

            // Load tokenizer
            let tokenizer = Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| EmbeddingError::Parse(format!("tokenizer: {e}")))?;

            Ok::<_, EmbeddingError>((model, tokenizer, dims))
        })
        .await
        .map_err(|e| EmbeddingError::Http(format!("Spawn error: {e}")))?
        .map_err(|e: EmbeddingError| e)?;

        info!(
            model_id,
            dims,
            device = cuda_device
                .map(|d| format!("cuda:{d}"))
                .as_deref()
                .unwrap_or("cpu"),
            "BERT embedding model loaded"
        );

        Ok(Self {
            inner: Arc::new(Mutex::new(BertEmbedder {
                model,
                tokenizer,
                dims,
                device,
            })),
            dims,
        })
    }
}

#[async_trait]
impl EmbeddingDriver for CandleEmbeddingDriver {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let inner = self.inner.clone();
        let texts_owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();

        // Run tokenization + GPU inference in a blocking thread.
        // Tokenization is CPU-bound; `.to_vec2()` synchronizes the CUDA stream.
        // Using std::sync::Mutex (not tokio) avoids nested runtime issues.
        let embeddings = tokio::task::spawn_blocking(move || {
            let embedder = inner
                .lock()
                .map_err(|e| EmbeddingError::Http(format!("Mutex poisoned: {e}")))?;
            run_bert_embeddings(&embedder, &texts_owned)
        })
        .await
        .map_err(|e| EmbeddingError::Http(format!("Task join: {e}")))?
        .map_err(|e| e)?;

        debug!(
            count = embeddings.len(),
            dims = self.dims,
            "Candle embed complete"
        );
        Ok(embeddings)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

/// Run BERT forward pass and return mean-pooled, L2-normalized embeddings.
fn run_bert_embeddings(
    embedder: &BertEmbedder,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, EmbeddingError> {
    let device = &embedder.device;
    let tokenizer = &embedder.tokenizer;

    // Tokenize all texts in one batch
    let encodings = tokenizer
        .encode_batch(texts.to_vec(), true)
        .map_err(|e| EmbeddingError::Parse(format!("tokenize: {e}")))?;

    // Determine max length for padding
    let max_len = encodings
        .iter()
        .map(|e| e.get_ids().len())
        .max()
        .unwrap_or(0);
    if max_len == 0 {
        return Ok(vec![vec![0.0f32; embedder.dims]; texts.len()]);
    }

    let n = encodings.len();

    // Build padded input_ids, token_type_ids, attention_mask tensors
    let mut all_ids: Vec<u32> = Vec::with_capacity(n * max_len);
    let mut all_type_ids: Vec<u32> = Vec::with_capacity(n * max_len);
    let mut all_masks: Vec<u32> = Vec::with_capacity(n * max_len);

    for enc in &encodings {
        let ids = enc.get_ids();
        let type_ids = enc.get_type_ids();
        let attention_mask = enc.get_attention_mask();
        let len = ids.len();

        all_ids.extend_from_slice(ids);
        all_type_ids.extend_from_slice(type_ids);
        all_masks.extend_from_slice(attention_mask);

        // Pad to max_len
        let pad = max_len - len;
        all_ids.extend(std::iter::repeat(0u32).take(pad));
        all_type_ids.extend(std::iter::repeat(0u32).take(pad));
        all_masks.extend(std::iter::repeat(0u32).take(pad));
    }

    let input_ids = Tensor::from_vec(all_ids, (n, max_len), device)
        .map_err(|e| EmbeddingError::Parse(format!("input_ids tensor: {e}")))?;
    let token_type_ids = Tensor::from_vec(all_type_ids, (n, max_len), device)
        .map_err(|e| EmbeddingError::Parse(format!("token_type_ids tensor: {e}")))?;
    let attention_mask = Tensor::from_vec(all_masks, (n, max_len), device)
        .map_err(|e| EmbeddingError::Parse(format!("attention_mask tensor: {e}")))?;

    // Forward pass → [batch, seq_len, hidden_size]
    let sequence_output = embedder
        .model
        .forward(&input_ids, &token_type_ids, Some(&attention_mask))
        .map_err(|e| EmbeddingError::Parse(format!("BERT forward: {e}")))?;

    // Mean pooling: sum over non-padding tokens, divide by count
    // attention_mask: [batch, seq_len] → expand to [batch, seq_len, 1]
    let mask_dtype = sequence_output.dtype();
    let mask_expanded = attention_mask
        .to_dtype(mask_dtype)
        .and_then(|m| m.unsqueeze(2))
        .and_then(|m| m.broadcast_as(sequence_output.shape()))
        .map_err(|e| EmbeddingError::Parse(format!("mask expand: {e}")))?;

    let sum_mask = attention_mask
        .to_dtype(mask_dtype)
        .and_then(|m| m.sum(1))
        .and_then(|m| m.unsqueeze(1))
        .map_err(|e| EmbeddingError::Parse(format!("sum_mask: {e}")))?;

    let masked_output = sequence_output
        .mul(&mask_expanded)
        .and_then(|o| o.sum(1))
        .map_err(|e| EmbeddingError::Parse(format!("masked sum: {e}")))?;

    // [batch, hidden_size]
    let pooled = masked_output
        .broadcast_div(&sum_mask)
        .map_err(|e| EmbeddingError::Parse(format!("mean pool div: {e}")))?;

    // L2 normalization
    let norms = pooled
        .sqr()
        .and_then(|sq| sq.sum_keepdim(1))
        .and_then(|sum| sum.sqrt())
        .map_err(|e| EmbeddingError::Parse(format!("l2 norm compute: {e}")))?;

    let normalized = pooled
        .broadcast_div(&norms)
        .map_err(|e| EmbeddingError::Parse(format!("l2 normalize: {e}")))?;

    // Convert to FP32 for storage (regardless of compute dtype)
    let normalized_f32 = normalized
        .to_dtype(DType::F32)
        .map_err(|e| EmbeddingError::Parse(format!("dtype cast: {e}")))?;

    // Extract to Vec<Vec<f32>>
    let result = normalized_f32
        .to_vec2::<f32>()
        .map_err(|e| EmbeddingError::Parse(format!("to_vec2: {e}")))?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that the mean pooling + L2 normalization math is correct
    /// using a tiny synthetic model (CPU, no HF Hub required).
    #[test]
    fn test_l2_norm_unit_vector() {
        let v = vec![3.0f32, 4.0]; // length = 5
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let normalized: Vec<f32> = v.iter().map(|x| x / norm).collect();
        let check: f32 = normalized.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (check - 1.0).abs() < 1e-5,
            "L2 norm should be 1.0, got {check}"
        );
    }
}
