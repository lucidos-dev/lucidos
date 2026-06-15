//! Rendezvous between a blocked agent waiting for the user's answer and
//! the answer-submission API (POST /api/v1/threads/{thread_id}/answer-question).
//! One broadcast channel per pending tool_use_id: the waiter subscribes;
//! the answer handler sends. Three waiters today: CC's PreToolUse hook and
//! Codex's `ask_user_question` MCP tool (both waiting on
//! POST /api/v1/internal/ask-user-question) and the chat agent's
//! in-process `ask_user_question` tool (waiting inside the agentic loop).
//!
//! In-memory only. On engine restart, both kinds of waiters die — CC
//! hooks with their subprocesses; chat tools with their LLM call. On
//! resume, CC re-emits the AskUserQuestion tool_use, the hook fires
//! again, and the endpoint's crash-recovery path reads a previously-
//! persisted UserQuestionAnswered event from the DB instead. Chat
//! tools have no resume — a restarted chat turn aborts via
//! `ResponseAborted` and the persisted card is orphaned by
//! `QUESTION_OVERTAKEN_EVENT_TYPES`; the user re-sends to ask again.

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

    /// Wake every waiter on `tool_use_id`. No-op if nothing is registered
    /// (the answer arrives before the hook re-subscribes after a crash;
    /// crash-recovery path will read the event from DB instead).
    pub async fn notify(&self, tool_use_id: &str, payload: AnswerPayload) {
        let map = self.inner.read().await;
        if let Some(tx) = map.get(tool_use_id) {
            let _ = tx.send(payload);
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
            r2.notify("toolu_test_1", AnswerPayload {
                answers: serde_json::json!({"q?": "Red"}),
            }).await;
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
        registry.notify("never-registered", AnswerPayload {
            answers: serde_json::json!({}),
        }).await;
    }

    #[tokio::test]
    async fn forget_drops_the_channel() {
        let registry = QuestionWaitRegistry::new();
        let _waiter = registry.register("toolu_x").await;
        registry.forget("toolu_x").await;
        assert!(registry.inner.read().await.get("toolu_x").is_none());
    }
}
