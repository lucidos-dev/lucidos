pub mod correction;
mod embedder_retry;
mod extract;
mod read;
mod rebuild;
mod scoring;

pub use scoring::MEMORY_CORRECTION_THRESHOLD;
pub(crate) use {
    scoring::age_in_days, scoring::jaccard_similarity, scoring::keywords_for,
    scoring::relevance_score, scoring::KEYWORD_BOOST, scoring::KEYWORD_SIMILARITY_PROXY,
};
