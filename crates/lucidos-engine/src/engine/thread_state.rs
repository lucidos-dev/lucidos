//! Thread state machine — compose lifecycle only.
//!
//! A thread is the unified entity for both drafts and conversations. It enters
//! `Composing` on `ThreadStarted`, transitions to `Active` on `MessageReceived`,
//! and to `Discarded` on `ThreadDiscarded` (terminal, only valid from
//! `Composing`).
//!
//! The archive flag is intentionally NOT modelled here — it lives on the
//! separate `archive_state` column (`Inbox | Archived`), maintained by the
//! `thread_lifecycle::resolve_transition` contract layer. An archived thread
//! carries `state='active'` plus `archive_state='archived'`; the two axes are
//! orthogonal by construction. Anywhere that needs "is this thread archived"
//! reads `archive_state` directly. Compose / send / blob-upload gates only
//! care about the compose axis, which is what `ThreadState` represents.
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
}

impl ThreadState {
    /// Parse the DB column value. Fails loud on unknown strings (per CLAUDE.md
    /// "no silent defaults") — corrupted state should surface, not be masked.
    pub fn from_db_str(s: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        match s {
            "composing" => Ok(Self::Composing),
            "active" => Ok(Self::Active),
            "discarded" => Ok(Self::Discarded),
            other => Err(format!(
                "thread_summaries.state has unexpected value '{}' (expected composing|active|discarded)",
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
        }
    }

    /// Compose updates (text, images, mode) are accepted in `Composing` and
    /// `Active`. Archived threads have `state='active'` so the gmail-revival
    /// keystrokes flow through naturally without a separate variant.
    pub fn can_compose(self) -> bool {
        matches!(self, Self::Composing | Self::Active)
    }

    /// Sending a message is allowed from the same states as compose.
    pub fn can_send(self) -> bool {
        matches!(self, Self::Composing | Self::Active)
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
    fn from_db_str_round_trips() {
        for s in [
            ThreadState::Composing,
            ThreadState::Active,
            ThreadState::Discarded,
        ] {
            assert_eq!(ThreadState::from_db_str(s.as_str()).unwrap(), s);
        }
    }

    #[test]
    fn from_db_str_rejects_archived_and_unknown() {
        // `archived` was a legal value before the state↔archive_state collapse;
        // post-migration no row may carry it. The parser rejects it loud so a
        // stray row surfaces as a 500 instead of silently routing to an arm
        // that no longer exists.
        assert!(ThreadState::from_db_str("archived").is_err());
        assert!(ThreadState::from_db_str("bogus").is_err());
        assert!(ThreadState::from_db_str("").is_err());
    }
}
