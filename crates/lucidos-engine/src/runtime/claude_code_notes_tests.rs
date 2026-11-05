use super::*;
use NotesVisibility::{Hidden, Shown};

#[test]
fn an_always_thinking_model_starts_hidden() {
    for model in [
        "claude-opus-5-5",
        "claude-opus-5-5[1m]",
        "claude-sonnet-5-5",
        "claude-fable-5-1",
    ] {
        assert_eq!(notes_visibility(Some(model)), Hidden, "{model}");
    }
}

#[test]
fn an_unknown_model_starts_hidden() {
    for model in [None, Some(""), Some("default"), Some("claude-unreleased-9")] {
        assert_eq!(notes_visibility(model), Hidden, "{model:?}");
    }
}

#[test]
fn a_model_that_is_not_always_thinking_starts_shown() {
    for model in [
        "claude-opus-4-7",
        "claude-opus-5",
        "claude-sonnet-5[1m]",
        "claude-haiku-5-5",
    ] {
        assert_eq!(notes_visibility(Some(model)), Shown, "{model}");
    }
}
