use super::*;
use crate::config::EmbeddingConfig;
use std::path::PathBuf;

fn make_embedder() -> Embedder {
    let model_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../models/embed");
    let config = EmbeddingConfig {
        model_path: model_path.clone(),
        cache_dir: model_path,
        ..EmbeddingConfig::default()
    };
    Embedder::from_config(&config).expect("init")
}

#[test]
fn test_embed_dim() {
    let embedder = make_embedder();
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
    let embedder = make_embedder();
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
    let embedder = make_embedder();
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
    let embedder = make_embedder();
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
    let embedder = make_embedder();
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
    let embedder = make_embedder();
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![0.0f32, 1.0, 0.0];
    let sim = embedder.cosine_similarity(&a, &b);
    assert!(
        (sim - 0.0).abs() < 1e-6,
        "orthogonal vectors should have sim=0, got {sim}"
    );
}
