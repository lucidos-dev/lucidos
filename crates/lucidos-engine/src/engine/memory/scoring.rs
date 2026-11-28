pub const MEMORY_CORRECTION_THRESHOLD: f32 = 0.65;

pub(crate) const MEMORY_DEDUP_THRESHOLD: f64 = 0.95;

pub(crate) const MEMORY_SUPERSEDE_THRESHOLD: f32 = 0.85;

pub(crate) const MEMORY_DEDUP_CANDIDATES: usize = 5;

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

pub(crate) fn age_in_days(
    now: chrono::DateTime<chrono::Utc>,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> f64 {
    (now - timestamp).num_seconds().max(0) as f64 / 86400.0
}

pub(crate) fn relevance_score(similarity: f64, importance: f32, age_days: f64) -> f64 {
    let clamped_age = age_days.max(0.0);
    let time_factor = 1.0 / (1.0 + clamped_age / HALF_LIFE_DAYS);
    similarity * (importance as f64) * time_factor
}

pub(crate) const KEYWORD_SIMILARITY_PROXY: f64 = 0.6;
pub(crate) const KEYWORD_BOOST: f64 = 1.2;

/// INVARIANT: Shortest token worth a keyword lookup, in BYTES. Below this a word matches
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

pub(crate) fn keywords_for(queries: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    let mut keywords: Vec<String> = queries
        .into_iter()
        .flat_map(|q| {
            q.as_ref()
                .split_whitespace()
                // INVARIANT: No uppercase filter: Norwegian common nouns like "bil" /
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

    #[test]
    fn a_phrase_becomes_one_lookup_per_word() {
        assert_eq!(
            keywords_for(["lucidos launch outcome"]),
            vec!["launch", "lucidos", "outcome"]
        );
    }

    #[test]
    fn short_words_are_dropped() {
        assert_eq!(keywords_for(["is it up to me"]), Vec::<String>::new());
    }

    #[test]
    fn the_length_floor_counts_bytes_exactly_as_the_injection_always_has() {
        assert_eq!(keywords_for(["på"]), vec!["på"]);
        assert_eq!(keywords_for(["at"]), Vec::<String>::new());
    }

    #[test]
    fn edge_punctuation_is_trimmed_and_inner_kept() {
        assert_eq!(
            keywords_for(["(release-process), v0.25.1!"]),
            vec!["release-process", "v0.25.1"]
        );
    }

    #[test]
    fn queries_are_merged_sorted_and_deduped() {
        assert_eq!(
            keywords_for(["launch outcome", "outcome adoption"]),
            vec!["adoption", "launch", "outcome"]
        );
    }
}
