//! How well a search hit's title holds the query. The one definition behind
//! thread search and every Search Everywhere category. The frontend's
//! `titleMatch` mirrors it for the categories it answers itself, pinned by the
//! generated `title-match-fixture.json`.

use std::cmp::Ordering;

/// Declared weakest first, so the derived `Ord` ranks an exact title above
/// everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TitleMatch {
    /// The title does not hold the query. The hit matched somewhere else, such
    /// as a path, a description or by meaning.
    None,
    /// The query sits inside a word of the title, as in "TimeoutSettings".
    Phrase,
    /// The query starts a word of the title, as in "Open settings".
    WordStart,
    /// The title is the query.
    Exact,
}

/// A [`TitleMatch`] plus the query's share of the title. Within one level, a
/// short title that is mostly the query beats a long one.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub(crate) struct TitleRank {
    pub(crate) level: TitleMatch,
    /// Query length over title length, in characters. 0 when `level` is `None`.
    pub(crate) coverage: f64,
}

impl TitleRank {
    /// Best first.
    pub(crate) fn best_first(a: &Self, b: &Self) -> Ordering {
        b.level
            .cmp(&a.level)
            .then_with(|| b.coverage.total_cmp(&a.coverage))
    }
}

/// Case-insensitive, with whitespace runs collapsed.
fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A word starts at the title's start or after a character that is not a
/// letter or a digit. camelCase is deliberately not a word boundary.
fn starts_a_word(title: &str, at: usize) -> bool {
    title[..at]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric())
}

fn level(title: &str, query: &str) -> TitleMatch {
    if query.is_empty() {
        return TitleMatch::None;
    }
    if title == query {
        return TitleMatch::Exact;
    }
    let mut found = TitleMatch::None;
    for (at, _) in title.match_indices(query) {
        if starts_a_word(title, at) {
            return TitleMatch::WordStart;
        }
        found = TitleMatch::Phrase;
    }
    found
}

pub(crate) fn title_match(title: &str, query: &str) -> TitleMatch {
    level(&normalize(title), &normalize(query))
}

pub(crate) fn title_rank(title: &str, query: &str) -> TitleRank {
    let (title, query) = (normalize(title), normalize(query));
    let level = level(&title, &query);
    let coverage = match level {
        TitleMatch::None => 0.0,
        _ => query.chars().count() as f64 / title.chars().count() as f64,
    };
    TitleRank { level, coverage }
}

#[cfg(test)]
#[path = "title_match_tests.rs"]
mod tests;
