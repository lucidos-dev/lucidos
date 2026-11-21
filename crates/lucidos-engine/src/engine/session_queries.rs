//! Worktree and resume lookups shared with the engine.

use std::path::{Path, PathBuf};

use super::git_ops::{find_worktree_for_branch, short_thread_id, worktrees_dir, WorktreeLookup};

/// Keeps a session-id lookup (bound to `thread_id = $1`) to events after the
/// thread's newest `SessionEnded { StaleResume }`. That event proves every
/// earlier id unresumable, and the fresh retry records its own id after it.
/// Without the filter, a retry that auto-detects its sid picks the dead one
/// again and fails the same way.
pub const AFTER_LAST_STALE_RESUME_SQL: &str = "sequence > COALESCE(( \
     SELECT MAX(sequence) FROM events \
     WHERE thread_id = $1 AND event_type = 'SessionEnded' \
       AND payload->>'reason' = 'stale_resume'), 0)";

/// Generate the deterministic per-thread worktree directory:
/// `<workspace>/.lucidos/worktrees/thread-<short_thread_id>`.
///
/// Phase 6.1 of the CC resume architecture: every thread owns one persistent
/// worktree, so the path is derived from the thread id rather than a random
/// per-spawn UUID. The short prefix is for readability, and the thread's
/// branch carries the same one. Collisions are avoided by the per-workspace
/// scope (the namespace is effectively `(workspace, thread)`), and the full
/// `thread_id` is recorded in `CodingAgentIdled.worktree_path` so lookups are
/// unambiguous.
pub fn deterministic_worktree_path(workspace_path: &Path, thread_id: uuid::Uuid) -> PathBuf {
    worktrees_dir(workspace_path).join(format!("thread-{}", short_thread_id(thread_id)))
}

/// Read the worktree path from the thread's latest `CodingAgentIdled`. First
/// turns and idle events without a recorded path return `None`.
pub async fn lookup_latest_worktree_path(
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
        log_legacy!(
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
pub async fn resolve_worktree_path(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    workspace_path: &Path,
    repo_root: &Path,
    branch_hint: Option<&str>,
) -> PathBuf {
    if let Some(path) = lookup_latest_worktree_path(pool, thread_id).await {
        log_legacy!(
            "[AgentSession] Resolved worktree path from CodingAgentIdled event for thread {}: {}",
            thread_id,
            path.display()
        );
        return path;
    }

    if let Some(branch) = branch_hint {
        match find_worktree_for_branch(repo_root, branch).await {
            WorktreeLookup::Found(path) => {
                log_legacy!(
                    "[AgentSession] Resolved worktree path via git worktree list for thread {} branch {}: {}",
                    thread_id,
                    branch,
                    path.display()
                );
                return path;
            }
            WorktreeLookup::NotFound => {}
            // Same fallback as NotFound, and the log is what tells them apart.
            // Step 3 below only computes a path, and the caller checks it on
            // disk before using it, so nothing destructive rides on this.
            WorktreeLookup::Unknown => log_legacy!(
                "[AgentSession] git worktree list gave no answer for thread {} branch {}; falling back to the deterministic path",
                thread_id,
                branch
            ),
        }
    }

    let path = deterministic_worktree_path(workspace_path, thread_id);
    log_legacy!(
        "[AgentSession] Generated new deterministic worktree path for thread {}: {}",
        thread_id,
        path.display()
    );
    path
}

/// The branch the thread's latest session ran on. `None` when no session
/// recorded one, or the lookup failed.
pub async fn lookup_session_branch_for_thread(
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
        log_legacy!(
            "[AgentSession] Failed to look up session branch for {}: {}",
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
pub async fn latest_originating_event_id(
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
            log_legacy!(
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
/// Read the newest non-null session ID from settings or idle events. Init
/// records settings before the first idle, so an interruption in that window
/// remains resumable. A recovery idle without an ID cannot mask an older known
/// ID.
///
/// An id recorded before the newest `SessionEnded { StaleResume }` is dead (see
/// [`AFTER_LAST_STALE_RESUME_SQL`]).
pub async fn lookup_latest_cc_session_id(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Option<String> {
    lookup_latest_session_id_where(pool, thread_id, None).await
}

pub async fn lookup_latest_session_id_for_agent(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    agent: crate::runtime::CodingAgent,
) -> Option<String> {
    lookup_latest_session_id_where(pool, thread_id, Some(agent)).await
}

async fn lookup_latest_session_id_where(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    agent: Option<crate::runtime::CodingAgent>,
) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(&format!(
        "SELECT payload->>'cc_session_id' FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('CodingAgentIdled', 'CodingAgentSettingsChanged') \
           AND payload->>'cc_session_id' IS NOT NULL \
           AND payload->>'cc_session_id' <> '' \
           AND ($2::text IS NULL OR COALESCE(payload->>'coding_agent', payload->>'agent', 'claude-code') = $2) \
           AND {AFTER_LAST_STALE_RESUME_SQL} \
         ORDER BY sequence DESC LIMIT 1"
    ))
    .bind(thread_id)
    .bind(agent.map(crate::runtime::CodingAgent::as_str))
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log_legacy!(
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

/// Claude Code's default config dir when `CLAUDE_CONFIG_DIR` is unset: `$HOME/.claude`.
///
/// Record the effective config directory for a fresh session without
/// `CLAUDE_CONFIG_DIR`. This keeps later resumes in the same directory if the
/// user changes their configuration. If discovery fails, recovery can start a
/// fresh session. Returns `None` when the home directory is unset.
pub fn default_claude_config_dir() -> Option<String> {
    std::env::var_os("HOME").map(|home| {
        std::path::Path::new(&home)
            .join(".claude")
            .to_string_lossy()
            .into_owned()
    })
}

/// The `CLAUDE_CONFIG_DIR` a spawn with these user variables would run under,
/// before any pin. The user-managed variable wins, then what Claude Code
/// inherits from the engine's own env. `None` means unset. An empty user value
/// still overrides the inherited one, and Claude Code reads empty as unset.
pub fn live_claude_config_dir(user_env_vars: &[(String, String)]) -> Option<String> {
    match user_env_vars.iter().find(|(k, _)| k == "CLAUDE_CONFIG_DIR") {
        Some((_, v)) => Some(v.clone()).filter(|v| !v.is_empty()),
        None => crate::core::inherited_env_var("CLAUDE_CONFIG_DIR"),
    }
}

/// Get a fallback description for a change proposal: thread title if available, else branch name.
pub async fn change_description_fallback(
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
                log_legacy!(
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
