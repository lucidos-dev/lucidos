//! Rendezvous between a blocked agent waiting for the user's answer and
//! the answer-submission API (POST /api/v1/threads/{thread_id}/answer-question).
//! One broadcast channel per pending tool_use_id: the waiter subscribes;
//! the answer handler sends. Three waiters today: CC's PreToolUse hook and
//! Codex's `ask_user_question` MCP tool (both waiting on
//! POST /api/v1/internal/ask-user-question) and the chat agent's
//! in-process `ask_user_question` tool (waiting inside the agentic loop).
//!
//! In-memory only. On engine restart, both kinds of waiters die — CC
//! hooks with their subprocesses; chat tools with their LLM call. Neither
//! waiter comes back, so **a question answered after its waiter died is
//! delivered out of band, not by re-running the tool**: CC does NOT re-emit
//! the `AskUserQuestion` tool_use on `--resume` (its transcript already closed
//! that call at teardown), so the answer rides the `answered_after_idle`
//! resume message instead (`agent_question::answered_question_recap`). A chat
//! thread parked on a question is PRESERVED across a restart (not aborted):
//! the card stays answerable, and answering with no live waiter (`notify`
//! returns `false`) re-enters the agentic loop via `resume_chat_after_answer`,
//! which reconstructs the `ToolResult` from the persisted answer. Both lanes
//! hand the answer back themselves; neither relies on the agent asking again.
//!
//! The endpoint's crash-recovery lookup (a previously-persisted
//! `UserQuestionAnswered` read back in `walk_question_batch`) still covers the
//! narrower case where the waiter died but the SUBPROCESS did not, so the hook
//! genuinely re-registers: a transient hook error, or an answer that landed in
//! the gap between the two.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// Payload broadcast to a waiting hook when the user answers.
#[derive(Clone, Debug)]
pub struct AnswerPayload {
    /// JSON object: question text → chosen label. Embedded verbatim into
    /// the hook's `updatedInput.answers` field.
    pub answers: serde_json::Value,
}

/// In-memory map of `tool_use_id → broadcast sender`. Cloning the registry
/// is cheap (Arc bump); shared by the engine and the API handlers.
#[derive(Clone, Default)]
pub struct QuestionWaitRegistry {
    inner: Arc<RwLock<HashMap<String, broadcast::Sender<AnswerPayload>>>>,
}

impl QuestionWaitRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register interest in `tool_use_id`. Returns a receiver. Multiple
    /// callers for the same id share the same broadcast channel — second
    /// hook re-fires (e.g. crash recovery) gets the same answer the first
    /// would have.
    pub async fn register(&self, tool_use_id: &str) -> broadcast::Receiver<AnswerPayload> {
        let mut map = self.inner.write().await;
        map.entry(tool_use_id.to_string())
            .or_insert_with(|| broadcast::channel(1).0)
            .subscribe()
    }

    /// Wake every waiter on `tool_use_id`. Returns whether a LIVE in-process
    /// waiter actually received the payload. `false` means no live loop is
    /// blocked on this id — either nothing is registered (the answer arrived
    /// before the hook re-subscribed after a crash) or the registered channel
    /// has no receivers left. The chat answer path keys on this: a `false`
    /// return after an engine restart (the in-process loop died with the
    /// process, so the map is empty) is what tells it to re-enter the agentic
    /// loop instead of relying on an in-memory wake that no longer lands. The
    /// coding-agent path ignores the return (it decides resume via
    /// `agent_sessions` / the DB crash-recovery lookup instead).
    pub async fn notify(&self, tool_use_id: &str, payload: AnswerPayload) -> bool {
        let map = self.inner.read().await;
        match map.get(tool_use_id) {
            // `send` is Ok(n) with n = receivers that got it; Err when zero.
            Some(tx) => tx.send(payload).is_ok(),
            None => false,
        }
    }

    /// Drop the channel for `tool_use_id` after the hook completes its turn.
    pub async fn forget(&self, tool_use_id: &str) {
        self.inner.write().await.remove(tool_use_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn register_and_wake_round_trip() {
        let registry = QuestionWaitRegistry::new();
        let mut waiter = registry.register("toolu_test_1").await;

        let r2 = registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            r2.notify(
                "toolu_test_1",
                AnswerPayload {
                    answers: serde_json::json!({"q?": "Red"}),
                },
            )
            .await;
        });

        let got = timeout(Duration::from_secs(1), waiter.recv())
            .await
            .expect("did not time out")
            .expect("received");

        assert_eq!(got.answers, serde_json::json!({"q?": "Red"}));
    }

    #[tokio::test]
    async fn notify_unknown_id_is_noop() {
        let registry = QuestionWaitRegistry::new();
        registry
            .notify(
                "never-registered",
                AnswerPayload {
                    answers: serde_json::json!({}),
                },
            )
            .await;
    }

    /// `notify` reports whether a LIVE in-process waiter received the wake — the
    /// signal the chat answer path uses to decide it must re-enter the agentic
    /// loop (a restart drops the in-memory waiter, so a post-restart answer sees
    /// `false` and triggers `resume_chat_after_answer`).
    #[tokio::test]
    async fn notify_reports_live_waiter_presence() {
        let registry = QuestionWaitRegistry::new();
        let payload = || AnswerPayload {
            answers: serde_json::json!({"q?": "A"}),
        };

        // Nothing registered (the post-restart shape: the map died with the
        // process) → false.
        assert!(
            !registry.notify("toolu_gone", payload()).await,
            "no registered waiter must report no live loop"
        );

        // A live waiter (loop blocked in-process) → true.
        let waiter = registry.register("toolu_live").await;
        assert!(
            registry.notify("toolu_live", payload()).await,
            "a live in-process waiter must be reported"
        );

        // Receiver dropped (loop gone but entry not yet forgotten) → false.
        drop(waiter);
        assert!(
            !registry.notify("toolu_live", payload()).await,
            "a registered id whose receiver is gone must report no live loop"
        );
    }

    #[tokio::test]
    async fn forget_drops_the_channel() {
        let registry = QuestionWaitRegistry::new();
        let _waiter = registry.register("toolu_x").await;
        registry.forget("toolu_x").await;
        assert!(registry.inner.read().await.get("toolu_x").is_none());
    }
}
