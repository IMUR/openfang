//! Local model cache for Candle inference.
//!
//! Downloads safetensors weights, tokenizer.json, and config.json from
//! Hugging Face Hub on first use, caching them under `~/.openfang/models/`.
//!
//! Model IDs use the standard HF format: `"BAAI/bge-small-en-v1.5"`.
//! Files are stored at `{home_dir}/models/{owner}/{name}/`.

use hf_hub::{api::tokio::Api, Repo, RepoType};
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Error type for model cache operations.
#[derive(Debug, thiserror::Error)]
pub enum ModelCacheError {
    #[error("HF Hub error: {0}")]
    Hub(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Model file not found: {file} in {model_id}")]
    FileMissing { model_id: String, file: String },
}

/// Paths for all files belonging to a resolved model.
#[derive(Debug, Clone)]
pub struct ModelFiles {
    /// Primary weights file (model.safetensors or pytorch_model.bin).
    pub weights: PathBuf,
    /// HuggingFace tokenizer definition.
    pub tokenizer: PathBuf,
    /// Model architecture config.
    pub config: PathBuf,
}

/// Resolve or download a model from Hugging Face Hub.
///
/// Files are downloaded to `{home_dir}/models/{model_id}/` and cached.
/// Subsequent calls return the cached paths without hitting the network.
///
/// # Arguments
///
/// - `model_id` — HF model ID, e.g. `"BAAI/bge-small-en-v1.5"`
/// - `home_dir` — OpenFang home directory (`~/.openfang`)
/// - `revision` — Git revision or branch (default: `"main"`)
pub async fn resolve_model(
    model_id: &str,
    home_dir: &std::path::Path,
    revision: Option<&str>,
) -> Result<ModelFiles, ModelCacheError> {
    let rev = revision.unwrap_or("main");

    // Check local cache first (no HF Hub call if already present)
    let cache_dir = model_cache_dir(home_dir, model_id);
    let weights_path = best_weights_path(&cache_dir);
    let tokenizer_path = cache_dir.join("tokenizer.json");
    let config_path = cache_dir.join("config.json");

    if weights_path.exists() && tokenizer_path.exists() && config_path.exists() {
        debug!(model_id, "Model cache hit");
        return Ok(ModelFiles {
            weights: weights_path,
            tokenizer: tokenizer_path,
            config: config_path,
        });
    }

    // Download from HF Hub
    info!(model_id, revision = rev, "Downloading model from HF Hub");
    std::fs::create_dir_all(&cache_dir)?;

    let api = Api::new().map_err(|e| ModelCacheError::Hub(e.to_string()))?;
    let repo = api.repo(Repo::with_revision(
        model_id.to_string(),
        RepoType::Model,
        rev.to_string(),
    ));

    // Download config.json
    let config_hf = repo
        .get("config.json")
        .await
        .map_err(|e| ModelCacheError::Hub(format!("config.json: {e}")))?;
    std::fs::copy(&config_hf, &config_path)?;

    // Download tokenizer.json (some NER/classification repos omit it; vocab matches bert-base-cased)
    let tokenizer_hf = match repo.get("tokenizer.json").await {
        Ok(p) => p,
        Err(e) => {
            warn!(
                model_id,
                error = %e,
                "tokenizer.json missing from repo; trying bert-base-cased tokenizer (HF fallback)"
            );
            let fallback = api.repo(Repo::with_revision(
                "bert-base-cased".to_string(),
                RepoType::Model,
                rev.to_string(),
            ));
            fallback.get("tokenizer.json").await.map_err(|e2| {
                ModelCacheError::Hub(format!(
                    "tokenizer.json: primary repo {e}; bert-base-cased fallback {e2}"
                ))
            })?
        }
    };
    std::fs::copy(&tokenizer_hf, &tokenizer_path)?;

    // Download weights: prefer safetensors, fall back to pytorch bin
    let weights_hf = if let Ok(p) = repo.get("model.safetensors").await {
        p
    } else {
        repo.get("pytorch_model.bin")
            .await
            .map_err(|e| ModelCacheError::Hub(format!("weights: {e}")))?
    };
    let weights_dest = if weights_hf
        .to_string_lossy()
        .ends_with("model.safetensors")
    {
        cache_dir.join("model.safetensors")
    } else {
        cache_dir.join("pytorch_model.bin")
    };
    std::fs::copy(&weights_hf, &weights_dest)?;

    info!(model_id, path = ?cache_dir, "Model cached");

    Ok(ModelFiles {
        weights: weights_dest,
        tokenizer: tokenizer_path,
        config: config_path,
    })
}

/// Return the local cache directory for a model.
///
/// `"BAAI/bge-small-en-v1.5"` → `~/.openfang/models/BAAI/bge-small-en-v1.5/`
pub fn model_cache_dir(home_dir: &std::path::Path, model_id: &str) -> PathBuf {
    home_dir.join("models").join(model_id)
}

/// Return the best available weights file in a cache directory.
///
/// Prefers `model.safetensors` over `pytorch_model.bin`.
fn best_weights_path(cache_dir: &std::path::Path) -> PathBuf {
    let safetensors = cache_dir.join("model.safetensors");
    if safetensors.exists() {
        safetensors
    } else {
        cache_dir.join("pytorch_model.bin")
    }
}

/// Check whether a model is already in the local cache.
pub fn is_cached(home_dir: &std::path::Path, model_id: &str) -> bool {
    let cache_dir = model_cache_dir(home_dir, model_id);
    best_weights_path(&cache_dir).exists()
        && cache_dir.join("tokenizer.json").exists()
        && cache_dir.join("config.json").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_cache_dir() {
        let home = std::path::Path::new("/home/user/.openfang");
        let dir = model_cache_dir(home, "BAAI/bge-small-en-v1.5");
        assert_eq!(
            dir,
            std::path::PathBuf::from("/home/user/.openfang/models/BAAI/bge-small-en-v1.5")
        );
    }

    #[test]
    fn test_is_cached_false_when_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(!is_cached(tmp.path(), "BAAI/bge-small-en-v1.5"));
    }
}
