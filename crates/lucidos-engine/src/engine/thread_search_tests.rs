//! Scoring for thread search: the dampening that stops a catch-all thread
//! outranking a focused one, and the combination that stops pure semantic
//! noise outranking a real text match.

use super::{combined_score, dampen_text_score, TEXT_MATCH_DAMPEN_THRESHOLD};

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
