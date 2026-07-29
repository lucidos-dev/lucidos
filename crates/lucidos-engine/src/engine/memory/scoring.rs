//! Pure scoring/similarity helpers for memory: cosine-threshold constant,
//! Jaccard word-set similarity, age-based time decay, and the blended
//! relevance score. No engine state — all free functions, unit-tested in
//! `memory_tests/`.

/// Cosine similarity threshold for matching wrong_fact against memory candidates
/// in correct_memory. Entries must exceed this to be considered for deletion.
pub(crate) const MEMORY_CORRECTION_THRESHOLD: f32 = 0.65;

/// Jaccard similarity between two strings based on word sets.
/// Returns a value between 0.0 (no overlap) and 1.0 (identical word sets).
pub(crate) fn jaccard_similarity(a: &str, b: &str) -> f32 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();
    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }
    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f32 / union as f32
}

const HALF_LIFE_DAYS: f64 = 365.0;

/// Days elapsed since a timestamp, clamped to non-negative (handles clock skew).
pub(crate) fn age_in_days(
    now: chrono::DateTime<chrono::Utc>,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> f64 {
    (now - timestamp).num_seconds().max(0) as f64 / 86400.0
}

/// Compute combined relevance score for a memory entry.
/// Blends cosine similarity, importance, and time decay into a single score.
///
/// Time decay is gentle: halves the time factor after `HALF_LIFE_DAYS` (1 year).
/// `time_factor = 1.0 / (1.0 + age_days / 365.0)`
pub(crate) fn relevance_score(similarity: f64, importance: f32, age_days: f64) -> f64 {
    let clamped_age = age_days.max(0.0);
    let time_factor = 1.0 / (1.0 + clamped_age / HALF_LIFE_DAYS);
    similarity * (importance as f64) * time_factor
}

#[cfg(test)]
#[path = "../memory_tests/jaccard.rs"]
mod jaccard_tests;

#[cfg(test)]
#[path = "../memory_tests/relevance_score.rs"]
mod relevance_score_tests;

#[cfg(test)]
#[path = "../memory_tests/age_in_days.rs"]
mod age_in_days_tests;
