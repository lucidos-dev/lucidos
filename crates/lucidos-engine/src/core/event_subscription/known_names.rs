//! The **known event names** a subscription can ever match, and the near-miss
//! matcher that tells a misspelled engine name from a workspace domain event.
//!
//! Both halves of the corpus are DERIVED. The thread half is
//! `all_persisted_event_types()` filtered by `classify_event`, the exact
//! expression that generates `EVENT_CLASSIFICATION` in the frontend's
//! `thread-lifecycle.ts`. The system half is
//! [`SystemEvent::PERSISTED_TYPE_NAMES`]. Nothing here restates a name.
//!
//! That is the whole point. Two renames have already broken live subscriptions
//! silently (`MemorySearched` to `MemoryRecalled`, `ClaudeCodeIdled` to
//! `CodingAgentIdled`), and a hand-copied list would have been the third.
//!
//! [`SystemEvent::PERSISTED_TYPE_NAMES`]: crate::engine::event_bus::SystemEvent::PERSISTED_TYPE_NAMES

use std::collections::HashMap;

use super::{SubscriptionSurface, EVENT_WAIT_EVENT_TYPES, PER_TOKEN_STREAMING_EVENT_TYPES};

/// How close a name must sit to a known one before Rule A calls it a misspelling.
const SUBJECT_MATCH_MAX_RATIO: f64 = 0.45;

/// The same threshold for Rule B, which has only the first token to go on and
/// so has to be stricter.
const FIRST_TOKEN_MATCH_MAX_RATIO: f64 = 0.30;

/// How many near matches an error message names.
const MAX_SUGGESTIONS: usize = 3;

/// **Every name a subscription on `surface` can match**, derived from the two
/// enumerations the engine already keeps.
///
/// The per-token streaming family is dropped at both surfaces, because
/// [`super::validate_subscribable_event_type`] refuses it with a message of its
/// own. Suggesting one would trade a silent failure for a loud wrong answer.
/// The `EventWait*` family is dropped at the wait surface only.
///
/// **This is the one list.** It answers the near-match heuristic, it seeds the
/// trigger form's dropdown, and the `event_types` tool action returns it. A
/// name the agent reads off that action therefore always validates.
pub fn subscribable_event_type_names(surface: SubscriptionSurface) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = crate::engine::thread_lifecycle::all_persisted_event_types()
        .into_iter()
        .filter(|name| crate::engine::thread_lifecycle::classify_event(name).is_some())
        .collect();
    names.extend_from_slice(crate::engine::event_bus::SystemEvent::PERSISTED_TYPE_NAMES);
    names.retain(|name| {
        !PER_TOKEN_STREAMING_EVENT_TYPES.contains(name)
            && (surface == SubscriptionSurface::Trigger || !EVENT_WAIT_EVENT_TYPES.contains(name))
    });
    names.sort_unstable();
    names.dedup();
    names
}

/// Split a PascalCase name into its words. `CredentialStored` becomes
/// `["Credential", "Stored"]`.
///
/// A run of capitals stays one token (`AppUI` yields `App` then `UI`), which is
/// what keeps `AppUiCaptureRequested` and `CaptureAppUI` from looking alike.
pub fn pascal_tokens(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let starts_word = c.is_uppercase()
            && !current.is_empty()
            && (!current.chars().last().is_some_and(|p| p.is_uppercase())
                || chars.get(i + 1).is_some_and(|n| n.is_lowercase()));
        if starts_word {
            tokens.push(std::mem::take(&mut current));
        }
        current.push(c);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Levenshtein edit distance between two names.
fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut cur = vec![0usize; b_chars.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b_chars.iter().enumerate() {
            let substitution = prev[j] + usize::from(ca != cb);
            cur[j + 1] = substitution.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b_chars.len()]
}

/// Edit distance scaled by the longer name's length, so a one-letter slip in a
/// short name and in a long one are comparable.
fn distance_ratio(a: &str, b: &str) -> f64 {
    let longest = a.chars().count().max(b.chars().count());
    if longest == 0 {
        return 0.0;
    }
    levenshtein(a, b) as f64 / longest as f64
}

/// Everything but the last token, joined. The **subject** of an event name:
/// `CredentialStored` is about a credential, `ThreadFinished` about a thread.
fn subject(tokens: &[String]) -> Option<String> {
    (tokens.len() >= 2).then(|| tokens[..tokens.len() - 1].concat())
}

/// **Is `name` a misspelling of an engine event, or a workspace domain event?**
///
/// A verdict only. What to suggest is [`suggest_event_types`]'s question, and
/// it ranks by a different measure.
///
/// Two rules, because plain edit distance cannot tell the two apart:
/// `OrderCreated` sits three edits from `ModelCreated` and must be accepted.
///
/// * **Rule A**: same subject, within [`SUBJECT_MATCH_MAX_RATIO`]. Catches
///   `CredentialStored` and ordinary typos, and misses `EmailReceived`.
/// * **Rule B**: same first token, within [`FIRST_TOKEN_MATCH_MAX_RATIO`].
///   Catches `CredentialRequestResolved`, subject `CredentialRequest`.
///
/// A rename that changed the leading words, such as `ClaudeCodeIdled`, is
/// caught by exact lookup against `ThreadEvent::LEGACY_TYPE_NAME_ALIASES`. No
/// name-only heuristic can separate it from a domain event.
///
/// **Surface-free, and the wait corpus is why.** A trigger accepts everything a
/// wait does, so a verdict reached over the narrower set holds at both.
pub fn is_near_miss(name: &str) -> bool {
    let tokens = pascal_tokens(name);
    let name_subject = subject(&tokens);
    let first = tokens.first();
    subscribable_event_type_names(SubscriptionSurface::Wait)
        .into_iter()
        .any(|known| {
            let known_tokens = pascal_tokens(known);
            let ratio = distance_ratio(name, known);
            let rule_a = name_subject.is_some()
                && subject(&known_tokens) == name_subject
                && ratio <= SUBJECT_MATCH_MAX_RATIO;
            let rule_b = first.is_some()
                && known_tokens.first() == first
                && ratio <= FIRST_TOKEN_MATCH_MAX_RATIO;
            rule_a || rule_b
        })
}

/// The known names closest to `name`, best first, at most
/// [`MAX_SUGGESTIONS`] of them.
///
/// Ranked by edit distance, then pulled towards names that share a **rare**
/// token. Rarity is what makes a rename findable: only `CodingAgentIdled`
/// contains `Idled`, so it outranks every name that merely looks like
/// `ClaudeCodeIdled` letter by letter.
///
/// Drawn from the wait corpus, so every suggestion is usable at either surface.
pub fn suggest_event_types(name: &str) -> Vec<&'static str> {
    let corpus = subscribable_event_type_names(SubscriptionSurface::Wait);
    let mut token_frequency: HashMap<String, usize> = HashMap::new();
    for known in &corpus {
        for token in pascal_tokens(known) {
            *token_frequency.entry(token).or_default() += 1;
        }
    }

    let tokens = pascal_tokens(name);
    let last = tokens.last().cloned();
    let mut scored: Vec<(f64, &'static str)> = corpus
        .iter()
        .map(|&known| {
            let known_tokens = pascal_tokens(known);
            let shared: f64 = known_tokens
                .iter()
                .filter(|t| tokens.contains(t))
                .map(|t| 1.0 / *token_frequency.get(t).unwrap_or(&1) as f64)
                .sum();
            let same_ending = f64::from(u8::from(known_tokens.last() == last.as_ref()));
            let score = distance_ratio(name, known) - 0.45 * shared.min(1.0) - 0.15 * same_ending;
            (score, known)
        })
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    scored
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, known)| known)
        .collect()
}

/// The `Did you mean` clause for an error message, empty when the corpus offers
/// nothing.
pub fn did_you_mean(name: &str) -> String {
    let suggestions = suggest_event_types(name);
    match suggestions.as_slice() {
        [] => String::new(),
        [one] => format!(" Did you mean {one}?"),
        many => format!(" Did you mean {}?", many.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_tokens_splits_words_and_keeps_capital_runs_together() {
        assert_eq!(pascal_tokens("CredentialStored"), ["Credential", "Stored"]);
        assert_eq!(pascal_tokens("AppUiRefreshRequested").len(), 4);
        assert_eq!(pascal_tokens("CaptureAppUI"), ["Capture", "App", "UI"]);
        assert!(pascal_tokens("").is_empty());
    }

    #[test]
    fn levenshtein_counts_single_edits() {
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        assert_eq!(levenshtein("abc", "ac"), 1);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    /// Rule A catches a swapped last word. Rule B catches a name whose subject
    /// grew a token, which is what `CredentialRequestResolved` did.
    #[test]
    fn the_two_rules_catch_the_engine_near_misses() {
        for name in [
            "CredentialStored",
            "CredentialRequestResolved",
            "ThreadFinished",
            "ResponseGenerted",
            "ChangeAplied",
        ] {
            assert!(is_near_miss(name), "{name} can never match");
        }
        for name in ["EmailReceived", "OrderCreated", "ReleaseFinnished"] {
            assert!(!is_near_miss(name), "{name} belongs to the workspace");
        }
    }

    /// The corpus is derived, so it has to be big and it has to hold names from
    /// both halves.
    #[test]
    fn the_corpus_is_derived_from_both_enumerations() {
        let names = subscribable_event_type_names(SubscriptionSurface::Wait);
        assert!(names.len() > 100, "only {} names", names.len());
        assert!(names.contains(&"ResponseGenerated"), "the thread half");
        assert!(names.contains(&"BackupCompleted"), "the system half");
        assert!(!names.contains(&"TextStreamed"), "per-token is dropped");

        let mut sorted = names.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "the two halves overlap");
    }

    /// The two surfaces differ by exactly one family, in one direction. Pinned
    /// because the near-match heuristic reads the narrower set and relies on
    /// the containment.
    #[test]
    fn the_trigger_corpus_is_the_wait_corpus_plus_the_event_wait_family() {
        let wait = subscribable_event_type_names(SubscriptionSurface::Wait);
        let trigger = subscribable_event_type_names(SubscriptionSurface::Trigger);

        assert!(!wait.contains(&"EventWaitStarted"), "a wait self-satisfies");
        assert!(trigger.contains(&"EventWaitStarted"), "a trigger does not");
        assert!(wait.iter().all(|n| trigger.contains(n)), "trigger ⊇ wait");
        assert_eq!(trigger.len(), wait.len() + EVENT_WAIT_EVENT_TYPES.len());
    }

    #[test]
    fn did_you_mean_is_empty_only_when_nothing_is_offered() {
        assert!(did_you_mean("CredentialStored").contains("CredentialCreated"));
        assert!(suggest_event_types("CredentialStored").len() <= MAX_SUGGESTIONS);
    }
}
