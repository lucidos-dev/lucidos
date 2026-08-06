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
pub(crate) fn deterministic_worktree_path(workspace_path: &Path, thread_id: uuid::Uuid) -> PathBuf {
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

/// The thread's last known `(has_changes, requires_restart)` coding-agent
/// change state. This is what an idle preserves when its diff probe could not
/// be answered: an unanswered probe is UNKNOWN, and writing `false` there is
/// what left external-repo threads with a dark Diff button after their branch
/// was renamed mid-session.
///
/// Each half is read from the surface that owns it.
///
/// `has_changes` comes from `thread_summaries.coding_agent_has_diff`, the exact
/// column the `CodingAgentIdled` projection writes, so preserving it makes the
/// emit a no-op instead of a downgrade. It is also the only source that is
/// right on a **first** turn, because the worktree's post-commit hook
/// (`coding_agent_diff_refresh`) has already corrected it mid-turn from the
/// branch the worktree is really on, and there is no previous idle event to
/// read. An apply or discard resets the column, so a stale `true` cannot
/// outlive the change it described.
///
/// `requires_restart` is NOT projected (the `CodingAgentIdled` arm leaves that
/// column to `ChangeProposed`), so it comes from the most recent CC turn-closer
/// payload. Both default to `false` when nothing is recorded yet.
pub(crate) async fn lookup_prior_change_flags(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> (bool, bool) {
    let has_changes = sqlx::query_scalar::<_, bool>(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        log!(
            "[AgentSession] Failed to read prior coding_agent_has_diff for {}: {}",
            thread_id,
            e
        );
        None
    })
    .unwrap_or(false);

    let query = format!(
        "SELECT payload FROM events \
         WHERE aggregate_id = $1 AND event_type IN ({}) \
         ORDER BY created DESC LIMIT 1",
        CC_TURN_CLOSER_EVENTS,
    );
    let requires_restart = sqlx::query_scalar::<_, serde_json::Value>(&query)
        .bind(thread_id.to_string())
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            log!(
                "[AgentSession] Failed to read prior requires_restart for {}: {}",
                thread_id,
                e
            );
            None
        })
        .and_then(|payload| payload.get("requires_restart").and_then(|v| v.as_bool()))
        .unwrap_or(false);

    (has_changes, requires_restart)
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

/// The resume session id for a follow-up, scoped to the thread's pinned account:
/// the newest session under `pinned_config_dir` when the thread has one, else the
/// globally-newest session (a legacy thread with no recorded config dir). Scoping
/// is what stops a resume from targeting a session created under a *different*
/// account after a mid-thread toggle flip.
async fn resume_sid_for_account(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    pinned_config_dir: Option<&str>,
) -> Option<String> {
    match pinned_config_dir {
        Some(dir) => lookup_latest_cc_session_id_for_config_dir(pool, thread_id, dir).await,
        None => lookup_latest_cc_session_id(pool, thread_id).await,
    }
}

/// Resolve `(resume_session_id, resume_branch)` for a follow-up CC request.
/// Priority: pending-change branch > caller-supplied session > most recent
/// `CodingAgentIdled` > fresh start. Pending-change branch wins because the
/// change-proposal flow removes the worktree but keeps the branch's commits.
///
/// `pinned_config_dir` is the thread's account pin (its first config dir): the
/// auto-detected resume session id is resolved WITHIN that account, so a thread
/// that was mis-pinned onto another account by a toggle flip still resumes the
/// session belonging to its original account rather than the globally-newest one.
pub(super) async fn resolve_resume_context(
    pool: &sqlx::PgPool,
    changes: &crate::core::changes_projection::ChangesProjection,
    thread_id: uuid::Uuid,
    caller_session_id: Option<String>,
    pinned_config_dir: Option<&str>,
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
            None => resume_sid_for_account(pool, thread_id, pinned_config_dir).await,
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
        // A resumable idle is the newest lifecycle event (a StaleResume /
        // SessionEnded / change-apply shadow has a different event_type and falls
        // through to a fresh spawn). Resolve the sid WITHIN the pinned account,
        // which can differ from this globally-newest idle if the thread was
        // mis-pinned onto another account by a mid-thread toggle flip.
        if event_type == "CodingAgentIdled" && sid.as_ref().is_some_and(|s| !s.is_empty()) {
            let resume_sid = resume_sid_for_account(pool, thread_id, pinned_config_dir).await;
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
    let placeholders: Vec<String> = (2..=event_types.len() + 1)
        .map(|i| format!("${}", i))
        .collect();
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

/// Event types that reset the transient-API-drop auto-resume budget. Either one
/// means the thread made real progress since the last drop: the user asked for
/// something new, or a turn actually completed. Kept as one SQL fragment so
/// [`api_error_auto_resumes_spent`]'s two halves cannot drift.
const API_ERROR_BUDGET_RESET_EVENTS: &str = "'MessageReceived', 'ResponseGenerated'";

/// How many `ContinuationRequested{auto_resume_after_api_error}` this thread has
/// already spent CONSECUTIVELY, i.e. since its last user message or completed
/// turn. Compared against `lifecycle::MAX_API_ERROR_AUTO_RESUMES` by
/// `auto_resume_after_api_error`.
///
/// The count lives in the events table rather than on the session because each
/// auto-resume tears the old session down and the continuation spawns a NEW one:
/// an in-memory counter would reset itself on exactly the transition it is meant
/// to count, and the loop would be unbounded. Event-sourced also means it
/// survives an engine restart mid-storm.
///
/// A query error yields `MAX_API_ERROR_AUTO_RESUMES`, i.e. "budget spent". An
/// unknown count must not authorize another spawn: the cost of being wrong that
/// way is an unbounded resume loop against a broken upstream, while the cost of
/// the other way is one red dot the user can clear with Continue.
pub(crate) async fn api_error_auto_resumes_spent(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    reason: &str,
) -> i64 {
    let q = format!(
        "SELECT COUNT(*) FROM events \
         WHERE thread_id = $1 \
           AND event_type = 'ContinuationRequested' \
           AND payload->>'reason' = $2 \
           AND sequence > COALESCE(( \
                 SELECT MAX(sequence) FROM events \
                 WHERE thread_id = $1 AND event_type IN ({}) \
               ), 0)",
        API_ERROR_BUDGET_RESET_EVENTS,
    );
    match sqlx::query_scalar::<_, i64>(&q)
        .bind(thread_id)
        .bind(reason)
        .fetch_one(pool)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            log!(
                "[AgentSession] Failed to count api-error auto-resumes for {}: {}. Treating the budget as spent.",
                thread_id,
                e
            );
            crate::engine::agent_session::lifecycle::MAX_API_ERROR_AUTO_RESUMES
        }
    }
}

/// Claude Code's default config dir when `CLAUDE_CONFIG_DIR` is unset: `$HOME/.claude`.
///
/// Used to record the EFFECTIVE config dir of a fresh session that ran without an
/// explicit `CLAUDE_CONFIG_DIR`, so a later resume can still pin the right dir even
/// after the user sets one (the unset→set switch). If CC's default ever diverges
/// from `~/.claude` a resume would pin a wrong dir and fail — but the explicit
/// "No conversation found" recovery (`is_definitive_session_not_found`) backstops
/// that by falling back to a fresh session. `None` only when `$HOME` is unset.
pub(crate) fn default_claude_config_dir() -> Option<String> {
    std::env::var_os("HOME").map(|home| {
        std::path::Path::new(&home)
            .join(".claude")
            .to_string_lossy()
            .into_owned()
    })
}

/// Return the `CLAUDE_CONFIG_DIR` (provider/account) the thread's **first** CC
/// session was created under — the thread's permanent account pin. Every spawn
/// after turn 1 runs under this dir (see `run_session/run.rs`), so a thread can
/// never switch provider mid-life even if the global `CLAUDE_CONFIG_DIR` toggle
/// changes between turns. This closes the strand-on-the-wrong-account bug: a
/// post-limit fresh spawn used to re-record whatever the live toggle was and
/// leave the thread pinned to that new account. Resuming under the pinned dir
/// also keeps `--resume` pointed at the dir CC wrote the transcript to
/// (`$CLAUDE_CONFIG_DIR/projects/<cwd>/<sid>.jsonl`).
///
/// Recorded at the Init-time `CodingAgentSettingsChanged` — the authoritative
/// point where CC reports its session id AND the engine knows the dir it spawned
/// under. Read across both CC lifecycle event types (only the Init settings event
/// carries the field today, so `CodingAgentIdled` rows return null and are
/// filtered out); **earliest** non-null wins. `None` for a thread that has not
/// recorded a dir yet — turn 1 then reads the live env, which is exactly how the
/// pin gets established.
pub(crate) async fn lookup_pinned_cc_config_dir(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT payload->>'claude_config_dir' FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('CodingAgentIdled', 'CodingAgentSettingsChanged') \
           AND payload->>'claude_config_dir' IS NOT NULL \
           AND payload->>'claude_config_dir' <> '' \
         ORDER BY sequence ASC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log!(
            "[AgentSession] Failed to look up pinned claude_config_dir for {}: {}",
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

/// The newest `cc_session_id` recorded UNDER `config_dir` — the resume target
/// scoped to the thread's pinned account. The `cc_session_id`↔`claude_config_dir`
/// pairing lives only in the Init-time `CodingAgentSettingsChanged` (the sole
/// event carrying both), so scope to that type. `None` when the pinned account
/// has no recorded session — the caller then spawns fresh under the pinned dir
/// rather than resuming a session that belongs to a different account.
pub(crate) async fn lookup_latest_cc_session_id_for_config_dir(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    config_dir: &str,
) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT payload->>'cc_session_id' FROM events \
         WHERE thread_id = $1 \
           AND event_type = 'CodingAgentSettingsChanged' \
           AND payload->>'claude_config_dir' = $2 \
           AND payload->>'cc_session_id' IS NOT NULL \
           AND payload->>'cc_session_id' <> '' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(config_dir)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log!(
            "[AgentSession] Failed to look up cc_session_id under {} for {}: {}",
            config_dir,
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
