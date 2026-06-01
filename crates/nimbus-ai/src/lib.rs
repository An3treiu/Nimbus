//! Pluggable AI for Nimbus — bring your own provider.
//!
//! All providers implement [`AiProvider`]. v1 uses only embeddings (for
//! semantic search); `chat` is reserved for a later phase. Keys and endpoints
//! are supplied by the user; Nimbus never proxies AI through its own servers.

mod providers;

pub use providers::{OllamaProvider, OpenAiProvider};

use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("ai http error: {0}")]
    Http(String),
    #[error("ai decode error: {0}")]
    Decode(String),
    #[error("ai returned no embeddings")]
    Empty,
}

/// An embedding vector.
pub type Embedding = Vec<f32>;

/// A provider that can turn text into embedding vectors.
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Embed a batch of texts, returning one vector per input (same order).
    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>, AiError>;
}

/// Cosine similarity of two equal-length vectors, in `[-1, 1]`.
/// Returns 0.0 if either vector has zero magnitude or lengths differ.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let mag_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        0.0
    } else {
        dot / (mag_a * mag_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors_have_similarity_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_have_zero_similarity() {
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
    }

    #[test]
    fn opposite_vectors_have_negative_similarity() {
        assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn mismatched_lengths_return_zero() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
    }

    #[test]
    fn zero_vector_returns_zero() {
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }
}
