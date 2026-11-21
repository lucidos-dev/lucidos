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
    fn only_composing_allows_a_mode_change() {
        assert!(ThreadState::Composing.can_change_mode());
        assert!(!ThreadState::Active.can_change_mode());
        assert!(!ThreadState::Discarded.can_change_mode());
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
