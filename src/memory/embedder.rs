use crate::error::{MemoryError, Result};
use ort::session::Session;
use ort::value::TensorRef;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokenizers::Tokenizer;

const MODEL_URL: &str =
    "https://huggingface.co/onnx-models/all-MiniLM-L6-v2-onnx/resolve/main/model.onnx";
const TOKENIZER_URL: &str =
    "https://huggingface.co/onnx-models/all-MiniLM-L6-v2-onnx/resolve/main/tokenizer.json";
const CACHE_SUBDIR: &str = "total-recall";
const MODEL_FILENAME: &str = "all-MiniLM-L6-v2.onnx";
const TOKENIZER_FILENAME: &str = "all-MiniLM-L6-v2-tokenizer.json";
const EMBEDDING_DIM: usize = 384;
const MAX_SEQ_LEN: usize = 128;

/// Real sentence embedding using all-MiniLM-L6-v2 (ONNX).
///
/// Session is guarded by Mutex so Embedder can be used behind Arc<Embedder> with &self methods.
pub struct Embedder {
    // Mutex because Session::run requires &mut self
    session: Mutex<Session>,
    tokenizer: Tokenizer,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        let cache_dir = Self::cache_dir()?;
        std::fs::create_dir_all(&cache_dir)?;

        let model_path = cache_dir.join(MODEL_FILENAME);
        let tokenizer_path = cache_dir.join(TOKENIZER_FILENAME);

        if !model_path.exists() {
            tracing::info!("Downloading all-MiniLM-L6-v2 ONNX model to {:?}", model_path);
            Self::download_file(MODEL_URL, &model_path)?;
        }

        if !tokenizer_path.exists() {
            tracing::info!(
                "Downloading all-MiniLM-L6-v2 tokenizer to {:?}",
                tokenizer_path
            );
            Self::download_file(TOKENIZER_URL, &tokenizer_path)?;
        }

        tracing::info!("Loading ONNX session from {:?}", model_path);
        let session = Session::builder()
            .map_err(|e| MemoryError::Embedding(format!("ORT session builder: {e}")))?
            .commit_from_file(&model_path)
            .map_err(|e| MemoryError::Embedding(format!("Load ONNX model: {e}")))?;

        tracing::info!("Loading tokenizer from {:?}", tokenizer_path);
        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| MemoryError::Embedding(format!("Load tokenizer: {e}")))?;

        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_SEQ_LEN,
                strategy: tokenizers::TruncationStrategy::LongestFirst,
                stride: 0,
                direction: tokenizers::TruncationDirection::Right,
            }))
            .map_err(|e| MemoryError::Embedding(format!("Tokenizer truncation: {e}")))?;

        tokenizer.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            direction: tokenizers::PaddingDirection::Right,
            pad_to_multiple_of: None,
            pad_id: 0,
            pad_type_id: 0,
            pad_token: String::from("[PAD]"),
        }));

        tracing::info!(
            "Embedder initialized: all-MiniLM-L6-v2 ONNX ({}d)",
            EMBEDDING_DIM
        );
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
        })
    }

    /// Embed a single piece of text into a 384-dim L2-normalized vector.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        self.embed_batch(&[text])
            .into_iter()
            .next()
            .unwrap_or_else(|| vec![0.0f32; EMBEDDING_DIM])
    }

    /// Embed a batch of texts, returning one 384-dim L2-normalized vector per input.
    pub fn embed_batch(&self, texts: &[&str]) -> Vec<Vec<f32>> {
        match self.embed_batch_inner(texts) {
            Ok(embeddings) => embeddings,
            Err(e) => {
                tracing::error!("Embedding failed: {e}; returning zero vectors");
                texts
                    .iter()
                    .map(|_| vec![0.0f32; EMBEDDING_DIM])
                    .collect()
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
        let input_ids_tensor = TensorRef::<i64>::from_array_view((shape, input_ids.as_slice()))?;
        let attn_mask_tensor =
            TensorRef::<i64>::from_array_view((shape, attention_mask.as_slice()))?;
        let type_ids_tensor =
            TensorRef::<i64>::from_array_view((shape, token_type_ids.as_slice()))?;

        // Run ONNX inference (lock mutex for exclusive mutable access to session)
        let mut session_guard = self
            .session
            .lock()
            .map_err(|e| format!("Session lock poisoned: {e}"))?;
        let outputs = session_guard.run(ort::inputs![
            "input_ids" => input_ids_tensor,
            "attention_mask" => attn_mask_tensor,
            "token_type_ids" => type_ids_tensor
        ])?;

        // Output[0] = last_hidden_state: [batch, seq_len, hidden_size]
        let output_tensor = outputs[0].try_extract_array::<f32>()?;
        let flat: Vec<f32> = output_tensor.iter().copied().collect();
        let hidden_size = flat.len() / (batch_size * seq_len);

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

    fn cache_dir() -> Result<PathBuf> {
        let base = dirs::cache_dir().ok_or_else(|| {
            MemoryError::Embedding("Could not determine cache directory".to_string())
        })?;
        Ok(base.join(CACHE_SUBDIR))
    }

    fn download_file(url: &str, dest: &Path) -> Result<()> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| MemoryError::Embedding(format!("Build HTTP client: {e}")))?;

        let response = client
            .get(url)
            .send()
            .map_err(|e| MemoryError::Embedding(format!("Download {url}: {e}")))?;

        if !response.status().is_success() {
            return Err(MemoryError::Embedding(format!(
                "HTTP {} downloading {url}",
                response.status()
            )));
        }

        let bytes = response
            .bytes()
            .map_err(|e| MemoryError::Embedding(format!("Read response body: {e}")))?;

        // Atomic write: temp file → rename
        let tmp_path = dest.with_extension("tmp");
        std::fs::write(&tmp_path, &bytes)
            .map_err(|e| MemoryError::Embedding(format!("Write temp file: {e}")))?;
        std::fs::rename(&tmp_path, dest)
            .map_err(|e| MemoryError::Embedding(format!("Rename temp file: {e}")))?;

        tracing::info!("Downloaded {} bytes from {}", bytes.len(), url);
        Ok(())
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
        assert_eq!(v.len(), 384, "embedding should be 384-dimensional");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "embedding should be L2-normalized, norm={norm}");
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
}
