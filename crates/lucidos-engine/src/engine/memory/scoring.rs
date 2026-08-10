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

/// Stand-in similarity for a keyword hit, which has no cosine distance of its
/// own, and the multiplier a keyword hit earns on top.
///
/// Shared by the pre-turn injection (`engine::context::retrieve_context`) and
/// the on-demand search (`engine::memory::read::search_memory_ranked`), which
/// both claim to rank a corpus the same way. They were separate literals until
/// the search was added, and the search silently omitted the boost, so any
/// entry the keyword arm found ranked 20% lower there than in the context block
/// the agent had already been given: two orderings over one corpus, with
/// nothing to decide between them. One definition is what makes that claim
/// true rather than aspirational.
pub(crate) const KEYWORD_SIMILARITY_PROXY: f64 = 0.6;
pub(crate) const KEYWORD_BOOST: f64 = 1.2;

/// Shortest token worth a keyword lookup, in BYTES. Below this a word matches
/// most of the index and discriminates nothing.
///
/// Bytes rather than chars, deliberately, because that is what the pre-turn
/// injection has always measured and this function was extracted from it to be
/// shared, not to change it. The two differ only for short non-ASCII words: a
/// two-character Norwegian preposition like "på" is three bytes and survives a
/// byte test, where a char test would drop it. Tightening that is a change to
/// what every user's memory retrieval returns, so it belongs in a change about
/// retrieval quality with its own evidence, not as a side effect of hoisting a
/// helper.
const MIN_KEYWORD_BYTES: usize = 3;

/// Split a query into the keywords to look up, one per word.
///
/// **A whole phrase is not a keyword.** `search_by_keyword` matches
/// `summary ILIKE '%<needle>%'`, so passing "lucidos launch outcome" asks for
/// that exact substring and finds nothing, essentially always. The injection
/// tokenized from the start; the on-demand search did not, which made its
/// keyword arm dead on every multi-word query, and the tool schema explicitly
/// asks for multi-word queries. Sorted and deduped so the caller's fan-out is
/// deterministic and never repeats a lookup.
pub(crate) fn keywords_for(queries: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    let mut keywords: Vec<String> = queries
        .into_iter()
        .flat_map(|q| {
            q.as_ref()
                .split_whitespace()
                // No uppercase filter: Norwegian common nouns like "bil" /
                // "hund" are valid entity tags but never capitalize.
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
                .filter(|w| w.len() >= MIN_KEYWORD_BYTES)
                .collect::<Vec<_>>()
        })
        .collect();
    keywords.sort();
    keywords.dedup();
    keywords
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

#[cfg(test)]
mod keyword_tests {
    use super::*;

    /// The bug this tokenizer exists for: `search_by_keyword` does
    /// `summary ILIKE '%needle%'`, so a whole question asks for that exact
    /// substring and finds nothing. The on-demand search passed the raw query
    /// for its first day and its keyword arm was dead on every multi-word
    /// input, which is every input the tool schema asks for.
    #[test]
    fn a_phrase_becomes_one_lookup_per_word() {
        assert_eq!(
            keywords_for(["lucidos launch outcome"]),
            vec!["launch", "lucidos", "outcome"]
        );
    }

    /// Words shorter than the floor match most of the index and discriminate
    /// nothing, so they are dropped rather than fanned out on.
    #[test]
    fn short_words_are_dropped() {
        assert_eq!(keywords_for(["is it up to me"]), Vec::<String>::new());
    }

    /// The floor is BYTES, which is what the pre-turn injection has always
    /// measured. This function was hoisted out of it to be shared, so a
    /// two-character non-ASCII word that reaches three bytes must still
    /// survive: switching to chars here would quietly change what every user's
    /// memory retrieval returns.
    #[test]
    fn the_length_floor_counts_bytes_exactly_as_the_injection_always_has() {
        // Two chars, three bytes.
        assert_eq!(keywords_for(["på"]), vec!["på"]);
        // Two chars, two bytes.
        assert_eq!(keywords_for(["at"]), Vec::<String>::new());
    }

    /// Punctuation is trimmed from the edges, not the middle: a hyphenated or
    /// dotted identifier is one token.
    #[test]
    fn edge_punctuation_is_trimmed_and_inner_kept() {
        assert_eq!(
            keywords_for(["(release-process), v0.25.1!"]),
            vec!["release-process", "v0.25.1"]
        );
    }

    /// Sorted and deduped across every query, so a caller fanning out never
    /// issues the same lookup twice and the order is deterministic.
    #[test]
    fn queries_are_merged_sorted_and_deduped() {
        assert_eq!(
            keywords_for(["launch outcome", "outcome adoption"]),
            vec!["adoption", "launch", "outcome"]
        );
    }
}
