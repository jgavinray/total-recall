use crate::config::EmbeddingConfig;
use crate::error::{MemoryError, Result};
use ort::session::Session;
use ort::value::TensorRef;
use std::path::PathBuf;
use std::sync::Mutex;
use tokenizers::Tokenizer;

const LOCAL_MODEL_MAX_SEQ_LEN: usize = 512;

struct ResolvedEmbeddingModel {
    label: String,
    model_path: PathBuf,
    tokenizer_path: PathBuf,
    dimension: usize,
    max_seq_len: usize,
    use_token_type_ids: bool,
}

/// Sentence embedding using the configured ONNX model and tokenizer.
pub struct Embedder {
    // Mutex because Session::run requires &mut self
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    dimension: usize,
    use_token_type_ids: bool,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        Self::from_config(&EmbeddingConfig::default())
    }

    pub fn from_config(config: &EmbeddingConfig) -> Result<Self> {
        let resolved = Self::resolve_model(config)?;
        tracing::info!(
            model = %resolved.label,
            model_path = %resolved.model_path.display(),
            tokenizer_path = %resolved.tokenizer_path.display(),
            dimension = resolved.dimension,
            "Loading embedding model"
        );
        let session = Session::builder()
            .map_err(|e| MemoryError::Embedding(format!("ORT session builder: {e}")))?
            .commit_from_file(&resolved.model_path)
            .map_err(|e| MemoryError::Embedding(format!("Load ONNX model: {e}")))?;

        let mut tokenizer = Tokenizer::from_file(&resolved.tokenizer_path)
            .map_err(|e| MemoryError::Embedding(format!("Load tokenizer: {e}")))?;

        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: resolved.max_seq_len,
                strategy: tokenizers::TruncationStrategy::LongestFirst,
                stride: 0,
                direction: tokenizers::TruncationDirection::Right,
            }))
            .map_err(|e| MemoryError::Embedding(format!("Tokenizer truncation: {e}")))?;

        if tokenizer.get_padding().is_none() {
            tokenizer.with_padding(Some(tokenizers::PaddingParams {
                strategy: tokenizers::PaddingStrategy::BatchLongest,
                direction: tokenizers::PaddingDirection::Right,
                pad_to_multiple_of: None,
                pad_id: 0,
                pad_type_id: 0,
                pad_token: String::from("[PAD]"),
            }));
        }

        tracing::info!(
            model = %resolved.label,
            dimension = resolved.dimension,
            token_type_ids = resolved.use_token_type_ids,
            "Embedder initialized"
        );
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            dimension: resolved.dimension,
            use_token_type_ids: resolved.use_token_type_ids,
        })
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// Embed a single piece of text into a configured-dimensional L2-normalized vector.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        self.embed_batch(&[text])
            .into_iter()
            .next()
            .unwrap_or_else(|| vec![0.0f32; self.dimension])
    }

    /// Embed a batch of texts, returning one configured-dimensional vector per input.
    pub fn embed_batch(&self, texts: &[&str]) -> Vec<Vec<f32>> {
        match self.embed_batch_inner(texts) {
            Ok(embeddings) => embeddings,
            Err(e) => {
                tracing::error!("Embedding failed: {e}; returning zero vectors");
                texts.iter().map(|_| vec![0.0f32; self.dimension]).collect()
            }
        }
    }

    fn embed_batch_inner(
        &self,
        texts: &[&str],
    ) -> std::result::Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        let batch_size = texts.len();
        if batch_size == 0 {
            return Ok(vec![]);
        }

        // Tokenize with padding to the longest sequence in the batch
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| format!("Tokenization: {e}"))?;

        let seq_len = encodings[0].get_ids().len();
        let n = batch_size * seq_len;

        // Build flat i64 tensors for ONNX: layout [batch, seq_len]
        let mut input_ids = vec![0i64; n];
        let mut attention_mask = vec![0i64; n];
        let mut token_type_ids = vec![0i64; n];

        for (i, enc) in encodings.iter().enumerate() {
            let ids = enc.get_ids();
            let mask = enc.get_attention_mask();
            let type_ids = enc.get_type_ids();
            let offset = i * seq_len;
            for j in 0..seq_len {
                input_ids[offset + j] = ids.get(j).copied().unwrap_or(0) as i64;
                attention_mask[offset + j] = mask.get(j).copied().unwrap_or(0) as i64;
                token_type_ids[offset + j] = type_ids.get(j).copied().unwrap_or(0) as i64;
            }
        }

        // Use `([usize; 2], &[T])` tuple form — avoids ndarray version mismatch with ort
        let shape = [batch_size, seq_len];
        // Run ONNX inference (lock mutex for exclusive mutable access to session)
        let mut session_guard = self
            .session
            .lock()
            .map_err(|e| format!("Session lock poisoned: {e}"))?;
        let outputs = if self.use_token_type_ids {
            let input_ids_tensor =
                TensorRef::<i64>::from_array_view((shape, input_ids.as_slice()))?;
            let attn_mask_tensor =
                TensorRef::<i64>::from_array_view((shape, attention_mask.as_slice()))?;
            let type_ids_tensor =
                TensorRef::<i64>::from_array_view((shape, token_type_ids.as_slice()))?;
            session_guard.run(ort::inputs![
                "input_ids" => input_ids_tensor,
                "attention_mask" => attn_mask_tensor,
                "token_type_ids" => type_ids_tensor
            ])?
        } else {
            let input_ids_tensor =
                TensorRef::<i64>::from_array_view((shape, input_ids.as_slice()))?;
            let attn_mask_tensor =
                TensorRef::<i64>::from_array_view((shape, attention_mask.as_slice()))?;
            session_guard.run(ort::inputs![
                "input_ids" => input_ids_tensor,
                "attention_mask" => attn_mask_tensor
            ])?
        };

        // Output[0] = last_hidden_state: [batch, seq_len, hidden_size]
        let output_tensor = outputs[0].try_extract_array::<f32>()?;
        let flat: Vec<f32> = output_tensor.iter().copied().collect();
        let hidden_size = flat.len() / (batch_size * seq_len);

        if hidden_size != self.dimension {
            return Err(format!(
                "configured embedding dimension {} does not match model output dimension {}",
                self.dimension, hidden_size
            )
            .into());
        }

        // Mean-pool with attention mask, then L2 normalize
        let mut result = Vec::with_capacity(batch_size);
        for b in 0..batch_size {
            let mut pooled = vec![0.0f32; hidden_size];
            let mut mask_sum = 0.0f32;

            for s in 0..seq_len {
                let mask_val = attention_mask[b * seq_len + s] as f32;
                if mask_val > 0.0 {
                    mask_sum += mask_val;
                    let token_start = b * seq_len * hidden_size + s * hidden_size;
                    for h in 0..hidden_size {
                        pooled[h] += flat[token_start + h] * mask_val;
                    }
                }
            }

            if mask_sum > 0.0 {
                for v in &mut pooled {
                    *v /= mask_sum;
                }
            }

            // L2 normalize
            let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 1e-6 {
                for v in &mut pooled {
                    *v /= norm;
                }
            }

            result.push(pooled);
        }

        Ok(result)
    }

    pub fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0;
        }
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a * norm_b)
    }

    fn resolve_model(config: &EmbeddingConfig) -> Result<ResolvedEmbeddingModel> {
        let model_name = config.model.trim();
        if model_name.is_empty() {
            return Err(MemoryError::Embedding(
                "embedding.model must name the configured embedding model".to_string(),
            ));
        }

        let requested_path = config.model_path.clone();

        if requested_path.exists() {
            return Self::resolve_local_model(config, requested_path);
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
}

impl Default for Embedder {
    fn default() -> Self {
        Self::new().expect("Failed to initialize embedder")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embed_dim() {
        let embedder = Embedder::new().expect("init");
        let v = embedder.embed("hello world");
        assert_eq!(v.len(), 1024, "embedding should be 1024-dimensional");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "embedding should be L2-normalized, norm={norm}"
        );
    }

    #[test]
    fn test_semantic_similarity() {
        let embedder = Embedder::new().expect("init");
        let dog = embedder.embed("dog");
        let puppy = embedder.embed("puppy");
        let invoice = embedder.embed("invoice");
        let sim_dog_puppy = embedder.cosine_similarity(&dog, &puppy);
        let sim_dog_invoice = embedder.cosine_similarity(&dog, &invoice);
        println!("dog<>puppy  = {sim_dog_puppy:.4}");
        println!("dog<>invoice = {sim_dog_invoice:.4}");
        assert!(
            sim_dog_puppy > sim_dog_invoice,
            "dog should be more similar to puppy ({sim_dog_puppy:.4}) than to invoice ({sim_dog_invoice:.4})"
        );
    }

    /// Verify that embed() is deterministic: same input must produce identical output.
    #[test]
    fn test_embed_determinism() {
        let embedder = Embedder::new().expect("init");
        let text = "the quick brown fox jumps over the lazy dog";
        let v1 = embedder.embed(text);
        let v2 = embedder.embed(text);
        assert_eq!(v1.len(), v2.len(), "output lengths should match");
        for (i, (a, b)) in v1.iter().zip(v2.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "dim {i}: expected identical outputs, got {a} vs {b}"
            );
        }
    }

    /// Verify that embed_batch returns embeddings in the same order as the input.
    #[test]
    fn test_embed_batch_order() {
        let embedder = Embedder::new().expect("init");
        let texts = ["apple", "banana", "cherry"];
        let batch = embedder.embed_batch(&texts);
        assert_eq!(batch.len(), texts.len());
        // Each embedding individually should match the single-embed result
        for (i, text) in texts.iter().enumerate() {
            let single = embedder.embed(text);
            let sim = embedder.cosine_similarity(&batch[i], &single);
            assert!(
                sim > 0.999,
                "batch[{i}] for '{text}' should match single embed (sim={sim:.4})"
            );
        }
    }

    /// Cosine similarity of a vector with itself must be 1.0 (within float tolerance).
    #[test]
    fn test_cosine_similarity_self() {
        let embedder = Embedder::new().expect("init");
        let v = embedder.embed("self-similarity test");
        let sim = embedder.cosine_similarity(&v, &v);
        assert!(
            (sim - 1.0).abs() < 1e-4,
            "self-similarity should be ~1.0, got {sim}"
        );
    }

    /// Cosine similarity of orthogonal unit vectors must be 0.0.
    #[test]
    fn test_cosine_similarity_zero() {
        // Safe: constructing known-orthogonal vectors directly, no I/O involved
        let embedder = Embedder::new().expect("init");
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        let sim = embedder.cosine_similarity(&a, &b);
        assert!(
            (sim - 0.0).abs() < 1e-6,
            "orthogonal vectors should have sim=0, got {sim}"
        );
    }
}
