//! Per-thread coalescing of rapid-fire CC spawn requests.
//!
//! Phase 2 of the CC resume architecture (`docs/plans/2026-04-24-cc-resume-architecture.md`)
//! makes every Claude Code subprocess exit on idle. Without coalescing, two user messages
//! arriving within a few hundred milliseconds while CC is dead would each call
//! `run_direct_agent` independently — racing two Claude Code subprocesses against the
//! same worktree, or (under the prior 3-second debounce) silently dropping the
//! second message with a "duplicate request" error.
//!
//! This module replaces that debounce with a leader-election queue: the first
//! caller becomes the leader and proceeds to spawn CC; subsequent callers
//! within the debounce window queue their messages and return immediately.
//! The leader, before invoking CC, drains the queue and combines all queued
//! messages with its own into a single stream-json input.
//!
//! Phase 5's event-driven spawn dispatcher will subsume this module.
//!
//! ## Lifecycle
//!
//! 1. Caller arrives at the CC spawn entry with a `QueuedMessage`.
//! 2. Caller calls [`CcSpawnCoalescer::try_become_leader`].
//!    - If the entry is empty or the previous leader has held the slot longer
//!      than [`LEADER_LOCK_TIMEOUT`] (assume it crashed without clearing), the
//!      caller becomes the new leader; the helper returns
//!      [`LeaderElection::Leader`].
//!    - Otherwise the caller appends its message to the leader's queue and the
//!      helper returns [`LeaderElection::Queued`].
//! 3. The leader sleeps a short queue-coalescing window (decided externally,
//!    e.g. 250ms in `chat::process`), then calls [`CcSpawnCoalescer::drain_queue`]
//!    to collect any queued follow-ups, combines them with its own message,
//!    and spawns CC.
//! 4. After the spawn completes (success or failure) the leader calls
//!    [`CcSpawnCoalescer::clear`] to release the entry. Late followers that
//!    arrived after `drain_queue` are returned as residual for re-routing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Maximum time the leader's slot is held before assuming the leader crashed
/// without calling [`CcSpawnCoalescer::clear`]. Sized to comfortably exceed
/// worst-case CC spawn duration (worktree creation + npm install + CC startup,
/// typically 5-10s, occasionally up to ~30s on cold starts).
const LEADER_LOCK_TIMEOUT: Duration = Duration::from_secs(60);

/// A user message queued for inclusion in the leader's spawn input.
/// Mirrors the relevant fields of `engine::AgentUserInput` but lives in this
/// module to avoid a circular dependency with the engine types.
#[derive(Debug, Clone)]
pub(crate) struct QueuedMessage {
    pub text: String,
    pub images: Option<Vec<crate::api::ChatImage>>,
    /// UUID of the `MessageReceived` event already emitted by the chat handler
    /// before this message was queued. Used by orphan-recovery paths so a
    /// dropped queue entry can still be re-routed without duplicating the
    /// exchange boundary. None for engine-internal messages.
    pub origin_event_id: Option<Uuid>,
}

/// Per-thread leader-election state. Holds the leader's high-water mark and
/// the queue of follow-ups that arrived after the leader was elected.
struct CoalesceEntry {
    leader_at: Instant,
    queue: Vec<QueuedMessage>,
}

/// Result of [`CcSpawnCoalescer::try_become_leader`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LeaderElection {
    /// Caller is the leader. They must spawn CC and call
    /// [`CcSpawnCoalescer::drain_queue`] before sending the first stream-json
    /// input.
    Leader,
    /// Another caller is already the leader. This caller's message has been
    /// queued for the leader to drain. The caller returns to its HTTP client
    /// without spawning anything; CC's response will arrive via SSE for both
    /// messages.
    Queued,
}

/// Per-thread coalescing state. Wrap in a single `Mutex` so leader election
/// and queue mutation are atomic.
pub(crate) struct CcSpawnCoalescer {
    state: Mutex<HashMap<Uuid, CoalesceEntry>>,
}

impl CcSpawnCoalescer {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Try to become the spawn leader for `thread_id`.
    ///
    /// If no leader exists for this thread, or the existing leader has held
    /// the slot longer than [`LEADER_LOCK_TIMEOUT`] (assume it crashed without
    /// clearing), the caller becomes the new leader: the queue is reset and
    /// `LeaderElection::Leader` is returned.
    ///
    /// Otherwise the caller's `message` is appended to the leader's queue and
    /// `LeaderElection::Queued` is returned. The caller must NOT spawn CC.
    pub(crate) fn try_become_leader(
        &self,
        thread_id: Uuid,
        message: QueuedMessage,
    ) -> LeaderElection {
        let mut guard = self.state.lock().expect("CcSpawnCoalescer mutex poisoned");
        let now = Instant::now();
        match guard.get_mut(&thread_id) {
            Some(entry) if now.duration_since(entry.leader_at) < LEADER_LOCK_TIMEOUT => {
                entry.queue.push(message);
                LeaderElection::Queued
            }
            _ => {
                // No active leader (or expired). Become the leader. The
                // leader's own message is NOT pushed onto the queue — it stays
                // in the leader's caller frame and is combined with the queue
                // after `drain_queue`.
                guard.insert(
                    thread_id,
                    CoalesceEntry {
                        leader_at: now,
                        queue: Vec::new(),
                    },
                );
                let _ = message; // explicit: leader's message is not queued
                LeaderElection::Leader
            }
        }
    }

    /// Drain any messages queued by followers since the leader was elected.
    /// Returns the messages in arrival order. Called by the leader just
    /// before sending the first stream-json input to CC.
    ///
    /// Note: this does NOT clear the leader entry. Followers arriving after
    /// `drain_queue` but before [`Self::clear`] will still be queued — they
    /// will be picked up by the next call to `drain_queue` (e.g. on retry) or
    /// surfaced as orphans on `clear`.
    pub(crate) fn drain_queue(&self, thread_id: Uuid) -> Vec<QueuedMessage> {
        let mut guard = self.state.lock().expect("CcSpawnCoalescer mutex poisoned");
        match guard.get_mut(&thread_id) {
            Some(entry) => std::mem::take(&mut entry.queue),
            None => Vec::new(),
        }
    }

    /// Release the leader entry for `thread_id`. Called after the leader's
    /// spawn completes. Returns any messages still in the queue so the caller
    /// can re-route them (e.g. as orphans).
    pub(crate) fn clear(&self, thread_id: Uuid) -> Vec<QueuedMessage> {
        let mut guard = self.state.lock().expect("CcSpawnCoalescer mutex poisoned");
        guard
            .remove(&thread_id)
            .map(|entry| entry.queue)
            .unwrap_or_default()
    }
}

impl Default for CcSpawnCoalescer {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert leftover queued messages (typically from `clear()` after a leader
/// completed) into orphaned injections so the chat handler can re-route them
/// instead of dropping them. Internal messages without `origin_event_id` are
/// dropped — they have no user-visible exchange to recover.
pub(crate) fn queued_to_orphans(
    queued: Vec<QueuedMessage>,
) -> Vec<crate::engine::InjectedPrompt> {
    queued
        .into_iter()
        .filter_map(|q| {
            let eid = q.origin_event_id?;
            Some(crate::engine::InjectedPrompt {
                text: q.text,
                event_id: Some(eid),
                mode: crate::engine::thread_events::ActorMode::Human,
                spawning_event_id: None,
                images: q.images,
                origin: None,
                kind: crate::engine::InjectedPromptKind::UserText,
            })
        })
        .collect()
}

/// Combine the leader's own message with the queued follow-ups into a single
/// text payload separated by a divider. Images are merged in arrival order.
///
/// Ordering is: leader_text first, then each queued message in the order it
/// was appended. This preserves the user's typed order.
pub(crate) fn combine_messages(
    leader_text: &str,
    leader_images: Option<Vec<crate::api::ChatImage>>,
    queued: Vec<QueuedMessage>,
) -> (String, Option<Vec<crate::api::ChatImage>>) {
    if queued.is_empty() {
        return (leader_text.to_string(), leader_images);
    }
    const SEPARATOR: &str = "\n\n---\n\n";
    let mut text = String::from(leader_text);
    let mut images = leader_images.unwrap_or_default();
    for msg in queued {
        text.push_str(SEPARATOR);
        text.push_str(&msg.text);
        if let Some(imgs) = msg.images {
            images.extend(imgs);
        }
    }
    let images = if images.is_empty() {
        None
    } else {
        Some(images)
    };
    (text, images)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(text: &str) -> QueuedMessage {
        QueuedMessage {
            text: text.into(),
            images: None,
            origin_event_id: None,
        }
    }

    #[test]
    fn first_caller_becomes_leader() {
        let c = CcSpawnCoalescer::new();
        let tid = Uuid::new_v4();
        assert_eq!(
            c.try_become_leader(tid, msg("first")),
            LeaderElection::Leader
        );
    }

    #[test]
    fn second_caller_within_window_is_queued() {
        let c = CcSpawnCoalescer::new();
        let tid = Uuid::new_v4();
        let _ = c.try_become_leader(tid, msg("first"));
        assert_eq!(
            c.try_become_leader(tid, msg("second")),
            LeaderElection::Queued
        );
    }

    #[test]
    fn leader_message_is_not_queued() {
        let c = CcSpawnCoalescer::new();
        let tid = Uuid::new_v4();
        let _ = c.try_become_leader(tid, msg("first"));
        let drained = c.drain_queue(tid);
        assert!(
            drained.is_empty(),
            "leader's own message must not appear in its own drain"
        );
    }

    #[test]
    fn queued_messages_are_drained_in_order() {
        let c = CcSpawnCoalescer::new();
        let tid = Uuid::new_v4();
        let _ = c.try_become_leader(tid, msg("leader"));
        let _ = c.try_become_leader(tid, msg("queued1"));
        let _ = c.try_become_leader(tid, msg("queued2"));
        let drained = c.drain_queue(tid);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].text, "queued1");
        assert_eq!(drained[1].text, "queued2");
    }

    #[test]
    fn drain_does_not_clear_leader_entry() {
        let c = CcSpawnCoalescer::new();
        let tid = Uuid::new_v4();
        let _ = c.try_become_leader(tid, msg("leader"));
        let _ = c.try_become_leader(tid, msg("queued1"));
        let _ = c.drain_queue(tid);
        // A follower arriving immediately after the drain still sees the leader.
        assert_eq!(
            c.try_become_leader(tid, msg("late")),
            LeaderElection::Queued,
            "leader entry must persist past drain so late followers still queue"
        );
        let drained = c.drain_queue(tid);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].text, "late");
    }

    #[test]
    fn clear_releases_leader_and_returns_residual_queue() {
        let c = CcSpawnCoalescer::new();
        let tid = Uuid::new_v4();
        let _ = c.try_become_leader(tid, msg("leader"));
        let _ = c.try_become_leader(tid, msg("queued1"));
        let residual = c.clear(tid);
        assert_eq!(residual.len(), 1);
        assert_eq!(residual[0].text, "queued1");
        // After clear, a new caller becomes leader.
        assert_eq!(
            c.try_become_leader(tid, msg("after_clear")),
            LeaderElection::Leader
        );
    }

    #[test]
    fn separate_threads_do_not_interfere() {
        let c = CcSpawnCoalescer::new();
        let tid_a = Uuid::new_v4();
        let tid_b = Uuid::new_v4();
        assert_eq!(
            c.try_become_leader(tid_a, msg("a1")),
            LeaderElection::Leader
        );
        assert_eq!(
            c.try_become_leader(tid_b, msg("b1")),
            LeaderElection::Leader,
            "different thread must elect its own leader"
        );
    }

    #[test]
    fn queued_to_orphans_keeps_user_messages_drops_internal() {
        let eid = Uuid::new_v4();
        let q = vec![
            QueuedMessage {
                text: "user follow-up".into(),
                images: None,
                origin_event_id: Some(eid),
            },
            QueuedMessage {
                text: "internal message".into(),
                images: None,
                origin_event_id: None,
            },
        ];
        let orphans = queued_to_orphans(q);
        assert_eq!(
            orphans.len(),
            1,
            "internal messages without origin_event_id must be dropped"
        );
        assert_eq!(orphans[0].text, "user follow-up");
        assert_eq!(orphans[0].event_id, Some(eid));
    }

    #[test]
    fn combine_with_no_followups_returns_leader_text_unchanged() {
        let (text, images) = combine_messages("only", None, vec![]);
        assert_eq!(text, "only");
        assert!(images.is_none());
    }

    #[test]
    fn combine_joins_messages_with_separator() {
        let (text, _images) = combine_messages(
            "first",
            None,
            vec![msg("second"), msg("third")],
        );
        assert_eq!(text, "first\n\n---\n\nsecond\n\n---\n\nthird");
    }

    #[test]
    fn combine_preserves_message_order() {
        let (text, _images) =
            combine_messages("L", None, vec![msg("Q1"), msg("Q2"), msg("Q3")]);
        // Leader text must be first.
        assert!(text.starts_with("L"));
        // Queued messages must follow in arrival order.
        let l_pos = text.find("L").unwrap();
        let q1_pos = text.find("Q1").unwrap();
        let q2_pos = text.find("Q2").unwrap();
        let q3_pos = text.find("Q3").unwrap();
        assert!(l_pos < q1_pos && q1_pos < q2_pos && q2_pos < q3_pos);
    }

    /// Regression: a CC spawn takes 5-10s (worktree creation + npm install +
    /// CC startup). Production previously passed `CC_SPAWN_DEBOUNCE` (250ms)
    /// as the leader-lock timeout, so a follow-up arriving in the multi-second
    /// spawn window after the debounce expired but before `clear()` was called
    /// re-elected itself as a new leader and spawned a parallel Claude Code subprocess
    /// against the same worktree. The fix: the slot is held for the full
    /// `LEADER_LOCK_TIMEOUT` regardless of any caller-supplied window.
    #[test]
    fn second_caller_during_spawn_window_is_queued_not_re_elected() {
        let c = CcSpawnCoalescer::new();
        let tid = Uuid::new_v4();
        assert_eq!(
            c.try_become_leader(tid, msg("first")),
            LeaderElection::Leader
        );
        // Sleep past any reasonable debounce window. With the prior bug
        // (250ms timeout), the second call became a new leader here.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            c.try_become_leader(tid, msg("second")),
            LeaderElection::Queued,
            "follow-up arriving during the multi-second spawn window must queue, \
             not start a parallel spawn"
        );
    }

    /// Regression for the bug Task 2.3 fixes: two CC spawn requests arriving
    /// while no Claude Code subprocess is alive must produce exactly one leader and one
    /// queued follower; the leader's combined input must contain BOTH
    /// messages in arrival order.
    #[test]
    fn rapid_fire_idle_messages_coalesce_into_one_spawn() {
        let c = CcSpawnCoalescer::new();
        let tid = Uuid::new_v4();

        // msg1 — first caller, becomes leader.
        let r1 = c.try_become_leader(tid, msg("msg1"));
        assert_eq!(r1, LeaderElection::Leader);

        // msg2 — arrives 100ms later, before the leader has spawned anything.
        std::thread::sleep(Duration::from_millis(100));
        let r2 = c.try_become_leader(tid, msg("msg2"));
        assert_eq!(
            r2, LeaderElection::Queued,
            "second rapid message must be queued, not spawn its own CC"
        );

        // Leader drains the queue and combines messages.
        let queued = c.drain_queue(tid);
        let (combined_text, _images) = combine_messages("msg1", None, queued);

        // Both messages must appear in the leader's combined input in order.
        assert!(combined_text.contains("msg1"));
        assert!(combined_text.contains("msg2"));
        let m1_pos = combined_text.find("msg1").unwrap();
        let m2_pos = combined_text.find("msg2").unwrap();
        assert!(
            m1_pos < m2_pos,
            "messages must appear in arrival order in the combined input"
        );

        // Leader clears after spawn completes.
        let residual = c.clear(tid);
        assert!(
            residual.is_empty(),
            "no residual messages should remain after a clean leader cycle"
        );
    }

}
