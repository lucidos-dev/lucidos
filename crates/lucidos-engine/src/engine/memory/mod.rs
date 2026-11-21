//! Engine-side memory subsystem. Split by responsibility seam:
//! - [`extract`] — build extraction context, turn an event/text/artifact into
//!   facts, dedup, and persist (`index_*`, `index_memory_inner_impl`).
//! - [`read`]: the agent's on-demand search, and the walk from a memory back
//!   to its source event.
//! - [`rebuild`] — batch/derived operations: artifact summaries, user-profile
//!   generation, full/incremental rebuild, post-import hook.
//! - [`correction`]: the `MemoryCorrected` record of a user correction, and
//!   its replay at the end of every rebuild.
//! - [`scoring`] — pure similarity/decay helpers (Jaccard, age, relevance).
//! - [`embedder_retry`] — background recovery for a degraded (empty)
//!   `EmbedderSlot` boot (offline first-run model download).
//!
//! All methods hang off `impl LucidosEngine` in their child file, so external
//! callers reach them via the type, unchanged. The free scoring helpers and
//! `MEMORY_CORRECTION_THRESHOLD` are re-exported here so existing
//! `engine::memory::<name>` paths keep resolving.

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
