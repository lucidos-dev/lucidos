pub mod extractor;
pub mod fastembed;
pub mod pgvector;
pub mod provider;
pub mod reembed;

pub use extractor::{ExtractedFact, MemoryExtractor, QueryClassification};
pub use fastembed::FastEmbedProvider;
pub use pgvector::{MemoryEntry, MemorySource, MemoryStats, PgVectorIndex, SearchResult};
pub use provider::EmbeddingProvider;

/// Drop memory entries below this importance before semantic ranking.
/// Multilingual-e5 embeddings have a high noise floor (~0.85 cosine for
/// any pair), so unimportant entries surface as false positives. Shared by
/// LLM context building and the user-facing thread search.
pub const RETRIEVAL_MIN_IMPORTANCE: f32 = 0.3;

/// Discard semantic hits below this cosine similarity. Below ~0.4 the
/// multilingual-e5 noise floor dominates and the match is effectively random.
/// Shared by LLM context building and the user-facing thread search.
pub const RETRIEVAL_MIN_SIMILARITY: f64 = 0.4;

/// Cosine similarity between two embedding vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}
