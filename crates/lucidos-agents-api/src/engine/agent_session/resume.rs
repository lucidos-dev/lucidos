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

/// Where a thread's last idle left its worktree HEAD, or why we cannot say.
///
/// `Absent` and `Unknown` are split because *branch adoption* reads them
/// differently. A verified `Absent` lets the spawn boundary fall back to a
/// weaker proof. Collapsing an unanswered query into it would let a dropped
/// Postgres connection authorize retargeting a thread. That is
/// `.claude/rules/rust.md`'s unanswered-probe rule, applied to the database.
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

/// The thread's last known `(has_changes, requires_restart)` coding-agent
/// change state. This is what an idle preserves when its diff probe could not
/// be answered: an unanswered probe is UNKNOWN, and writing `false` there is
/// what left external-repo threads with a dark Diff button after their branch
/// was renamed mid-session.
///
/// Each half is read from the surface that owns it.
///
/// `has_changes` comes from the change state holding work, which the
/// `CodingAgentIdled` projection writes, so preserving it makes the
/// emit a no-op instead of a downgrade. It is also the only source that is
/// right on a **first** turn, because the worktree's post-commit hook
/// (`coding_agent_diff_refresh`) has already corrected it mid-turn from the
/// branch the worktree is really on, and there is no previous idle event to
/// read. An apply or discard resets the state, so a stale `true` cannot
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

/// The resume session id for a follow-up, scoped to the thread's pinned account:
/// the newest session under `pinned_config_dir` when the thread has one, else the
/// globally-newest session (a legacy thread with no recorded config dir). Scoping
/// is what stops a resume from targeting a session created under a *different*
/// account after a mid-thread toggle flip.
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

/// Resolve `(resume_session_id, resume_branch)` for a follow-up CC request.
/// Priority: open-change branch > caller-supplied session > most recent
/// `CodingAgentIdled` > fresh start. The branch of an open change (pending or
/// set aside) wins because the change-proposal flow removes the worktree but
/// keeps the branch's commits.
///
/// `pinned_config_dir` is the thread's account pin (its first config dir): the
/// auto-detected resume session id is resolved WITHIN that account, so a thread
/// that was mis-pinned onto another account by a toggle flip still resumes the
/// session belonging to its original account rather than the globally-newest one.
///
/// `start_fresh` wins over all of it: a *successor session* never resumes, and
/// neither does a session after a *provider block*. Both run on the thread's
/// own branch.
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

/// Which session a run starts, for the handoff and resume decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RunKind {
    /// A first message, a follow-up or a Continue: the thread's own next session.
    Ordinary,
    /// A lane thread resolving its open batch. Its sessions are the thread's
    /// own, so a pending handoff starts its successor session.
    Lane,
    /// A conflict-resolution or orphan-recovery session. It is not the
    /// thread's next session, so it never consumes a pending handoff.
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

/// Resolve what a run of `kind` resumes. The thread's own next session reads
/// the pending handoff under the handoff lock. A withdraw then either lands
/// first or sees this run's claim and refuses.
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

/// Return the account pin the thread's **first** CC session was created
/// under: its `CLAUDE_CONFIG_DIR` and whether it was set at all. Every spawn
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
/// pin gets established. A failed read is an error, never `None`: reading it as
/// "no pin" would move the thread to the live account and lose its transcript.
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
