use crate::engine::git_ops::{find_worktree_for_branch, worktrees_dir};
use std::path::{Path, PathBuf};

/// Lifecycle events that close out a CC turn.
pub(crate) const CC_TURN_CLOSER_EVENTS: &str =
    "'CodingAgentIdled', 'SessionEnded', 'ChangeApplied', 'ChangeDiscarded'";

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
            "[ClaudeCode] Failed to look up latest worktree_head_sha for {}: {}",
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
            "[ClaudeCode] Failed to look up latest worktree_path for {}: {}",
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
            "[ClaudeCode] Resolved worktree path from CodingAgentIdled event for thread {}: {}",
            thread_id,
            path.display()
        );
        return path;
    }

    if let Some(branch) = branch_hint {
        if let Some(path) = find_worktree_for_branch(repo_root, branch).await {
            log!(
                "[ClaudeCode] Resolved worktree path via git worktree list for thread {} branch {}: {}",
                thread_id,
                branch,
                path.display()
            );
            return path;
        }
    }

    let path = deterministic_worktree_path(workspace_path, thread_id);
    log!(
        "[ClaudeCode] Generated new deterministic worktree path for thread {}: {}",
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
        .pop()
        .map(|c| c.branch_name);

    if let Some(branch) = pending_branch {
        let resume_sid = match caller_session_id {
            Some(sid) => Some(sid),
            None => lookup_latest_cc_session_id(pool, thread_id).await,
        };
        log!(
            "[ClaudeCode] Resuming on pending-change branch {} for thread {} (sid={:?})",
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
                "[ClaudeCode] Failed to look up last lifecycle event for resume: {}",
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
            "[ClaudeCode] Failed to look up session branch for {}: {}",
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
            "[ClaudeCode] Failed to look up session repo+branch for {}: {}",
            thread_id,
            e
        );
        e
    })
    .ok()
    .flatten()
}

/// Return the `cc_session_id` from the most recent `CodingAgentIdled` event
/// for the thread, or `None` if there is no idled event yet (truly first turn)
/// or the recorded sid is empty. Used to recover the resume target when CC
/// must continue an existing conversation (e.g., after a pending change, or
/// after the in-memory `agent_sessions` entry has been removed because the
/// CC subprocess exited at idle).
/// Look up the most recent "originating event" id for a thread — the
/// MessageReceived / TriggerStarted / CodingAgentUserMessageSent that
/// kicked off the latest exchange. Used by abort/recovery paths to stamp
/// `request_event_id` on ResponseAborted so the rerun can find the prompt
/// to resume. `event_types` controls which start events count for this
/// caller (chat threads use `&["MessageReceived","TriggerStarted"]`; CC
/// callers also include `CodingAgentUserMessageSent`).
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
    query.fetch_optional(pool).await.ok().flatten()
}

pub(crate) async fn lookup_latest_cc_session_id(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT payload->>'cc_session_id' FROM events \
         WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log!(
            "[ClaudeCode] Failed to look up latest cc_session_id for {}: {}",
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
                    "[ClaudeCode] Failed to fetch thread title for change description: {}",
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
mod tests {
    use super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        ActorMode, EventChannel, EventMeta, SessionEndReason, ThreadEvent,
    };
    use crate::test_support::{setup_test_db, teardown_test_db};
    use uuid::Uuid;

    fn cc_meta() -> EventMeta {
        EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        }
    }

    async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: cc_meta(),
        })
        .await
        .unwrap();
    }

    async fn seed_session_started(bus: &EventBus, thread_id: Uuid, session_id: &str, branch: &str) {
        emit(
            bus,
            thread_id,
            ThreadEvent::MessageReceived {
                text: "go".into(),
                images: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
        )
        .await;
        emit(
            bus,
            thread_id,
            ThreadEvent::SessionStarted {
                session_id: session_id.into(),
                branch: branch.into(),
                repo_id: None,
            },
        )
        .await;
    }

    async fn seed_pending_change(bus: &EventBus, thread_id: Uuid, branch: &str) -> Uuid {
        let change_id = Uuid::new_v4();
        emit(
            bus,
            thread_id,
            ThreadEvent::ChangeProposed {
                change_id: change_id.to_string(),
                description: Some("work".into()),
                files: vec!["src/x.rs".to_string()],
                requires_restart: false,
                origin: None,
                commit_sha: None,
                branch_name: branch.to_string(),
                repo_root: "/tmp/repo".to_string(),
                hardened: true,
                path: String::new(),
                diff: String::new(),
            },
        )
        .await;
        change_id
    }

    #[tokio::test]
    async fn pending_change_after_session_ended_resumes_branch() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let branch = "claude-code/pending";

        seed_session_started(&bus, thread_id, "sess-1", branch).await;
        seed_pending_change(&bus, thread_id, branch).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionEnded {
                reason: SessionEndReason::Shutdown,
            },
        )
        .await;

        let (sid, resume_branch) = resolve_resume_context(&pool, bus.changes_projection(), thread_id, None).await;
        assert_eq!(sid, None);
        assert_eq!(resume_branch, Some(branch.to_string()));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn pending_change_branch_with_idled_session_recovers_sid() {
        // Contract: when a pending change exists, the canonical branch wins for
        // branch selection (overriding any later SessionStarted on a different
        // branch), but the `cc_session_id` is recovered from the most recent
        // `CodingAgentIdled` event regardless of which branch produced it.
        // CC needs the sid to `--resume` the conversation; without it, the
        // revived subprocess starts with zero history.
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let canonical_branch = "claude-code/canonical";
        let wrong_branch = "claude-code/wrong";

        seed_session_started(&bus, thread_id, "real-session", canonical_branch).await;
        seed_pending_change(&bus, thread_id, canonical_branch).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionEnded {
                reason: SessionEndReason::Shutdown,
            },
        )
        .await;

        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionStarted {
                session_id: "wrong-session".into(),
                branch: wrong_branch.into(),
                repo_id: None,
            },
        )
        .await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("wrong-session".into()),
                agent: crate::runtime::AgentKind::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
            },
        )
        .await;

        let (sid, resume_branch) = resolve_resume_context(&pool, bus.changes_projection(), thread_id, None).await;
        assert_eq!(sid, Some("wrong-session".to_string()));
        assert_eq!(resume_branch, Some(canonical_branch.to_string()));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn pending_change_with_idled_session_recovers_cc_session_id() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let branch = "claude-code/with-idled-sid";
        let session_id = "sess-recovered-abc";

        // Seed: SessionStarted, pending change, CodingAgentIdled WITH cc_session_id, SessionEnded
        seed_session_started(&bus, thread_id, session_id, branch).await;
        seed_pending_change(&bus, thread_id, branch).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::CodingAgentIdled {
                has_changes: true,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some(session_id.into()),
                agent: crate::runtime::AgentKind::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
            },
        )
        .await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionEnded {
                reason: SessionEndReason::Shutdown,
            },
        )
        .await;

        let (sid, resume_branch) = resolve_resume_context(&pool, bus.changes_projection(), thread_id, None).await;
        assert_eq!(
            sid,
            Some(session_id.to_string()),
            "cc_session_id must be recovered when pending branch exists"
        );
        assert_eq!(resume_branch, Some(branch.to_string()));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn applied_change_falls_through_to_fresh_start() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let branch = "claude-code/applied";

        seed_session_started(&bus, thread_id, "sess-1", branch).await;
        let change_id = seed_pending_change(&bus, thread_id, branch).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionEnded {
                reason: SessionEndReason::Shutdown,
            },
        )
        .await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::ChangeApplied {
                change_id: change_id.to_string(),
                requires_restart: false,
                client_update: false,
                commits: vec![],
                thread_title: None,
                actor: None,
                pre_merge_sha: None,
                post_merge_sha: None,
                path: String::new(),
            },
        )
        .await;

        let (sid, resume_branch) = resolve_resume_context(&pool, bus.changes_projection(), thread_id, None).await;
        assert_eq!(sid, None);
        assert_eq!(resume_branch, None);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Reproduces the SessionEnded-after-CodingAgentIdled blind-spot.
    ///
    /// In a clean (no-changes) CC turn the engine emits CodingAgentIdled and
    /// then, on the post-loop cleanup or stale-resume retry path, emits
    /// SessionEnded { Completed } / { StaleResume }. The auto-detect resolver
    /// pulls the most-recent lifecycle event and returns no sid because the
    /// row is SessionEnded, not CodingAgentIdled — even though a usable
    /// CodingAgentIdled with the cc_session_id sits one row earlier.
    ///
    /// The chat handler's job is to pre-resolve the sid via
    /// `lookup_latest_cc_session_id` and pass it as `caller_session_id`. With
    /// the caller-supplied sid the resolver short-circuits on the
    /// `caller_session_id.is_some()` branch and CC resumes the conversation.
    #[tokio::test]
    async fn no_pending_change_with_session_ended_after_idle_recovers_via_caller_sid() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let branch = "claude-code/clean-turn";
        let session_id = "sess-clean-completed";

        // Clean turn: SessionStarted, CodingAgentIdled, SessionEnded { Completed }.
        // No pending change — this isolates the no-pending-change resolver path.
        seed_session_started(&bus, thread_id, session_id, branch).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some(session_id.into()),
                agent: crate::runtime::AgentKind::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
            },
        )
        .await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionEnded {
                reason: SessionEndReason::Shutdown,
            },
        )
        .await;

        // Bug repro: resolver with `None` returns `(None, None)` — the
        // auto-detect path sees SessionEnded as the latest lifecycle event
        // and refuses to resume.
        let (sid_without_caller, branch_without_caller) =
            resolve_resume_context(&pool, bus.changes_projection(), thread_id, None).await;
        assert_eq!(
            sid_without_caller, None,
            "auto-detect must return None when latest is SessionEnded — \
             this is the bug the chat handler must work around"
        );
        assert_eq!(branch_without_caller, None);

        // Fix: the chat handler looks up the prior cc_session_id and passes
        // it as `caller_session_id`. The resolver short-circuits and returns
        // the sid + the SessionStarted branch.
        let recovered_sid = lookup_latest_cc_session_id(&pool, thread_id).await;
        assert_eq!(
            recovered_sid,
            Some(session_id.to_string()),
            "lookup_latest_cc_session_id must surface the sid from CodingAgentIdled \
             so the chat handler has something to pass to the resolver"
        );

        let (sid_with_caller, branch_with_caller) =
            resolve_resume_context(&pool, bus.changes_projection(), thread_id, recovered_sid).await;
        assert_eq!(
            sid_with_caller,
            Some(session_id.to_string()),
            "caller-supplied sid must win — this is what preserves CC \
             conversation memory across the in-memory session being torn down"
        );
        assert_eq!(branch_with_caller, Some(branch.to_string()));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    // -------------------- Phase 6.1 worktree-path tests --------------------

    /// Helper: create a temp git repo with an initial commit on `main`.
    async fn make_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        use crate::engine::git_ops::git_cmd;
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let _ = git_cmd(&["init"], &repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
        tokio::fs::write(repo.join("init.txt"), "initial")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "initial commit"], &repo).await;
        (tmp, repo)
    }

    async fn emit_idled(
        bus: &EventBus,
        thread_id: Uuid,
        cc_session_id: Option<&str>,
        worktree_path: Option<&std::path::Path>,
    ) {
        emit(
            bus,
            thread_id,
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: cc_session_id.map(String::from),
                agent: crate::runtime::AgentKind::ClaudeCode,
                reason: None,
                worktree_path: worktree_path.map(|p| p.to_string_lossy().into_owned()),
                worktree_head_sha: None,
            },
        )
        .await;
    }

    #[test]
    fn deterministic_path_is_short_thread_prefix_under_workspace_worktrees() {
        let workspace = std::path::PathBuf::from("/tmp/ws");
        let thread_id =
            Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef").unwrap();
        let path = deterministic_worktree_path(&workspace, thread_id);
        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/ws/.lucidos/worktrees/thread-01234567")
        );
    }

    #[tokio::test]
    async fn first_turn_creates_deterministic_worktree_path() {
        let (pool, db_name) = setup_test_db().await;
        let (_repo_tmp, repo_root) = make_test_repo().await;
        let workspace = tempfile::tempdir().unwrap();
        let thread_id = Uuid::new_v4();

        let path = resolve_worktree_path(&pool, thread_id, workspace.path(), &repo_root, None)
            .await;
        let expected_suffix = format!(
            "thread-{}",
            &thread_id.simple().to_string()[..THREAD_WORKTREE_ID_LEN]
        );
        assert!(
            path.ends_with(&expected_suffix),
            "expected suffix {} not present in {}",
            expected_suffix,
            path.display()
        );
        assert!(path.starts_with(workspace.path()));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn subsequent_turns_reuse_recorded_worktree_path() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let (_repo_tmp, repo_root) = make_test_repo().await;
        let workspace = tempfile::tempdir().unwrap();
        let thread_id = Uuid::new_v4();

        // Promote the thread to CC lifecycle before emitting CodingAgentIdled.
        seed_session_started(&bus, thread_id, "sid-1", "claude-code/test").await;

        // Simulate Phase-6.1 idle that recorded a path.
        let recorded = workspace
            .path()
            .join(".lucidos/worktrees/thread-deadbeef");
        emit_idled(&bus, thread_id, Some("sid-1"), Some(&recorded)).await;

        let resolved =
            resolve_worktree_path(&pool, thread_id, workspace.path(), &repo_root, None).await;
        assert_eq!(
            resolved, recorded,
            "second turn must reuse the path recorded on the prior CodingAgentIdled"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn legacy_threads_resolve_via_git_worktree_list_fallback() {
        use crate::engine::git_ops::git_cmd;

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let (_repo_tmp, repo_root) = make_test_repo().await;
        let workspace = tempfile::tempdir().unwrap();
        let thread_id = Uuid::new_v4();
        let branch = "claude-code/legacy-feature";

        // Promote thread to CC lifecycle.
        seed_session_started(&bus, thread_id, "sid-legacy", branch).await;

        // Legacy CodingAgentIdled: no `worktree_path` field, but a branch
        // hint exists and the worktree is on disk.
        emit_idled(&bus, thread_id, Some("sid-legacy"), None).await;

        // Create a worktree on the branch outside the workspace dir to prove
        // the fallback returns the on-disk location, not the deterministic
        // workspace path.
        let wt_tmp = tempfile::tempdir().unwrap();
        let wt_path = wt_tmp.path().join("legacy-wt");
        let out = git_cmd(
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                branch,
            ],
            &repo_root,
        )
        .await
        .unwrap();
        assert!(
            out.status.success(),
            "worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let resolved = resolve_worktree_path(
            &pool,
            thread_id,
            workspace.path(),
            &repo_root,
            Some(branch),
        )
        .await;
        // `git worktree list` may canonicalize symlinks (macOS resolves
        // `/var/folders/...` → `/private/var/folders/...`), so compare
        // canonicalized paths to make the assertion symlink-tolerant.
        let canon = |p: &std::path::Path| {
            std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
        };
        assert_eq!(
            canon(&resolved),
            canon(&wt_path),
            "legacy thread with branch hint must resolve via git worktree list"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn legacy_thread_with_no_on_disk_worktree_falls_through_to_deterministic_path() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let (_repo_tmp, repo_root) = make_test_repo().await;
        let workspace = tempfile::tempdir().unwrap();
        let thread_id = Uuid::new_v4();

        seed_session_started(&bus, thread_id, "sid-stale", "claude-code/no-worktree").await;
        // Legacy idle without worktree_path, branch no longer has a worktree.
        emit_idled(&bus, thread_id, Some("sid-stale"), None).await;

        let resolved = resolve_worktree_path(
            &pool,
            thread_id,
            workspace.path(),
            &repo_root,
            Some("claude-code/no-worktree"),
        )
        .await;
        let expected =
            deterministic_worktree_path(workspace.path(), thread_id);
        assert_eq!(
            resolved, expected,
            "must fall through to deterministic path when branch has no worktree on disk"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn lookup_latest_worktree_path_returns_none_for_legacy_events() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_session_started(&bus, thread_id, "sid-1", "claude-code/legacy").await;
        emit_idled(&bus, thread_id, Some("sid-1"), None).await;

        let path = lookup_latest_worktree_path(&pool, thread_id).await;
        assert!(path.is_none(), "legacy idle must yield None");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn lookup_latest_worktree_path_returns_recorded_value() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_session_started(&bus, thread_id, "sid-1", "claude-code/wt").await;
        let p = std::path::PathBuf::from("/some/wt/path");
        emit_idled(&bus, thread_id, Some("sid-1"), Some(&p)).await;

        let got = lookup_latest_worktree_path(&pool, thread_id).await;
        assert_eq!(got, Some(p));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn lookup_latest_worktree_path_picks_most_recent_idle() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_session_started(&bus, thread_id, "sid-1", "claude-code/wt2").await;
        let earlier = std::path::PathBuf::from("/old/path");
        let later = std::path::PathBuf::from("/new/path");
        emit_idled(&bus, thread_id, Some("sid-1"), Some(&earlier)).await;
        emit_idled(&bus, thread_id, Some("sid-2"), Some(&later)).await;

        let got = lookup_latest_worktree_path(&pool, thread_id).await;
        assert_eq!(got, Some(later));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    // -------------------- Phase 8.1 worktree_head_sha tests --------------------

    /// Helper that fully populates the new Phase-8.1 field so we can assert
    /// SHAs round-trip through serde + the projection lookup helper.
    async fn emit_idled_with_sha(
        bus: &EventBus,
        thread_id: Uuid,
        cc_session_id: Option<&str>,
        worktree_path: Option<&std::path::Path>,
        head_sha: Option<&str>,
    ) {
        emit(
            bus,
            thread_id,
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: cc_session_id.map(String::from),
                agent: crate::runtime::AgentKind::ClaudeCode,
                reason: None,
                worktree_path: worktree_path.map(|p| p.to_string_lossy().into_owned()),
                worktree_head_sha: head_sha.map(String::from),
            },
        )
        .await;
    }

    /// Phase 8.1 contract: an idle event carrying `worktree_head_sha` must
    /// persist the field through the EventBus → DB → projection round-trip
    /// so the next spawn can diff against the recorded SHA.
    #[tokio::test]
    async fn idled_event_includes_worktree_head_sha() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

        seed_session_started(&bus, thread_id, "sid-1", "claude-code/sha").await;
        emit_idled_with_sha(
            &bus,
            thread_id,
            Some("sid-1"),
            Some(std::path::Path::new("/some/wt")),
            Some(sha),
        )
        .await;

        let got = lookup_latest_worktree_head_sha(&pool, thread_id).await;
        assert_eq!(
            got.as_deref(),
            Some(sha),
            "the SHA written to CodingAgentIdled must round-trip through the projection"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn lookup_latest_worktree_head_sha_returns_none_for_legacy_events() {
        // Legacy CodingAgentIdled (Phase 8 not yet shipped) → no SHA field.
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_session_started(&bus, thread_id, "sid-1", "claude-code/legacy-sha").await;
        emit_idled(&bus, thread_id, Some("sid-1"), None).await;

        let got = lookup_latest_worktree_head_sha(&pool, thread_id).await;
        assert!(got.is_none(), "legacy idle must yield None");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn lookup_latest_worktree_head_sha_picks_most_recent_idle() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_session_started(&bus, thread_id, "sid-1", "claude-code/sha2").await;
        let earlier = "1111111111111111111111111111111111111111";
        let later = "2222222222222222222222222222222222222222";
        emit_idled_with_sha(&bus, thread_id, Some("sid-1"), None, Some(earlier)).await;
        emit_idled_with_sha(&bus, thread_id, Some("sid-2"), None, Some(later)).await;

        let got = lookup_latest_worktree_head_sha(&pool, thread_id).await;
        assert_eq!(
            got.as_deref(),
            Some(later),
            "the most recent CodingAgentIdled must win"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    // -------------------- Phase 8.2 + 8.3 integration-style tests --------------------

    /// Phase 8.2 contract: when a thread has a prior `CodingAgentIdled` with
    /// a recorded `worktree_head_sha` AND the worktree on disk has moved
    /// since, [`super::super::external_edits::compute_external_edit_note`] driven by
    /// [`lookup_latest_worktree_head_sha`] produces a non-empty note for the
    /// next spawn to inject. Exercises the lookup → helper handoff that
    /// `run_direct_agent` performs at spawn time.
    #[tokio::test]
    async fn external_edits_produce_injected_note() {
        use crate::engine::git_ops::git_cmd;

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let (_repo_tmp, repo_root) = make_test_repo().await;
        let thread_id = Uuid::new_v4();
        seed_session_started(&bus, thread_id, "sid-edits", "claude-code/edits").await;

        // Snapshot the SHA the agent saw on its prior idle.
        let _ = git_cmd(&["config", "user.email", "t@t"], &repo_root).await;
        let _ = git_cmd(&["config", "user.name", "t"], &repo_root).await;
        let head = git_cmd(&["rev-parse", "HEAD"], &repo_root).await.unwrap();
        let last_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
        emit_idled_with_sha(
            &bus,
            thread_id,
            Some("sid-edits"),
            Some(&repo_root),
            Some(&last_sha),
        )
        .await;

        // Simulate the user externally committing in the worktree.
        tokio::fs::write(repo_root.join("user.txt"), "user did this")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo_root).await;
        let _ = git_cmd(&["commit", "-m", "user commit between turns"], &repo_root).await;

        // Drive the same lookup → helper sequence run_direct_agent uses.
        let recorded_sha = lookup_latest_worktree_head_sha(&pool, thread_id).await;
        assert!(recorded_sha.is_some(), "test setup must record a SHA");
        let note = super::super::external_edits::compute_external_edit_note(
            &repo_root,
            recorded_sha.as_deref(),
        )
        .await
        .expect("non-empty diff against recorded SHA must produce a note");

        assert!(note.contains("user commit between turns"), "note: {}", note);
        assert!(note.starts_with("[Note from engine"));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Phase 8.3 contract: when the user externally checks out a different
    /// branch in the worktree, [`super::super::external_edits::verify_branch`] fails
    /// loudly. `run_direct_agent` propagates the failure as a spawn refusal.
    #[tokio::test]
    async fn spawn_refuses_when_user_checked_out_different_branch() {
        use crate::engine::git_ops::git_cmd;

        let (_pool, _db_name) = setup_test_db().await;
        let (_repo_tmp, repo_root) = make_test_repo().await;

        // Engine expects to spawn on `claude-code/feature` …
        let expected_branch = "claude-code/feature";
        let _ = git_cmd(&["checkout", "-b", expected_branch], &repo_root).await;

        // … but the user externally jumped to a different branch.
        let _ = git_cmd(&["checkout", "-b", "user-detour"], &repo_root).await;

        let err = super::super::external_edits::verify_branch(&repo_root, expected_branch)
            .await
            .expect_err("branch mismatch must refuse the spawn");
        assert_eq!(err.expected, expected_branch);
        assert_eq!(err.found.as_deref(), Some("user-detour"));
        let msg = format!("{}", err);
        assert!(msg.contains("user-detour"));
        assert!(msg.contains(expected_branch));
        assert!(msg.contains("Resolve manually"));

        _pool.close().await;
        teardown_test_db(&_db_name).await;
    }
}
