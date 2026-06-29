use crate::engine::git_ops::{find_worktree_for_branch, worktrees_dir};
use std::path::{Path, PathBuf};

/// Lifecycle events that close out a CC turn.
pub(crate) const CC_TURN_CLOSER_EVENTS: &str =
    "'CodingAgentIdled', 'SessionEnded', 'ChangeApplied', 'ChangeDiscarded'";

/// Event types that can be the originating event of a chat-channel turn —
/// the event whose id every event in the turn carries as `request_event_id`.
/// `ChildThreadCompleted` is included because a parent thread waking from a
/// finished child has the CTC's id stamped as `pre_emitted_origin` (see
/// `engine_impl::notify_parent_of_child_completion` → `process.rs`'s
/// `origin_id`). Used by every chat-side path that resolves "which turn was
/// being killed" — `abort_in_flight_for_restart`, `emit_stuck_thread_eviction_abort`,
/// the `chat/rerun.rs` Continue fallback, and the `recover_orphaned_threads`
/// SQL.
pub(crate) const CHAT_ORIGINATING_EVENT_TYPES: &[&str] =
    &["MessageReceived", "TriggerStarted", "ChildThreadCompleted"];

/// Event types that can be the originating event of a CC-channel turn.
/// Mirror of `CHAT_ORIGINATING_EVENT_TYPES` plus `CodingAgentUserMessageSent`
/// — CC follow-ups within an existing session anchor on the CCUMS injected
/// into the subprocess. `ChildThreadCompleted` is included because a CC
/// parent waking from a finished child also has the CTC's id stamped as
/// `pre_emitted_origin` (the wake routes through the same shared
/// `chat::process_message_with_steps` and `AgentUserInput { origin_event_id }`
/// flows into `run_session`'s `EventMeta { request_event_id: Some(origin_id) }`).
pub(crate) const CC_ORIGINATING_EVENT_TYPES: &[&str] = &[
    "MessageReceived",
    "CodingAgentUserMessageSent",
    "TriggerStarted",
    "ChildThreadCompleted",
];

/// Length of the `thread_id` prefix used to build deterministic worktree
/// directory names (e.g. `thread-1a2b3c4d`). 8 hex chars ~= 4 billion
/// combinations, far in excess of plausible per-workspace thread counts;
/// the full `thread_id` lives in the `CodingAgentIdled` payload for
/// unambiguous lookup.
pub(crate) const THREAD_WORKTREE_ID_LEN: usize = 8;

/// Generate the deterministic per-thread worktree directory:
/// `<workspace>/.lucidos/worktrees/thread-<short_thread_id>`.
///
/// Phase 6.1 of the CC resume architecture: every thread owns one persistent
/// worktree, so the path is derived from the thread id rather than a random
/// per-spawn UUID. The 8-char prefix is for readability — collision avoidance
/// is the responsibility of the per-workspace scope (the namespace is
/// effectively `(workspace, thread)`), and the full `thread_id` is recorded
/// in `CodingAgentIdled.worktree_path` so lookups are unambiguous.
pub(crate) fn deterministic_worktree_path(
    workspace_path: &Path,
    thread_id: uuid::Uuid,
) -> PathBuf {
    let id_str = thread_id.simple().to_string();
    let short = &id_str[..THREAD_WORKTREE_ID_LEN.min(id_str.len())];
    worktrees_dir(workspace_path).join(format!("thread-{}", short))
}

/// Look up the most-recent `CodingAgentIdled` event for a thread and return
/// its `worktree_head_sha` payload field. Returns `None` for legacy events
/// that predate the field (Phase 8.1), for the truly-first turn of a thread,
/// and for idles emitted without a worktree.
///
/// Used by the spawn path to detect external user edits made between turns
/// (see `external_edits::compute_external_edit_note`).
pub(crate) async fn lookup_latest_worktree_head_sha(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT payload->>'worktree_head_sha' FROM events \
         WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
           AND payload->>'worktree_head_sha' IS NOT NULL \
           AND payload->>'worktree_head_sha' != '' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log!(
            "[AgentSession] Failed to look up latest worktree_head_sha for {}: {}",
            thread_id,
            e
        );
        e
    })
    .ok()
    .flatten()
    .flatten()
    .filter(|s| !s.is_empty())
}

/// Look up the most-recent `CodingAgentIdled` event for a thread and return
/// its `worktree_path` payload field. Returns `None` for legacy events that
/// predate the field, for the truly-first turn of a thread, and for idles
/// emitted without a worktree (recovery's "no branch" path).
pub(crate) async fn lookup_latest_worktree_path(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Option<PathBuf> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT payload->>'worktree_path' FROM events \
         WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
           AND payload->>'worktree_path' IS NOT NULL \
           AND payload->>'worktree_path' != '' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log!(
            "[AgentSession] Failed to look up latest worktree_path for {}: {}",
            thread_id,
            e
        );
        e
    })
    .ok()
    .flatten()
    .flatten()
    .filter(|s| !s.is_empty())
    .map(PathBuf::from)
}

/// Resolve the worktree path for the next CC spawn on `thread_id`.
///
/// Resolution order:
/// 1. The most-recent `CodingAgentIdled` event's `worktree_path` payload
///    field. This is the fast, deterministic path: every CC turn after
///    Phase 6.1 stamps it.
/// 2. `git worktree list --porcelain` filtered to the thread's branch
///    (if known). Covers legacy threads whose `CodingAgentIdled` events
///    predate the `worktree_path` field but whose worktree still exists
///    on disk.
/// 3. The new deterministic path
///    `<workspace>/.lucidos/worktrees/thread-<short>`. Used for the
///    truly-first turn of a thread and for legacy threads whose worktree
///    has already been cleaned up.
///
/// The returned path is not guaranteed to exist on disk — callers must
/// check and create the worktree when missing (see `run_session.rs`).
pub(crate) async fn resolve_worktree_path(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    workspace_path: &Path,
    repo_root: &Path,
    branch_hint: Option<&str>,
) -> PathBuf {
    if let Some(path) = lookup_latest_worktree_path(pool, thread_id).await {
        log!(
            "[AgentSession] Resolved worktree path from CodingAgentIdled event for thread {}: {}",
            thread_id,
            path.display()
        );
        return path;
    }

    if let Some(branch) = branch_hint {
        if let Some(path) = find_worktree_for_branch(repo_root, branch).await {
            log!(
                "[AgentSession] Resolved worktree path via git worktree list for thread {} branch {}: {}",
                thread_id,
                branch,
                path.display()
            );
            return path;
        }
    }

    let path = deterministic_worktree_path(workspace_path, thread_id);
    log!(
        "[AgentSession] Generated new deterministic worktree path for thread {}: {}",
        thread_id,
        path.display()
    );
    path
}

/// Resolve `(resume_session_id, resume_branch)` for a follow-up CC request.
/// Priority: pending-change branch > caller-supplied session > most recent
/// `CodingAgentIdled` > fresh start. Pending-change branch wins because the
/// change-proposal flow removes the worktree but keeps the branch's commits.
pub(super) async fn resolve_resume_context(
    pool: &sqlx::PgPool,
    changes: &crate::core::changes_projection::ChangesProjection,
    thread_id: uuid::Uuid,
    caller_session_id: Option<String>,
) -> (Option<String>, Option<String>) {
    let pending_branch = changes
        .pending_for_thread(thread_id)
        .await
        .unwrap_or_else(|e| {
            log!(
                "[AgentSession] resume: pending_for_thread({}): {} — \
                 skipping pending-branch resume path",
                thread_id,
                e
            );
            Vec::new()
        })
        .pop()
        .map(|c| c.branch_name);

    if let Some(branch) = pending_branch {
        let resume_sid = match caller_session_id {
            Some(sid) => Some(sid),
            None => lookup_latest_cc_session_id(pool, thread_id).await,
        };
        log!(
            "[AgentSession] Resuming on pending-change branch {} for thread {} (sid={:?})",
            branch,
            thread_id,
            resume_sid
        );
        return (resume_sid, Some(branch));
    }

    if caller_session_id.is_some() {
        let branch = lookup_session_branch_for_thread(pool, thread_id).await;
        return (caller_session_id, branch);
    }

    let query = format!(
        "SELECT event_type, payload->>'cc_session_id' FROM events \
         WHERE thread_id = $1 AND event_type IN ({}) \
         ORDER BY sequence DESC LIMIT 1",
        CC_TURN_CLOSER_EVENTS,
    );
    let last_lifecycle = sqlx::query_as::<_, (String, Option<String>)>(&query)
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            log!(
                "[AgentSession] Failed to look up last lifecycle event for resume: {}",
                e
            );
            None
        });

    if let Some((event_type, sid)) = last_lifecycle.as_ref() {
        if event_type == "CodingAgentIdled" {
            let resume_sid = sid.clone().filter(|s| !s.is_empty());
            if resume_sid.is_some() {
                let branch = lookup_session_branch_for_thread(pool, thread_id).await;
                return (resume_sid, branch);
            }
        }
    }

    (None, None)
}

async fn lookup_session_branch_for_thread(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT payload->>'branch' FROM events \
         WHERE thread_id = $1 AND event_type = 'SessionStarted' \
           AND payload->>'branch' IS NOT NULL AND payload->>'branch' != '' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log!(
            "[AgentSession] Failed to look up session branch for {}: {}",
            thread_id,
            e
        );
        e
    })
    .ok()
    .flatten()
}

/// `(repo_id, branch)` from the most recent `SessionStarted` event for the
/// thread that recorded both. Pre-Mar-2026 SessionStarted payloads sometimes
/// omitted `repo_id`; the filter skips those rather than returning a partial
/// result, since both fields are needed to recover a diff.
pub(crate) async fn lookup_latest_session_repo_and_branch(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Option<(String, String)> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT payload->>'repo_id', payload->>'branch' FROM events \
         WHERE thread_id = $1 AND event_type = 'SessionStarted' \
           AND payload->>'repo_id' IS NOT NULL AND payload->>'repo_id' <> '' \
           AND payload->>'branch' IS NOT NULL AND payload->>'branch' <> '' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log!(
            "[AgentSession] Failed to look up session repo+branch for {}: {}",
            thread_id,
            e
        );
        e
    })
    .ok()
    .flatten()
}

/// Look up the most recent "originating event" id for a thread — the event
/// whose id every event in the latest turn carries as `request_event_id`.
/// Used by abort/recovery paths to stamp `request_event_id` on
/// ResponseAborted so the rerun and frontend exchange grouping can find the
/// right turn to anchor.
///
/// Callers pass the canonical list for their channel:
/// - Chat / trigger: `CHAT_ORIGINATING_EVENT_TYPES` (MR / TriggerStarted / CTC)
/// - CC: `CC_ORIGINATING_EVENT_TYPES` (chat list + CodingAgentUserMessageSent)
///
/// Both lists include `ChildThreadCompleted` because either agent can wake
/// from a finished child — see the constants' docs.
pub(crate) async fn latest_originating_event_id(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    event_types: &[&str],
) -> Option<uuid::Uuid> {
    let placeholders: Vec<String> = (2..=event_types.len() + 1).map(|i| format!("${}", i)).collect();
    let q = format!(
        "SELECT id FROM events \
         WHERE aggregate_id = $1 \
           AND event_type IN ({}) \
         ORDER BY sequence DESC LIMIT 1",
        placeholders.join(",")
    );
    let mut query = sqlx::query_scalar::<_, uuid::Uuid>(&q).bind(thread_id.to_string());
    for et in event_types {
        query = query.bind(*et);
    }
    query
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            log!(
                "[AgentSession] Failed to look up latest originating event id for {}: {}",
                thread_id,
                e
            );
            e
        })
        .ok()
        .flatten()
}

/// Return the most-recent CC session id recorded for a thread, so the spawn /
/// recovery paths can `--resume` the existing conversation instead of starting
/// fresh.
///
/// The id lands in two event types: `CodingAgentSettingsChanged` (emitted once
/// at CC `Init`, the earliest point it's known) and `CodingAgentIdled` (emitted
/// at every turn boundary). Reading across BOTH — newest non-null wins — is what
/// makes a turn interrupted by an engine restart *before* its first idle still
/// resumable; the `Init` settings event is the only carrier in that window. The
/// `IS NOT NULL` filter also stops a recovery-emitted idle that carries no id
/// (e.g. `engine_restart_interrupt` from a path that couldn't resolve it) from
/// shadowing an earlier known id.
pub(crate) async fn lookup_latest_cc_session_id(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT payload->>'cc_session_id' FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('CodingAgentIdled', 'CodingAgentSettingsChanged') \
           AND payload->>'cc_session_id' IS NOT NULL \
           AND payload->>'cc_session_id' <> '' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log!(
            "[AgentSession] Failed to look up latest cc_session_id for {}: {}",
            thread_id,
            e
        );
        e
    })
    .ok()
    .flatten()
    .flatten()
    .filter(|s| !s.is_empty())
}

/// Get a fallback description for a change proposal: thread title if available, else branch name.
pub(crate) async fn change_description_fallback(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    branch_name: &str,
) -> String {
    let title: Option<String> =
        match sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                log!(
                    "[AgentSession] Failed to fetch thread title for change description: {}",
                    e
                );
                None
            }
        };

    match title {
        Some(t) if !t.is_empty() => t,
        _ => branch_name.to_string(),
    }
}

#[cfg(test)]
#[path = "resume_tests.rs"]
mod tests;
