use crate::engine::session_queries::{
    default_claude_config_dir, live_claude_config_dir, lookup_session_branch_for_thread,
    AFTER_LAST_STALE_RESUME_SQL,
};
pub(crate) use crate::engine::session_queries::{
    deterministic_worktree_path, lookup_latest_cc_session_id, lookup_latest_session_id_for_agent,
    lookup_latest_worktree_path, resolve_worktree_path,
};

/// Lifecycle events that close out a CC turn.
pub(crate) const CC_TURN_CLOSER_EVENTS: &str =
    "'CodingAgentIdled', 'SessionEnded', 'ChangeApplied', 'ChangeDiscarded'";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdleAnchor {
    /// An idle recorded this sha.
    Found(String),
    /// The query answered, and the thread has no idle carrying a sha.
    Absent,
    /// The query failed. Nothing is known about the anchor.
    Unknown,
}

impl IdleAnchor {
    /// The sha when there is one. `Absent` and `Unknown` collapse here, which
    /// suits every caller that only skips work when no sha is available.
    pub(crate) fn found(&self) -> Option<&str> {
        match self {
            Self::Found(sha) => Some(sha.as_str()),
            Self::Absent | Self::Unknown => None,
        }
    }
}

/// Look up the most-recent `CodingAgentIdled` event for a thread and return
/// its `worktree_head_sha` payload field. Answers `Absent` for legacy events
/// that predate the field (Phase 8.1), for the truly-first turn of a thread,
/// and for idles emitted without a worktree.
///
/// Used by the spawn path to detect external user edits made between turns
/// (see `external_edits::compute_external_edit_note`) and to anchor
/// `external_edits::try_adopt_renegade_branch`.
pub(crate) async fn lookup_latest_worktree_head_sha(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> IdleAnchor {
    match sqlx::query_scalar::<_, Option<String>>(
        "SELECT payload->>'worktree_head_sha' FROM events \
         WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
           AND payload->>'worktree_head_sha' IS NOT NULL \
           AND payload->>'worktree_head_sha' != '' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    {
        Ok(row) => match row.flatten().filter(|s| !s.is_empty()) {
            Some(sha) => IdleAnchor::Found(sha),
            None => IdleAnchor::Absent,
        },
        Err(e) => {
            log_legacy!(
                "[AgentSession] Failed to look up latest worktree_head_sha for {}: {}",
                thread_id,
                e
            );
            IdleAnchor::Unknown
        }
    }
}

pub(crate) async fn lookup_prior_change_flags(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> (bool, bool) {
    let has_changes = sqlx::query_scalar::<_, bool>(
        "SELECT coding_agent_change_state <> $2 FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .bind(crate::engine::thread_lifecycle::ChangeStateKind::None)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        log_legacy!(
            "[AgentSession] Failed to read the prior branch work for {}: {}",
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
            log_legacy!(
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

pub(crate) async fn resume_sid_for_account(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    pinned_config_dir: Option<&str>,
) -> Option<String> {
    match pinned_config_dir {
        Some(dir) => lookup_latest_cc_session_id_for_config_dir(pool, thread_id, dir).await,
        None => lookup_latest_cc_session_id(pool, thread_id).await,
    }
}

pub(super) async fn resolve_resume_context(
    pool: &sqlx::PgPool,
    changes: &crate::core::changes_projection::ChangesProjection,
    thread_id: uuid::Uuid,
    caller_session_id: Option<String>,
    pinned_config_dir: Option<&str>,
    start_fresh: bool,
) -> (Option<String>, Option<String>) {
    let open_branch = changes
        .open_for_thread(thread_id)
        .await
        .unwrap_or_else(|e| {
            log_legacy!(
                "[AgentSession] resume: open_for_thread({}): {}; \
                 skipping the open-branch resume path",
                thread_id,
                e
            );
            Vec::new()
        })
        .pop()
        .map(|c| c.branch_name);

    // A fresh session runs on the thread's own branch.
    if start_fresh {
        let branch = match open_branch {
            Some(branch) => Some(branch),
            None => lookup_session_branch_for_thread(pool, thread_id).await,
        };
        log_legacy!(
            "[AgentSession] Thread {} starts a fresh session on branch {:?}",
            thread_id,
            branch
        );
        return (None, branch);
    }

    if let Some(branch) = open_branch {
        let resume_sid = match caller_session_id {
            Some(sid) => Some(sid),
            None => resume_sid_for_account(pool, thread_id, pinned_config_dir).await,
        };
        log_legacy!(
            "[AgentSession] Resuming on open-change branch {} for thread {} (sid={:?})",
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
            log_legacy!(
                "[AgentSession] Failed to look up last lifecycle event for resume: {}",
                e
            );
            None
        });

    if let Some((event_type, sid)) = last_lifecycle.as_ref() {
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

/// Which session a run starts, for the handoff and resume decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RunKind {
    /// A first message, a follow-up or a Continue: the thread's own next session.
    Ordinary,
    /// A lane thread resolving its open batch. Its sessions are the thread's
    /// own, so a pending handoff starts its successor session.
    Lane,
    Resolution,
}

/// What a run resumes, and the handoff or provider block its session starts
/// from.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RunResume {
    pub pending_handoff: Option<crate::engine::pending_handoff::PendingHandoff>,
    /// The policy that blocked an ordinary run's newest session
    /// ([`blocking_policy`]). A pending handoff already starts fresh.
    pub blocking_policy: Option<&'static str>,
    pub session_id: Option<String>,
    /// The branch an ordinary run's worktree resumes on.
    pub branch: Option<String>,
}

impl RunResume {
    /// Whether the run's `SessionStarted` records its continuity whatever
    /// its spawn did. A resumed spawn always records it.
    pub(super) fn records_continuity(&self, kind: RunKind) -> bool {
        kind == RunKind::Ordinary || self.pending_handoff.is_some()
    }
}

pub(super) async fn resolve_run_resume(
    pool: &sqlx::PgPool,
    changes: &crate::core::changes_projection::ChangesProjection,
    thread_id: uuid::Uuid,
    kind: RunKind,
    caller_session_id: Option<String>,
    pinned_config_dir: Option<&str>,
) -> Result<RunResume, sqlx::Error> {
    let pending_handoff = if kind == RunKind::Resolution {
        None
    } else {
        let claim = crate::engine::successor::lock_handoff(pool, thread_id).await?;
        let pending = crate::engine::pending_handoff::pending_handoff(pool, thread_id).await?;
        claim.commit().await?;
        pending
    };
    let blocking_policy = if kind == RunKind::Ordinary && pending_handoff.is_none() {
        blocking_policy(pool, thread_id).await
    } else {
        None
    };
    let (session_id, branch) = match kind {
        RunKind::Ordinary => {
            resolve_resume_context(
                pool,
                changes,
                thread_id,
                caller_session_id,
                pinned_config_dir,
                pending_handoff.is_some() || blocking_policy.is_some(),
            )
            .await
        }
        // A lane thread's successor session starts fresh in the lane's own
        // worktree. Without a handoff it keeps its context.
        RunKind::Lane if pending_handoff.is_some() => (None, None),
        RunKind::Lane | RunKind::Resolution => (caller_session_id, None),
    };
    Ok(RunResume {
        pending_handoff,
        blocking_policy,
        session_id,
        branch,
    })
}

/// The reason OpenAI's safety systems blocked the thread's newest session, when
/// the newest provider error since that session started was a policy block.
/// A later failure with no provider error, such as a spawn that failed before
/// its session began, leaves the block in place. The session that carried the
/// block is never resumed: it would replay the history that tripped it. A newer
/// `SessionStarted` clears the block, since its own id is the one to resume. A
/// failed read answers `None`, which resumes as before.
pub(crate) async fn blocking_policy(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Option<&'static str> {
    let provider_error = sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "SELECT payload->'provider_error' FROM events \
         WHERE thread_id = $1 AND event_type = 'ResponseFailed' \
           AND jsonb_typeof(payload->'provider_error') = 'object' \
           AND sequence > COALESCE(( \
                 SELECT MAX(sequence) FROM events \
                 WHERE thread_id = $1 AND event_type = 'SessionStarted'), 0) \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        log_legacy!(
            "[AgentSession] Failed to look up a provider block for {}: {}",
            thread_id,
            e
        );
        None
    })
    .flatten()?;
    serde_json::from_value::<crate::engine::thread_events::ProviderError>(provider_error)
        .ok()?
        .policy_name()
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
        log_legacy!(
            "[AgentSession] Failed to look up session repo+branch for {}: {}",
            thread_id,
            e
        );
        e
    })
    .ok()
    .flatten()
}

/// Event types that reset the transient-API-drop auto-resume budget. Either one
/// means the thread made real progress since the last drop: the user asked for
/// something new, or a turn actually completed. Kept as one SQL fragment so
/// [`api_error_auto_resumes_spent`]'s two halves cannot drift.
const API_ERROR_BUDGET_RESET_EVENTS: &str = "'MessageReceived', 'ResponseGenerated'";

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
            log_legacy!(
                "[AgentSession] Failed to count api-error auto-resumes for {}: {}. Treating the budget as spent.",
                thread_id,
                e
            );
            crate::engine::agent_session::lifecycle::MAX_API_ERROR_AUTO_RESUMES
        }
    }
}

/// A thread's account pin as its first session recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedAccountPin {
    /// Where the transcripts live. Session ids are scoped by this path.
    pub dir: String,
    /// Whether `CLAUDE_CONFIG_DIR` was set. `None` on rows from before it was
    /// recorded.
    pub explicit: Option<bool>,
}

impl RecordedAccountPin {
    /// The pin to replay, given the variables this spawn injects.
    pub(crate) fn resolve(&self, user_env_vars: &[(String, String)]) -> crate::runtime::AccountPin {
        crate::runtime::AccountPin::from_recorded(
            self.dir.clone(),
            self.explicit,
            live_claude_config_dir(user_env_vars).as_deref(),
            default_claude_config_dir().as_deref(),
        )
    }
}

pub(crate) async fn lookup_pinned_cc_config_dir(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Result<Option<RecordedAccountPin>, sqlx::Error> {
    let pinned = sqlx::query_as::<_, (String, Option<bool>)>(
        "SELECT payload->>'claude_config_dir', \
                (payload->>'claude_config_dir_explicit')::boolean FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('CodingAgentIdled', 'CodingAgentSettingsChanged') \
           AND payload->>'claude_config_dir' IS NOT NULL \
           AND payload->>'claude_config_dir' <> '' \
         ORDER BY sequence ASC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(pinned.map(|(dir, explicit)| RecordedAccountPin { dir, explicit }))
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
    sqlx::query_scalar::<_, Option<String>>(&format!(
        "SELECT payload->>'cc_session_id' FROM events \
         WHERE thread_id = $1 \
           AND event_type = 'CodingAgentSettingsChanged' \
           AND payload->>'claude_config_dir' = $2 \
           AND payload->>'cc_session_id' IS NOT NULL \
           AND payload->>'cc_session_id' <> '' \
           AND {AFTER_LAST_STALE_RESUME_SQL} \
         ORDER BY sequence DESC LIMIT 1"
    ))
    .bind(thread_id)
    .bind(config_dir)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log_legacy!(
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

#[cfg(test)]
#[path = "resume_tests.rs"]
mod tests;
