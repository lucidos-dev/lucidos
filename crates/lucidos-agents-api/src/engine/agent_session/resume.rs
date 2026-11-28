use crate::engine::session_queries::{
    default_claude_config_dir, live_claude_config_dir, lookup_session_branch_for_thread,
    AFTER_LAST_STALE_RESUME_SQL,
};
pub(crate) use crate::engine::session_queries::{
    deterministic_worktree_path, lookup_latest_cc_session_id, lookup_latest_session_id_for_agent,
    lookup_latest_worktree_path, resolve_worktree_path,
};

pub(crate) const CC_TURN_CLOSER_EVENTS: &str =
    "'CodingAgentIdled', 'SessionEnded', 'ChangeApplied', 'ChangeDiscarded'";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdleAnchor {
    Found(String),
    Absent,
    Unknown,
}

impl IdleAnchor {
    pub(crate) fn found(&self) -> Option<&str> {
        match self {
            Self::Found(sha) => Some(sha.as_str()),
            Self::Absent | Self::Unknown => None,
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RunKind {
    Ordinary,
    Lane,
    Resolution,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RunResume {
    pub pending_handoff: Option<crate::engine::pending_handoff::PendingHandoff>,
    pub blocking_policy: Option<&'static str>,
    pub session_id: Option<String>,
    pub branch: Option<String>,
}

impl RunResume {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedAccountPin {
    pub dir: String,
    pub explicit: Option<bool>,
}

impl RecordedAccountPin {
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
