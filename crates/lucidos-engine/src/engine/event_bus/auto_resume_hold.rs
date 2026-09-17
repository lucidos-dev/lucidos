//! The *auto-resume hold*: which children the child-to-parent fan-in must stay
//! quiet about, because the engine has already decided to resume them.
//!
//! A coding-agent turn that dies on a transient upstream `API Error` emits a
//! real `ResponseFailed`, and the fan-in announces that to the parent as a
//! completion. The engine then resumes the same session seconds later. A parent
//! believed such a card, read the child's branch as empty, and spawned a
//! duplicate session onto the same files. Two agents then edited one scroll
//! path.
//!
//! So the two decisions become one. The engine takes a hold before it emits the
//! terminal, and `EventBus::notify_parent_if_child` stands down while the hold
//! is set. The card belongs at the REAL terminal, whatever it turns out to be.
//! Rationale and gap analysis:
//! `docs/plans/2026-09-16-a-terminal-the-engine-will-resume-is-not-a-completion.md`.
//!
//! **A hold spans two emits, and no more.** The run loop drops it once the
//! terminal and its `CodingAgentIdled` are out, and what the release hands back
//! carries the decision onward. A hold is keyed by thread, so one living longer
//! would swallow the next turn's card as well.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Children whose current terminal the engine is about to auto-resume, keyed by
/// thread id. The value is the terminal's error text, which
/// `EventBus::announce_withheld_completion` needs to announce the failure if the
/// resume turns out not to happen.
///
/// `Clone` shares one register: every clone of the bus consults the same map.
///
/// **In memory on purpose, and crash-safe because of what it does NOT touch.** A
/// hold suppresses one announcement. It never clears
/// `thread_summaries.parent_callback_pending`, the durable record that the child
/// still owes its parent a card. An engine death drops every hold, and
/// recovery's resumed turn then reports through the marker as an un-held turn
/// would.
#[derive(Clone, Default)]
pub(crate) struct AutoResumeHolds(Arc<Mutex<HashMap<Uuid, String>>>);

impl AutoResumeHolds {
    /// Withhold the parent's completion card for this child's current terminal.
    ///
    /// Re-taking a hold overwrites the previous error, because a hold describes
    /// ONE terminal and a second call can only be a later one.
    pub(crate) fn hold(&self, thread_id: Uuid, error: String) {
        self.0.lock().unwrap().insert(thread_id, error);
    }

    /// Is the fan-in standing down for this child?
    pub(crate) fn is_held(&self, thread_id: Uuid) -> bool {
        self.0.lock().unwrap().contains_key(&thread_id)
    }

    /// Stop withholding, and hand back the error the held terminal carried.
    /// `None` when nothing was held, which is every ordinary turn.
    pub(crate) fn release(&self, thread_id: Uuid) -> Option<String> {
        self.0.lock().unwrap().remove(&thread_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_with_no_hold_is_not_held() {
        let holds = AutoResumeHolds::default();
        assert!(!holds.is_held(Uuid::new_v4()));
    }

    #[test]
    fn a_hold_is_released_once_and_gives_its_error_back() {
        let holds = AutoResumeHolds::default();
        let thread_id = Uuid::new_v4();
        holds.hold(thread_id, "API Error: Connection lost mid-response.".into());

        assert!(holds.is_held(thread_id));
        assert_eq!(
            holds.release(thread_id).as_deref(),
            Some("API Error: Connection lost mid-response."),
            "the release must hand back what the withheld terminal said, so the \
             announcement can carry it",
        );
        assert!(!holds.is_held(thread_id));
        assert_eq!(
            holds.release(thread_id),
            None,
            "a second release announces nothing: the card already went out",
        );
    }

    /// One child's hold must not silence another's terminal. Several children of
    /// one parent routinely fail together, which is what a network drop looks
    /// like from here.
    #[test]
    fn a_hold_silences_only_the_child_it_names() {
        let holds = AutoResumeHolds::default();
        let held = Uuid::new_v4();
        let other = Uuid::new_v4();
        holds.hold(held, "API Error: 529 overloaded".into());

        assert!(holds.is_held(held));
        assert!(!holds.is_held(other));
    }

    /// A second terminal on the same child replaces the first. The register
    /// answers for the turn that is live, never for one already settled.
    #[test]
    fn re_holding_a_child_keeps_the_newer_error() {
        let holds = AutoResumeHolds::default();
        let thread_id = Uuid::new_v4();
        holds.hold(thread_id, "API Error: first".into());
        holds.hold(thread_id, "API Error: second".into());

        assert_eq!(
            holds.release(thread_id).as_deref(),
            Some("API Error: second")
        );
    }
}
