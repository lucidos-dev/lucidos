//! Thread state machine.
//!
//! A thread is the unified entity for both drafts and conversations. It enters
//! `Composing` on `ThreadStarted`, transitions to `Active` on `MessageReceived`,
//! to `Discarded` on `ThreadDiscarded` (terminal, only valid from `Composing`),
//! or to `Archived` on `ThreadArchived` (only valid from `Active`).
//!
//! `Archived` is a soft terminal: a follow-up `MessageReceived` revives the
//! thread to `Active` (gmail-like — opening an archived conversation and
//! replying brings it back). Compose writes are accepted in `Archived` so the
//! draft a user types into a re-opened archived thread syncs to peer devices
//! while they compose the reply that will revive it.
//!
//! `Discarded` is a hard terminal — every compose PUT and message POST returns
//! 410 Gone. This is the "make impossible states impossible" lever that
//! replaces the old LWW + tombstone machinery.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadState {
    Composing,
    Active,
    Discarded,
    Archived,
}

impl ThreadState {
    /// Parse the DB column value. Fails loud on unknown strings (per CLAUDE.md
    /// "no silent defaults") — corrupted state should surface, not be masked.
    pub fn from_db_str(s: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        match s {
            "composing" => Ok(Self::Composing),
            "active" => Ok(Self::Active),
            "discarded" => Ok(Self::Discarded),
            "archived" => Ok(Self::Archived),
            other => Err(format!(
                "thread_summaries.state has unexpected value '{}' (expected composing|active|discarded|archived)",
                other
            )
            .into()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Composing => "composing",
            Self::Active => "active",
            Self::Discarded => "discarded",
            Self::Archived => "archived",
        }
    }

    /// Compose updates (text, images, mode) are accepted everywhere except
    /// `Discarded`. Typing into an archived thread is the prelude to the send
    /// that will revive it, so the keystrokes have to flow through.
    pub fn can_compose(self) -> bool {
        matches!(self, Self::Composing | Self::Active | Self::Archived)
    }

    /// Sending a message is allowed from the same states as compose. Sending
    /// from `Archived` revives the thread to `Active` (handled by the
    /// `MessageReceived` projection).
    pub fn can_send(self) -> bool {
        matches!(self, Self::Composing | Self::Active | Self::Archived)
    }

    /// Discarding a thread is only valid from `Composing` — once a thread has
    /// messages, the user should `Archive` it instead.
    pub fn can_discard(self) -> bool {
        matches!(self, Self::Composing)
    }

    /// Mode (lucidos vs claude_code) is only mutable while composing — the
    /// first `MessageReceived` locks it on the thread.
    pub fn can_change_mode(self) -> bool {
        matches!(self, Self::Composing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composing_allows_compose_send_discard_and_mode_change() {
        let s = ThreadState::Composing;
        assert!(s.can_compose());
        assert!(s.can_send());
        assert!(s.can_discard());
        assert!(s.can_change_mode());
    }

    #[test]
    fn active_allows_compose_and_send_but_not_discard_or_mode_change() {
        let s = ThreadState::Active;
        assert!(s.can_compose());
        assert!(s.can_send());
        assert!(!s.can_discard());
        assert!(!s.can_change_mode());
    }

    #[test]
    fn discarded_allows_nothing() {
        let s = ThreadState::Discarded;
        assert!(!s.can_compose());
        assert!(!s.can_send());
        assert!(!s.can_discard());
        assert!(!s.can_change_mode());
    }

    #[test]
    fn archived_allows_compose_and_send_but_not_discard_or_mode_change() {
        let s = ThreadState::Archived;
        // Gmail-like revival: typing + send on an archived thread brings it
        // back to active. The MessageReceived projection performs the actual
        // state flip; the gate just has to let the keystrokes through.
        assert!(s.can_compose());
        assert!(s.can_send());
        assert!(!s.can_discard());
        assert!(!s.can_change_mode());
    }

    #[test]
    fn from_db_str_round_trips() {
        for s in [
            ThreadState::Composing,
            ThreadState::Active,
            ThreadState::Discarded,
            ThreadState::Archived,
        ] {
            assert_eq!(ThreadState::from_db_str(s.as_str()).unwrap(), s);
        }
    }

    #[test]
    fn from_db_str_rejects_unknown() {
        assert!(ThreadState::from_db_str("bogus").is_err());
        assert!(ThreadState::from_db_str("").is_err());
    }
}
