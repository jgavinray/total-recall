use crate::config::EmbeddingConfig;
use crate::error::{MemoryError, Result};
use std::path::PathBuf;

const LOCAL_MODEL_MAX_SEQ_LEN: usize = 512;

pub(super) struct ResolvedEmbeddingModel {
    pub(super) label: String,
    pub(super) model_path: PathBuf,
    pub(super) tokenizer_path: PathBuf,
    pub(super) dimension: usize,
    pub(super) max_seq_len: usize,
    pub(super) use_token_type_ids: bool,
}

pub(super) fn resolve_model(config: &EmbeddingConfig) -> Result<ResolvedEmbeddingModel> {
    let model_name = config.model.trim();
    if model_name.is_empty() {
        return Err(MemoryError::Embedding(
            "embedding.model must name the configured embedding model".to_string(),
        ));
    }

    let requested_path = config.model_path.clone();

    if requested_path.exists() {
        return resolve_local_model(config, requested_path);
    }

    Err(MemoryError::Embedding(format!(
        "embedding.model_path must point at a local model directory for {}; got {}",
        config.model,
        requested_path.display()
    )))
}

fn resolve_local_model(
    config: &EmbeddingConfig,
    model_path: PathBuf,
) -> Result<ResolvedEmbeddingModel> {
    let (model_path, tokenizer_path) = if model_path.is_dir() {
        (
            model_path.join("model.onnx"),
            model_path.join("tokenizer.json"),
        )
    } else {
        (model_path, config.cache_dir.join("tokenizer.json"))
    };

    if !model_path.exists() {
        return Err(MemoryError::Embedding(format!(
            "configured ONNX model not found at {}",
            model_path.display()
        )));
    }
    if !tokenizer_path.exists() {
        return Err(MemoryError::Embedding(format!(
            "configured tokenizer not found at {}",
            tokenizer_path.display()
        )));
    }
    if config.dimension == 0 {
        return Err(MemoryError::Embedding(
            "embedding.dimension must be greater than zero".to_string(),
        ));
    }

    Ok(ResolvedEmbeddingModel {
        label: config.model.clone(),
        model_path,
        tokenizer_path,
        dimension: config.dimension,
        max_seq_len: LOCAL_MODEL_MAX_SEQ_LEN,
        use_token_type_ids: false,
    })
}
