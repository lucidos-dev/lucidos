use crate::engine::agent_session::external_edits;
use crate::engine::LucidosEngine;
use uuid::Uuid;

impl LucidosEngine {
    pub(super) async fn emit_coding_agent_idled(
        &self,
        thread_id: Uuid,
        snapshot: CodingAgentIdleSnapshot<'_>,
        meta: &crate::engine::thread_events::EventMeta,
        coding_agent: crate::runtime::CodingAgent,
    ) {
        let cc_session_id = {
            let guard = self.agent_sessions.lock().await;
            guard.get(&thread_id).and_then(|s| s.cc_session_id.clone())
        };
        // Phase 8.1: snapshot the worktree's HEAD SHA so that the next spawn
        // can detect external user edits made between turns. `git rev-parse`
        // is best-effort — failures (e.g. zero-commit branch, missing
        // worktree on disk) leave the field as `None`.
        let worktree_head_sha = match snapshot.worktree_path {
            Some(p) => external_edits::git_head_sha(p).await,
            None => None,
        };
        // Clear any permission card still dangling at this turn boundary. A live
        // card blocks the turn (so idle can't fire during a genuine wait); by
        // the time we idle the session is done and any unresolved
        // `CodingAgentPermissionRequest` is orphaned — e.g. a workflow whose
        // parallel subagent's card outlived the main turn. Without this the card
        // sits clickable on a finished thread, and a later click resurrected the
        // thread into a dead `running` (fixed alongside the non-resurrecting
        // resolution projection —
        // `docs/plans/2026-07-02-cc-permission-card-zombie-running.md`). Engine-
        // driven, so no user actor. No-op when nothing is pending.
        crate::engine::cc_permission::resolve_pending_permissions_as_session_ended(
            self.pool(),
            &self.event_bus,
            &self.pending_cc_permission,
            thread_id,
            None,
        )
        .await;
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                        has_changes: snapshot.has_changes,
                        is_external_repo: snapshot.is_external_repo,
                        requires_restart: snapshot.requires_restart,
                        cc_session_id,
                        coding_agent,
                        reason: None,
                        worktree_path: snapshot.worktree_path
                            .map(|p| p.to_string_lossy().into_owned()),
                        worktree_head_sha,
                        bg_bash_pending: snapshot.bg_bash_pending,
                    },
                    meta: meta.clone(),
                },
                "[AgentSession] CodingAgentIdled",
            )
            .await;
    }
}

/// Per-emit snapshot of the worktree/CC state that `emit_coding_agent_idled`
/// stamps onto a `CodingAgentIdled` event.
pub(super) struct CodingAgentIdleSnapshot<'a> {
    pub(super) has_changes: bool,
    pub(super) is_external_repo: bool,
    pub(super) requires_restart: bool,
    pub(super) bg_bash_pending: bool,
    pub(super) worktree_path: Option<&'a std::path::Path>,
}
