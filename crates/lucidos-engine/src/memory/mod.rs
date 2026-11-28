pub mod embedder_slot;
pub mod extractor;
pub mod fastembed;
pub mod legacy_cache;
pub mod model_download;
pub mod pgvector;
pub mod provider;
pub(crate) mod query_judgment;
pub mod reembed;

pub use embedder_slot::{EmbedderSlot, EmbeddingModelLoadState, EmbeddingModelStatus};
pub use extractor::{ExtractedFact, QueryClassification, EXTRACTOR_VERSION};
pub use fastembed::FastEmbedProvider;
pub use model_download::{
    apply_default_cache_dir, ensure_model_cached, CacheOutcome, DownloadFrame,
    ModelDownloadObserver,
};
pub use pgvector::{
    CorrectedMemory, MemoryEntry, MemorySource, MemoryStats, PgVectorIndex, SearchResult,
};
pub use provider::EmbeddingProvider;
pub use {extractor::classify_query, extractor::summarize_conversation};
pub(crate) use {extractor::extract_facts, extractor::fallback_fact};

pub const RETRIEVAL_MIN_IMPORTANCE: f32 = 0.3;

pub const RETRIEVAL_MIN_SIMILARITY: f64 = 0.4;

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}
