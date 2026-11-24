//! Claim the live coding-agent session for an in-place apply.

use uuid::Uuid;

/// Outcome of `begin_in_place_merge`'s atomic claim on a thread's live session.
pub(crate) enum InPlaceMergeStart {
    /// No live, non-exited session with a worktree — caller falls through to the
    /// dead-session / temp-worktree merge tiers.
    NoLiveSession,
    /// Another operation holds the session's change claim, named here.
    Claimed(crate::engine::types::ChangeClaim),
    /// A Stop, Discard or Archive is ending the session (`pending_stop`).
    SessionStopping,
    /// Session claimed; `change_claim` is now `Apply`. Caller owns clearing it.
    Ready(crate::engine::change_ops::LiveSessionInfo),
}

/// Pure decision core of `begin_in_place_merge` and `apply_now`, split out so
/// the claim state machine is unit-testable without a full engine. A session is
/// claimable when it is live and has a worktree. No apply may already run on
/// it, and no stop may be ending it.
///
/// A pending stop still reads live until the loop breaks out, and a Discard
/// deletes the branch the merge would publish. `claude_code::stop_refusal`
/// refuses the other direction.
#[derive(Debug, PartialEq, Eq)]
pub enum InPlaceMergeClaim {
    NoLiveSession,
    Claimed(crate::engine::types::ChangeClaim),
    SessionStopping,
    Claim,
}

pub fn decide_in_place_merge_claim(
    session: Option<&crate::engine::AgentSession>,
) -> InPlaceMergeClaim {
    let Some(s) = session.filter(|s| s.is_live() && s.worktree_path.is_some()) else {
        return InPlaceMergeClaim::NoLiveSession;
    };
    if let Some(holder) = s.change_claim {
        return InPlaceMergeClaim::Claimed(holder);
    }
    if s.pending_stop.is_some() {
        return InPlaceMergeClaim::SessionStopping;
    }
    InPlaceMergeClaim::Claim
}

impl super::LucidosEngine {
    /// Atomically claim a thread's live session for an in-place merge.
    ///
    /// Check for a live session and claim the apply under one lock. This
    /// excludes concurrent in-place merges that could both send prompts into
    /// the same session. `Ready` carries the claim. The caller clears it on
    /// completion.
    pub(crate) async fn begin_in_place_merge(&self, thread_id: Uuid) -> InPlaceMergeStart {
        let mut guard = self.agent_sessions.lock().await;
        match decide_in_place_merge_claim(guard.get(&thread_id)) {
            InPlaceMergeClaim::NoLiveSession => InPlaceMergeStart::NoLiveSession,
            InPlaceMergeClaim::Claimed(holder) => InPlaceMergeStart::Claimed(holder),
            InPlaceMergeClaim::SessionStopping => InPlaceMergeStart::SessionStopping,
            InPlaceMergeClaim::Claim => {
                // Re-fetch mutably to set the claim flag; presence + worktree
                // were validated by `decide_in_place_merge_claim` above under the
                // same lock, so the unwraps cannot fire.
                let s = guard
                    .get_mut(&thread_id)
                    .expect("session present (checked under lock)");
                s.change_claim = Some(crate::engine::types::ChangeClaim::Apply);
                InPlaceMergeStart::Ready(crate::engine::change_ops::LiveSessionInfo {
                    worktree_path: s
                        .worktree_path
                        .clone()
                        .expect("worktree present (checked under lock)"),
                    msg_tx: s.msg_tx.clone(),
                    last_event_at: s.last_event_at.clone(),
                })
            }
        }
    }
}

pub async fn emit_apply_now_ended(
    event_bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    outcome: super::thread_events::ApplyNowOutcome,
) -> Option<i64> {
    match event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ApplyNowEnded { outcome },
            meta: crate::engine::thread_events::EventMeta::NONE,
        })
        .await
    {
        Ok(emitted) => emitted.and_then(|r| r.summary_version),
        Err(e) => {
            log_legacy!(
                "[ApplyNow] ApplyNowEnded emit failed for {}: {}",
                thread_id,
                e
            );
            None
        }
    }
}

/// End every Apply Now a stop cut off. The apply task dies with the engine. So
/// an apply still in flight at startup has nobody left to end it, and the
/// thread would read as applying for good (ADR 0437). A failed read leaves the
/// flags to the next startup.
pub async fn end_interrupted_applies(
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
) {
    let interrupted = sqlx::query_scalar::<_, Uuid>(
        "SELECT thread_id FROM thread_summaries WHERE apply_in_flight",
    )
    .fetch_all(pool)
    .await;
    match interrupted {
        Ok(threads) => {
            for thread_id in threads {
                emit_apply_now_ended(
                    event_bus,
                    thread_id,
                    super::thread_events::ApplyNowOutcome::Interrupted,
                )
                .await;
            }
        }
        Err(e) => log_legacy!("[Startup] Reading in-flight applies failed: {}", e),
    }
}
