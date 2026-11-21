//! *Successor sessions*: a coding-agent thread hands off to a fresh session in
//! the same thread, which carries on with a clean context. The branch, the
//! change and the parent's wait stay with the thread.
//!
//! Threads that handed off to a *legacy successor thread* before this keep their
//! links, read here and rendered as they were. Nothing creates new ones.
//!
//! Plan: `docs/plans/2026-10-23-successor-session-in-the-same-thread.md`.

use crate::engine::agent_session::runtime_helpers::RuntimeHelpersExt;
use crate::engine::handoff_sql::pending_handoff_sql;
pub use crate::engine::handoff_state::*;
use crate::engine::pending_handoff::successor_first_message;
pub use crate::engine::pending_handoff::Implementer;
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) fn provider_blocked_input(
    thread_id: Uuid,
    policy: &'static str,
    message: &str,
) -> String {
    let brief = handoff_brief(
        thread_id,
        BriefCause::ProviderBlocked { policy },
        true,
        None,
    );
    successor_first_message(&brief, Some(message), message)
}

/// The input of a fresh session that starts because a resume failed: the
/// thread's recap, then a *handoff brief* that ends on `message`.
pub(crate) async fn resume_failed_input(pool: &PgPool, thread_id: Uuid, message: &str) -> String {
    let brief = handoff_brief(thread_id, BriefCause::ResumeFailed, true, None);
    let input = successor_first_message(&brief, Some(message), message);
    super::agent_session::prepend_reconstruction(pool, thread_id, &input).await
}

/// Give a successor's fresh branch its old thread's plan marker. Best effort:
/// without it the successor's first proposal waits for a marker, which says
/// so and costs no work.
pub(crate) async fn carry_plan_marker(
    pool: &PgPool,
    repo_root: &std::path::Path,
    successor_thread_id: Uuid,
    successor_branch: &str,
) {
    let old_branch = match predecessor_of(pool, successor_thread_id).await {
        Ok(Some(old)) => {
            crate::engine::session_queries::lookup_session_branch_for_thread(pool, old).await
        }
        Ok(None) => None,
        Err(e) => {
            crate::log_legacy!("[Successor] predecessor of {}: {}", successor_thread_id, e);
            None
        }
    };
    let Some(old_branch) = old_branch else {
        return;
    };
    if let Err(e) =
        super::git_ops::carry_planned(pool, repo_root, &old_branch, successor_branch).await
    {
        crate::log_legacy!(
            "[Successor] could not carry the plan marker from {} to {}: {}",
            old_branch,
            successor_branch,
            e
        );
    }
}

/// Threads whose successor session this engine is starting, each with whether
/// another start came in meanwhile. That start runs once this one ends: a
/// successor session may hand off again in its own first turn.
static STARTING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<Uuid, bool>>> =
    std::sync::LazyLock::new(Default::default);

/// Holds `thread_id` in [`STARTING`] until dropped, then runs a start that
/// came in meanwhile.
struct StartingGuard {
    thread_id: Uuid,
    engine: std::sync::Arc<super::LucidosEngine>,
}

impl StartingGuard {
    fn take(engine: &std::sync::Arc<super::LucidosEngine>, thread_id: Uuid) -> Option<Self> {
        let mut starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(again) = starting.get_mut(&thread_id) {
            *again = true;
            return None;
        }
        starting.insert(thread_id, false);
        Some(Self {
            thread_id,
            engine: engine.clone(),
        })
    }
}

impl Drop for StartingGuard {
    fn drop(&mut self) {
        let again = STARTING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.thread_id)
            .unwrap_or(false);
        if again {
            self.engine.start_successor_session(self.thread_id);
        }
    }
}

#[easy_ext::ext(SuccessorExt)]
pub(crate) impl super::LucidosEngine {
    /// Start `thread_id`'s *successor session* once no run holds the thread,
    /// so its old session's process has exited first. Does nothing when no
    /// handoff is pending by then: another input already started the session,
    /// and its first prompt carried the opening. A *next-input start* waits for
    /// that input instead. A closed lane thread starts no engine handoff
    /// ([`successor_to_start`]).
    fn start_successor_session(self: &std::sync::Arc<Self>, thread_id: Uuid) {
        let Some(guard) = StartingGuard::take(self, thread_id) else {
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            wait_until_thread_released(&engine.agent_sessions, thread_id).await;
            if engine.is_shutting_down() {
                crate::log_legacy!(
                    "[Successor] Engine shutting down; thread {} starts its successor session on the next boot",
                    thread_id
                );
                return;
            }
            let handoff = match successor_to_start(engine.pool(), thread_id).await {
                Ok(Some(handoff)) => handoff,
                Ok(None) => return,
                Err(e) => {
                    crate::log_legacy!("[Successor] pending handoff of {}: {}", thread_id, e);
                    return;
                }
            };
            crate::log_legacy!(
                "[Successor] Thread {} starts its successor session on {:?}",
                thread_id,
                handoff.coding_agent
            );
            let implementer = handoff.implementer();
            // The user's request starts the session as their own message. The
            // first prompt puts the opening ahead of it (`first_message`).
            let (prompt, origin) = match handoff.request {
                Some(request) => (request.message, request.by),
                None => (
                    handoff.message,
                    super::thread_events::MessageOrigin::engine(
                        super::thread_events::EngineReason::SuccessorSession,
                    ),
                ),
            };
            // The guard lives as long as the turn's task.
            engine.spawn_engine_coding_turn_as(
                thread_id,
                prompt,
                None,
                origin,
                implementer,
                move |_| drop(guard),
            );
        });
    }
}

/// Wait until no run holds `thread_id`: its process exited, and its teardown
/// let go of the worktree.
async fn wait_until_thread_released(
    agent_sessions: &tokio::sync::Mutex<
        std::collections::HashMap<Uuid, super::types::AgentSession>,
    >,
    thread_id: Uuid,
) {
    loop {
        let holder = {
            let sessions = agent_sessions.lock().await;
            sessions
                .get(&thread_id)
                .filter(|s| crate::engine::session_claim::holds_thread(s))
                .map(|s| s.msg_tx.clone())
        };
        match holder {
            Some(holder) => holder.closed().await,
            None => return,
        }
    }
}

/// Start every successor session a handoff left pending, as at boot: the
/// engine that recorded the handoff may have stopped before it started one.
pub async fn start_pending_successor_sessions(engine: std::sync::Arc<super::LucidosEngine>) {
    let threads: Vec<Uuid> = match sqlx::query_scalar(&format!(
        "SELECT ts.thread_id FROM thread_summaries ts \
         WHERE ts.is_coding_agent AND {}",
        pending_handoff_sql("ts.thread_id")
    ))
    .fetch_all(engine.pool())
    .await
    {
        Ok(threads) => threads,
        Err(e) => {
            crate::log_legacy!("[Successor] Could not list pending handoffs: {}", e);
            return;
        }
    };
    for thread_id in threads {
        engine.start_successor_session(thread_id);
    }
}

#[cfg(test)]
#[path = "successor_tests.rs"]
mod tests;

#[cfg(test)]
mod explicit_pick_tests {
    use super::*;
    use crate::runtime::CodingAgent;
    #[test]
    fn backend_only_is_an_explicit_pick() {
        assert!(!ImplementerRequest::default().is_explicit());
        assert!(ImplementerRequest {
            coding_agent: Some(CodingAgent::Codex),
            ..Default::default()
        }
        .is_explicit());
    }
}
