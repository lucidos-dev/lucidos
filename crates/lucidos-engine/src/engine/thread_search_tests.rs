//! Scoring for thread search: the dampening that stops a catch-all thread
//! outranking a focused one, the combination that stops pure semantic noise
//! outranking a real text match, and the ranking by title match and age.

use chrono::{Duration, Utc};
use std::cmp::Ordering;

use super::{
    combined_score, dampen_text_score, rank_order, recency_boost, text_hit_score, title_match,
    RankKey, TitleMatch, TEXT_MATCH_DAMPEN_THRESHOLD,
};

/// A focused thread (few messages) keeps its full text score.
#[test]
fn dampen_preserves_score_for_focused_threads() {
    assert_eq!(dampen_text_score(0.7, 1), 0.7);
    assert_eq!(dampen_text_score(0.7, 50), 0.7);
    assert_eq!(dampen_text_score(0.7, TEXT_MATCH_DAMPEN_THRESHOLD), 0.7);
}

/// A catch-all thread with thousands of messages must be crushed below the
/// pure-semantic floor so it can't outrank focused thematic matches.
#[test]
fn dampen_crushes_huge_catch_all_threads() {
    let dampened = dampen_text_score(0.7, 2023);
    assert!(
        dampened < 0.05,
        "2023-msg thread should drop to noise; got {}",
        dampened
    );
}

/// A focused 6-message thread that fully matches must outrank a 2000-msg
/// catch-all that happens to mention the tokens, even with both contributing
/// the same raw text + semantic signal.
#[test]
fn focused_thread_outranks_catch_all_after_dampening() {
    let focused = combined_score(Some(dampen_text_score(0.7, 6)), Some(0.89));
    let catch_all = combined_score(Some(dampen_text_score(0.7, 2023)), Some(0.87));
    assert!(
        focused > catch_all,
        "focused {} must outrank catch-all {}",
        focused,
        catch_all
    );
}

/// Pure semantic noise must never outrank a real text content match.
/// Multilingual-e5-small produces ~0.85+ similarity for almost any pair, so
/// MAX(text=0.7, semantic=0.88) used to let unrelated threads dominate.
#[test]
fn text_match_outranks_pure_semantic_noise() {
    let text = combined_score(Some(0.7), None);
    let noise = combined_score(None, Some(0.88));
    assert!(
        text > noise,
        "text {} must outrank semantic {}",
        text,
        noise
    );
}

#[test]
fn both_signals_outrank_either_alone() {
    let both = combined_score(Some(0.7), Some(0.9));
    let text_only = combined_score(Some(0.7), None);
    let semantic_only = combined_score(None, Some(0.9));
    assert!(both > text_only);
    assert!(both > semantic_only);
    assert!(
        (both - 1.15).abs() < 1e-9,
        "0.7 + 0.5*0.9 = 1.15, got {}",
        both
    );
}

#[test]
fn semantic_only_is_halved() {
    assert!((combined_score(None, Some(0.88)) - 0.44).abs() < 1e-9);
}

#[test]
fn empty_signals_score_zero() {
    assert_eq!(combined_score(None, None), 0.0);
}

#[test]
fn title_match_ignores_case_and_spacing() {
    assert_eq!(
        title_match("Fix  Search", " fix search "),
        TitleMatch::Exact
    );
    assert_eq!(
        title_match("Thread search ranking", "SEARCH"),
        TitleMatch::Phrase
    );
    assert_eq!(
        title_match("Ranking for search", "search ranking"),
        TitleMatch::None,
        "tokens out of order are not a phrase"
    );
    assert_eq!(title_match("Anything", "   "), TitleMatch::None);
}

#[test]
fn recency_boost_no_timestamp() {
    assert!((recency_boost(0.8, None) - 0.8).abs() < f64::EPSILON);
}

#[test]
fn recency_boost_keeps_a_fresh_hit_whole() {
    assert!(recency_boost(1.0, Some(Utc::now())) > 0.95);
}

#[test]
fn recency_boost_halves_an_old_hit() {
    // exp(-60/14) ~ 0.014, so the boost is ~ 0.5 + 0.5*0.014 ~ 0.507.
    let boosted = recency_boost(1.0, Some(Utc::now() - Duration::days(60)));
    assert!(boosted > 0.49 && boosted < 0.55, "got {boosted}");
}

/// A title naming the query is no incidental keyword, so a long thread keeps
/// its full score. A content hit in the same thread is still dampened.
#[test]
fn a_title_hit_in_a_long_thread_is_not_dampened() {
    assert_eq!(text_hit_score(1.0, 2023, TitleMatch::Exact), 1.0);
    assert_eq!(text_hit_score(1.0, 2023, TitleMatch::Phrase), 1.0);
    assert!(text_hit_score(0.7, 2023, TitleMatch::None) < 0.05);
}

/// A ranking key for a hit `age_days` old with a raw (unboosted) `score`.
fn key(title: TitleMatch, text_hit: bool, score: f64, age_days: i64) -> RankKey {
    let last_activity = Utc::now() - Duration::days(age_days);
    RankKey {
        title,
        text_hit,
        score: recency_boost(score, Some(last_activity)),
        last_activity,
    }
}

/// Two hits that score the same: the newer one ranks first.
#[test]
fn a_newer_hit_outranks_an_equal_older_one() {
    let newer = key(TitleMatch::None, true, 0.7, 0);
    let older = key(TitleMatch::None, true, 0.7, 30);
    assert_eq!(rank_order(&newer, &older), Ordering::Less);
}

/// Recency outweighs a small relevance edge: a fresh content hit beats a
/// slightly stronger one from two months ago.
#[test]
fn recency_can_lift_a_fresh_hit_over_a_stale_stronger_one() {
    let fresh = key(TitleMatch::None, true, 0.7, 0);
    let stale = key(TitleMatch::None, true, 1.1, 60);
    assert_eq!(rank_order(&fresh, &stale), Ordering::Less);
}

/// The title tier is hard: an old thread titled with the query outranks a
/// fresh thread with the strongest possible text plus semantic score.
#[test]
fn an_old_exact_title_outranks_a_fresh_content_hit() {
    let exact = key(TitleMatch::Exact, true, 1.0, 365);
    let phrase = key(TitleMatch::Phrase, true, 1.0, 0);
    let content = key(
        TitleMatch::None,
        true,
        combined_score(Some(1.0), Some(1.0)),
        0,
    );
    assert_eq!(rank_order(&exact, &phrase), Ordering::Less);
    assert_eq!(rank_order(&exact, &content), Ordering::Less);
    assert_eq!(rank_order(&phrase, &content), Ordering::Less);
}

/// Recency must not undo `text_match_outranks_pure_semantic_noise`: a year-old
/// word match still beats today's meaning-only hit at full similarity.
#[test]
fn recency_never_lifts_semantic_noise_over_a_word_match() {
    let old_word_hit = key(TitleMatch::None, true, 0.7, 365);
    let fresh_noise = key(TitleMatch::None, false, combined_score(None, Some(1.0)), 0);
    assert_eq!(rank_order(&old_word_hit, &fresh_noise), Ordering::Less);
}
