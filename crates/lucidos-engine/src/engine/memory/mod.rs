//! Engine-side memory subsystem. Split by responsibility seam:
//! - [`extract`] — build extraction context, turn an event/text/artifact into
//!   facts, dedup, and persist (`index_*`, `index_memory_inner_impl`).
//! - [`rebuild`] — batch/derived operations: artifact summaries, user-profile
//!   generation, full/incremental rebuild, correction replay, post-import hook.
//! - [`scoring`] — pure similarity/decay helpers (Jaccard, age, relevance).
//!
//! All methods hang off `impl LucidosEngine` in their child file, so external
//! callers reach them via the type, unchanged. The free scoring helpers and
//! `MEMORY_CORRECTION_THRESHOLD` are re-exported here so existing
//! `engine::memory::<name>` paths keep resolving.

mod extract;
mod rebuild;
mod scoring;

pub(crate) use scoring::{
    age_in_days, jaccard_similarity, relevance_score, MEMORY_CORRECTION_THRESHOLD,
};
