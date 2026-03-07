use crate::error::{MemoryError, Result};
use ndarray::Array1;

pub struct Embedder {
    dimension: usize,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        tracing::info!("Initializing embedder with 384-dim hash-based embedding");
        Ok(Self { dimension: 384 })
    }

    pub fn embed(&self, text: &str) -> Vec<f32> {
        let mut embedding = vec![0.0f32; self.dimension];

        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in text.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }

        for i in 0..self.dimension {
            let offset = (hash >> (i % 64)) as usize;
            embedding[i] = ((offset as i64 % 1000) as f32 - 500.0) / 500.0;
        }

        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.001 {
            for e in &mut embedding {
                *e /= norm;
            }
        }

        embedding
    }

    pub fn embed_batch(&self, texts: &[&str]) -> Vec<Vec<f32>> {
        texts.iter().map(|t| self.embed(t)).collect()
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
}

impl Default for Embedder {
    fn default() -> Self {
        Self::new().unwrap()
    }
}
