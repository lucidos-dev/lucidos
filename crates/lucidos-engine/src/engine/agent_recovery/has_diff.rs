//! Resume-dispatch / startup reconciliation free functions: orphaned CC
//! permission recovery, coding_agent_has_diff reconciliation, and
//! held-back change proposal on startup.

use super::super::agent_session::change_description_fallback;
use super::super::agent_session::resume::{deterministic_worktree_path, resolve_worktree_path};
use super::super::change_ops::branch_is_hardened;
use super::super::event_bus::{BusEvent, EventBus};
use super::super::git_ops::{
    default_local_branch, describe_branch_changes, files_require_restart, main_worktree,
    proposal_files_for_branch, worktrees_dir,
};
use super::super::thread_events::{
    EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use super::helpers::*;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Re-emit `CodingAgentPermissionResolved` for every persisted
/// `CodingAgentPermissionRequest` that has no paired resolution.
///
/// `pending_cc_permission` lives only in memory, so any Claude Code subprocess that
/// was blocked on a permission card before an engine restart is dead — the
/// MCP HTTP request was severed, the in-memory waiter is gone, and any
/// subsequent click on the (still rendered) PermissionCard hits a 404 from
/// `submit_mcp_consent`. Emitting the resolution clears the card buttons
/// (the projection flips status from `waiting_for_user_answer` back to
/// `running`); the follow-up `orphan running → idle` reset in `main.rs`
/// then settles the thread to `idle` since no live CC owns it.
pub async fn recover_orphan_cc_permission_requests(
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
) {
    let rows: Vec<(Uuid, String)> = match sqlx::query_as(
        "SELECT e.thread_id, e.payload->>'request_id' AS request_id \
         FROM events e \
         WHERE e.event_type = 'CodingAgentPermissionRequest' \
           AND e.thread_id IS NOT NULL \
           AND NOT EXISTS ( \
             SELECT 1 FROM events r \
             WHERE r.event_type = 'CodingAgentPermissionResolved' \
               AND r.payload->>'request_id' = e.payload->>'request_id' \
           )",
    )
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            log!("[Recovery] orphan CC permission query failed: {}", e);
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    log!(
        "[Recovery] Auto-resolving {} orphan CC permission request(s) (Claude Code session gone after restart)",
        rows.len()
    );

    for (thread_id, request_id) in rows {
        event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event:
                        crate::engine::thread_events::ThreadEvent::CodingAgentPermissionResolved {
                            request_id,
                            allowed: false,
                            reason: Some(
                                "Coding agent terminated before answering: request expired"
                                    .to_string(),
                            ),
                            persist_scope: None,
                        },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Recovery] CodingAgentPermissionResolved (orphan)",
            )
            .await;
    }
}

/// CC keys its session JSONL by CWD, so a resumed subprocess must boot in the
/// same path its prior turn used. Reuse the thread's deterministic path when
/// we have a mapping; fall back to `cc-<random>` only for orphan branches with
/// no thread to resume against.
pub(crate) fn lost_session_worktree_path(
    workspace_path: &Path,
    branch_thread_id: Option<Uuid>,
) -> PathBuf {
    match branch_thread_id {
        Some(thread_id) => deterministic_worktree_path(workspace_path, thread_id),
        None => {
            let wt_id = Uuid::new_v4().as_simple().to_string();
            worktrees_dir(workspace_path).join(format!("cc-{}", wt_id))
        }
    }
}

/// Reconcile `thread_summaries.coding_agent_has_diff` for one CC thread against
/// on-disk git state. Used by the engine-startup sweep
/// (`refresh_coding_agent_has_diff_for_active_cc_threads`) to recover from
/// commits that landed while the engine was down — the post-commit hook's
/// curl call is best-effort `|| true`, so an engine-down commit leaves the
/// projection at its pre-commit value (typically FALSE).
///
/// Decision tree:
/// - Worktree dir is missing on disk → write `FALSE` explicitly. The branch
///   ref may still exist in the main repo, but for a thread without a
///   live worktree the user has nothing to diff against; leaving a stale
///   `TRUE` here would render the WaitingBanner Diff button for a thread
///   that can't act on it.
/// - Worktree dir exists → delegate to `session_seed::seed_coding_agent_has_diff`,
///   which writes the explicit result of the Diff button's own computation
///   (`branch_changed_files` = `git diff <default_diff_base>...branch`): TRUE
///   when that net diff is non-empty, FALSE otherwise. Writing the explicit
///   value (not no-op) is what corrects a stale `TRUE` left by a prior
///   `ChangeProposed` whose work no longer produces a net diff against the
///   branch's true fork point.
///
/// Failures are logged but not propagated — the engine startup sweep must
/// not block on git or DB hiccups. `branch_changed_files` swallows git errors
/// to an empty list, so a transient git failure writes FALSE; the next
/// post-commit hook fire or the next engine restart reconciles.
pub(crate) async fn reconcile_thread_coding_agent_has_diff(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    repo_root: &Path,
    branch_name: &str,
    worktree_path: &Path,
) {
    if !worktree_path.exists() {
        // No worktree on disk — clear the column. The Diff button has nothing
        // to act on for a thread without a live workspace.
        if let Err(e) = sqlx::query(
            "UPDATE thread_summaries SET coding_agent_has_diff = FALSE WHERE thread_id = $1",
        )
        .bind(thread_id)
        .execute(pool)
        .await
        {
            log!(
                "[Recovery] Failed to clear coding_agent_has_diff for thread {} (missing worktree {}): {}",
                thread_id,
                worktree_path.display(),
                e
            );
        }
        return;
    }
    crate::engine::session_seed::seed_coding_agent_has_diff(
        pool,
        thread_id,
        repo_root,
        branch_name,
    )
    .await;
}

/// Engine-startup sweep that walks every active CC thread in
/// `thread_summaries` and reconciles `coding_agent_has_diff` with on-disk git
/// state. Must run BEFORE the HTTP server starts accepting frontend
/// connections — a frontend that hits a stale projection sees the wrong
/// WaitingBanner Diff button state.
///
/// Why this exists: live updates to `coding_agent_has_diff` flow through the
/// `ChangeProposed` / `ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`
/// projection handlers, fed by the per-commit `curl` call from the
/// post-commit hook installed in every CC worktree. That `curl` call
/// trails `|| true` — if the engine is down at commit time, the
/// `ChangeProposed` event never lands and the projection stays at its
/// pre-commit value. On engine restart, this sweep reconciles every active
/// CC thread against on-disk git state so the Diff button reflects reality
/// before the first SSE connection.
///
/// The enumeration filters on `is_coding_agent=true AND state='active' AND
/// archive_state!='archived'`. Discarded and archived threads are
/// intentionally skipped — `ThreadArchived` already clears
/// `coding_agent_has_diff` via its projection handler, and discarded threads
/// were never user-active. The archive filter is on `archive_state` (not
/// `state`) because the state↔archive_state collapse left archived rows
/// carrying `state='active'`. External-repo CC threads are included (the
/// single-signal Diff button design treats them as first-class — they get
/// the Diff signal, just no Apply path).
///
/// Sequential per-thread for simplicity. The git lookup is a single
/// `git rev-list` subprocess; at the expected scale (a few dozen active CC
/// threads) the cumulative cost is negligible compared to the rest of
/// engine boot. Buffered concurrency would only matter for hundreds of
/// threads, which we don't expect.
///
/// `lucidos_repo_root` is the main Lucidos repo path used for CC threads not
/// bound to an external repo (`cc_repo_id IS NULL`). Tests inject a temp
/// repo here; the engine boot path passes
/// `crate::engine::git_ops::main_worktree().await`.
pub(crate) async fn refresh_coding_agent_has_diff_for_active_cc_threads(
    pool: &sqlx::PgPool,
    workspace_path: &Path,
    lucidos_repo_root: &Path,
) {
    // Build a `cc_repo_id -> repo_root` lookup once for external-repo CC
    // threads. RepositoryStore::list returns Repository rows with `id` (Uuid)
    // and `path` (String).
    // Batch-load to avoid N separate get() calls — the sweep can touch up to
    // one row per active CC thread.
    let mut repo_root_by_id: std::collections::HashMap<String, PathBuf> =
        std::collections::HashMap::new();
    match crate::core::repositories::RepositoryStore::list(pool).await {
        Ok(repos) => {
            for repo in repos {
                repo_root_by_id.insert(repo.id.to_string(), PathBuf::from(repo.path));
            }
        }
        Err(e) => {
            log!(
                "[Recovery] Failed to list repositories for coding_agent_has_diff sweep: {}",
                e
            );
            // Continue with empty map — external-repo threads will fall back
            // to lucidos_repo_root (which will git-fail safely if the branch
            // doesn't live there, leaving an optimistic TRUE that the next
            // restart cleans up).
        }
    }

    #[derive(sqlx::FromRow)]
    struct ActiveCcThread {
        thread_id: Uuid,
        cc_repo_id: Option<String>,
        coding_agent_kind: Option<String>,
    }

    let active_threads: Vec<ActiveCcThread> = match sqlx::query_as(
        "SELECT thread_id, cc_repo_id, coding_agent_kind FROM thread_summaries \
         WHERE is_coding_agent = TRUE AND state = 'active' \
           AND archive_state != 'archived'",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            log!(
                "[Recovery] Failed to enumerate active CC threads for coding_agent_has_diff sweep: {}",
                e
            );
            return;
        }
    };

    if active_threads.is_empty() {
        // No active CC threads — common on a fresh workspace or one where
        // every Claude Code session has been archived. Skip the rest of the sweep.
        return;
    }

    let total = active_threads.len();
    let mut visited = 0usize;
    let mut skipped_no_branch = 0usize;
    let mut missing_worktree = 0usize;
    let started = std::time::Instant::now();

    for ActiveCcThread {
        thread_id,
        cc_repo_id,
        coding_agent_kind,
    } in active_threads
    {
        // Look up the latest SessionStarted's branch. Most-recent wins because
        // a thread may have multiple SessionStarted events (initial + resume +
        // hardening); the last one names the live branch.
        let branch: Option<String> = match sqlx::query_scalar(
            "SELECT payload->>'branch' FROM events \
             WHERE event_type = 'SessionStarted' AND thread_id = $1 \
               AND payload->>'branch' IS NOT NULL AND payload->>'branch' != '' \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        {
            Ok(b) => b,
            Err(e) => {
                log!(
                    "[Recovery] Failed to look up branch for thread {} during coding_agent_has_diff sweep: {}",
                    thread_id,
                    e
                );
                continue;
            }
        };

        let branch_name = match branch {
            Some(b) => b,
            None => {
                // Active CC thread with no SessionStarted carrying a branch
                // is a projection invariant violation — every active CC
                // thread MUST have at least one SessionStarted with a branch.
                // If we hit this it's a real bug worth surfacing, not a
                // silent skip.
                log!(
                    "[Recovery WARN] active CC thread {} has no SessionStarted with branch — projection invariant violated, skipping",
                    thread_id
                );
                skipped_no_branch += 1;
                continue;
            }
        };

        // App coding-agent threads live in the WORKSPACE git repo (their branch
        // holds the `data/apps/<id>/` edits) and carry a NULL `cc_repo_id`. Route
        // them to `workspace_path` BEFORE the `cc_repo_id` branch — a NULL
        // `cc_repo_id` otherwise falls through to `lucidos_repo_root`, where the
        // `lucidos-<agent>-app-...` branch does not exist, so `proposal_files_for_branch`
        // comes back empty and the sweep wipes `coding_agent_has_diff` to FALSE on
        // every restart (hiding the WaitingBanner Diff button and the standalone
        // WIP diff button). Mirrors the kind routing in
        // `propose_one_held_back_change`.
        let repo_root = if coding_agent_kind.as_deref() == Some("app") {
            workspace_path.to_path_buf()
        } else {
            match cc_repo_id.as_deref() {
                Some(rid) => repo_root_by_id.get(rid).cloned().unwrap_or_else(|| {
                    log!(
                        "[Recovery WARN] cc_repo_id {} not found in repositories map for thread {} — falling back to lucidos_repo_root",
                        rid,
                        thread_id
                    );
                    lucidos_repo_root.to_path_buf()
                }),
                None => lucidos_repo_root.to_path_buf(),
            }
        };

        // Resolve the worktree the same way production code does. The
        // deterministic path is the last-resort fallback inside
        // `resolve_worktree_path`; using it directly here would skip the
        // (1) `CodingAgentIdled.worktree_path` lookup and the (2)
        // `git worktree list` branch match. Skipping (1)+(2) would make the
        // sweep wipe `coding_agent_has_diff` for every legacy thread whose
        // worktree lives at a non-deterministic path
        // (e.g. `.lucidos/worktrees/cc-<random>` from before Phase 6.1) —
        // exactly the population the sweep is supposed to recover for.
        let worktree_path = resolve_worktree_path(
            pool,
            thread_id,
            workspace_path,
            &repo_root,
            Some(&branch_name),
        )
        .await;
        if !worktree_path.exists() {
            missing_worktree += 1;
        }

        reconcile_thread_coding_agent_has_diff(
            pool,
            thread_id,
            &repo_root,
            &branch_name,
            &worktree_path,
        )
        .await;
        visited += 1;
    }

    log!(
        "[Recovery] coding_agent_has_diff sweep: visited {}/{} active CC threads ({} skipped (no branch), {} missing worktree, {}ms)",
        visited,
        total,
        skipped_no_branch,
        missing_worktree,
        started.elapsed().as_millis()
    );
}

/// Engine-startup entry point for the `coding_agent_has_diff` reconciliation
/// sweep. Resolves the Lucidos main repo root via `main_worktree()` and
/// then defers to `refresh_coding_agent_has_diff_for_active_cc_threads`.
///
/// `main.rs` calls this once on every engine boot, after orphan worktree
/// recovery and before the HTTP server starts. Free function (rather than
/// a method on `LucidosEngine`) so it can be invoked with just the pool +
/// workspace path — the sweep doesn't need any other engine state, and
/// keeping it standalone matches the shape of
/// `recover_orphan_cc_permission_requests` which is also called free-form
/// from `main.rs`.
pub async fn refresh_coding_agent_has_diff_on_startup(pool: &sqlx::PgPool, workspace_path: &Path) {
    let lucidos_repo_root = main_worktree().await;
    refresh_coding_agent_has_diff_for_active_cc_threads(pool, workspace_path, &lucidos_repo_root)
        .await;
}

/// Engine-startup sweep that re-proposes any change that was never surfaced
/// at idle: an idle coding-agent thread with a committed branch diff but no
/// pending change. Must run BEFORE the HTTP server starts so the frontend
/// sees the correct projection on the first SSE connection.
///
/// This recovers threads that were wedged by the now-removed bg-bash
/// propose-gate. That gate refused to emit `ChangeProposed` while a CC
/// background task was (thought to be) pending; its only automatic escape
/// was a 5-minute nudge that itself failed when CC verified completion via a
/// shell `ps` check instead of `TaskOutput` (the tracker never saw the
/// drain). Such a thread would sit forever with a real diff, no Apply
/// button, no Discard, no Archive — the only escape was a manual
/// `POST /internal/seed-change-for-test`. See thread
/// `c1cec485-b1d0-483d-b31d-e2ba21dd76fb` / `7f971704-75cf-4a9e-8280-973eb2bea45d`.
///
/// With the gate gone, `may_touch_change_state_at_idle` fires for every clean
/// idle, so this sweep is a no-op in steady state — it only does work on the
/// restart that lands the gate removal, and as a general safety net for any
/// future missed idle-proposal.
///
/// Selection (column-independent): idle coding-agent threads, not archived,
/// with `coding_agent_has_diff = TRUE` and `coding_agent_proposed = FALSE`,
/// excluding external repos. Each candidate is then re-validated per-thread
/// by `propose_one_held_back_change` (see its skip conditions).
pub async fn propose_held_back_changes_on_startup(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    workspace_path: &Path,
) {
    let thread_ids = select_unproposed_idle_cc_threads(pool).await;
    if thread_ids.is_empty() {
        return;
    }
    let lucidos_repo_root = main_worktree().await;
    propose_held_back_changes_on_startup_with_roots(
        pool,
        event_bus,
        &lucidos_repo_root,
        workspace_path,
        &thread_ids,
    )
    .await;
}

/// Candidate selection for `propose_held_back_changes_on_startup`: idle
/// coding-agent threads with a committed diff but no proposed change yet.
/// Broad on purpose — `propose_one_held_back_change` re-validates every row
/// (clean-terminal, branch resolves, files non-empty, no existing pending).
pub(crate) async fn select_unproposed_idle_cc_threads(pool: &sqlx::PgPool) -> Vec<Uuid> {
    match sqlx::query_scalar::<_, Uuid>(
        "SELECT thread_id FROM thread_summaries \
         WHERE is_coding_agent = TRUE \
           AND archive_state != 'archived' \
           AND coding_agent_has_diff = TRUE \
           AND coding_agent_proposed = FALSE \
           AND coding_agent_is_external_repo = FALSE",
    )
    .fetch_all(pool)
    .await
    {
        Ok(ids) => ids,
        Err(e) => {
            log!(
                "[Recovery] Failed to select unproposed idle CC threads: {}",
                e
            );
            Vec::new()
        }
    }
}

/// Test-injectable form: takes `lucidos_repo_root` (the dev/main repo for
/// Lucidos-kind threads) and `workspace_path` (the workspace dir for App-kind
/// threads) explicitly, so integration tests can point both at a
/// `tempfile::tempdir()` repo instead of the compile-time `main_worktree()`
/// (which would resolve to the live Lucidos checkout in CI). Production code
/// calls the no-`_with_roots` form, which resolves `lucidos_repo_root` from
/// `main_worktree()` and takes `workspace_path` from the engine.
///
/// Each thread's actual repo is picked per-row by `coding_agent_kind`:
/// `'app'` → `workspace_path` (the branch lives in the workspace's own git);
/// anything else (`'lucidos'` or NULL) → `lucidos_repo_root` (Lucidos dev
/// repo). External-kind threads are filtered out earlier via the
/// `coding_agent_is_external_repo` eligibility gate in
/// `propose_one_held_back_change`.
pub async fn propose_held_back_changes_on_startup_with_roots(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    lucidos_repo_root: &Path,
    workspace_path: &Path,
    thread_ids: &[Uuid],
) {
    let started = std::time::Instant::now();
    let mut proposed = 0usize;
    for &thread_id in thread_ids {
        match propose_one_held_back_change(
            pool,
            event_bus,
            lucidos_repo_root,
            workspace_path,
            thread_id,
        )
        .await
        {
            Ok(true) => proposed += 1,
            Ok(false) => {}
            Err(e) => log!(
                "[Recovery] Failed to propose held-back change for thread {}: {}",
                thread_id,
                e
            ),
        }
    }
    if proposed > 0 {
        log!(
            "[Recovery] Proposed held-back changes for {}/{} candidate thread(s) in {}ms",
            proposed,
            thread_ids.len(),
            started.elapsed().as_millis()
        );
    }
}

/// Per-thread worker for the held-back-propose helper. Returns `Ok(true)`
/// if a `ChangeProposed` was emitted, `Ok(false)` if the thread was skipped.
///
/// Skip conditions (each emits zero `ChangeProposed`):
///   - `coding_agent_proposed = TRUE` → already has an Apply button.
///   - `coding_agent_has_diff = FALSE` → no work to propose.
///   - `coding_agent_is_external_repo = TRUE` → engine doesn't own proposals
///     for external repos (CC pushes/PRs from the session).
///   - last turn did NOT end cleanly (Generated) → only finished threads are
///     re-proposed; interrupted/canceled/failed threads recover via Continue.
///   - empty / missing SessionStarted branch → can't resolve a branch.
///   - `proposal_files_for_branch` returns `None` → no committed net diff.
///   - existing pending change for the branch → already rescued.
///
/// Surviving threads emit `ChangeProposed` with
/// `MessageOrigin::engine(EngineReason::StaleSession)` — the route popover
/// reads "Engine · Stale session cleanup".
async fn propose_one_held_back_change(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    lucidos_repo_root: &Path,
    workspace_path: &Path,
    thread_id: Uuid,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    // Eligibility: re-read from the projection. The sweep narrowed by
    // is_coding_agent + archive_state, but the per-thread shape
    // (proposed/has_diff/external/kind) still has to be checked here.
    //
    // `coding_agent_kind` picks the repo to inspect: `'app'` → the workspace
    // git (where `data/apps/<id>/` lives), anything else → the Lucidos main
    // worktree. Without the kind route, App-kind stuck threads silently fall
    // out: their branch doesn't exist in the Lucidos repo, so
    // `proposal_files_for_branch(lucidos_repo_root, ...)` returns None and the
    // thread stays stuck — the same shape this whole helper exists to fix.
    let row: Option<(bool, bool, bool, Option<String>)> = sqlx::query_as(
        "SELECT coding_agent_proposed, coding_agent_has_diff, \
                coding_agent_is_external_repo, coding_agent_kind \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let (proposed, has_diff, is_external, kind) = match row {
        Some(r) => r,
        None => return Ok(false),
    };
    if proposed || !has_diff || is_external {
        return Ok(false);
    }
    // Only re-propose threads whose last turn ended cleanly (Generated),
    // mirroring `may_touch_change_state_at_idle`'s terminal-kind gate. This
    // sweep exists to un-wedge threads that FINISHED cleanly but whose
    // per-idle `propose_change` never landed (the removed bg-bash gate, or
    // an engine death between idle and proposal). A thread whose latest turn
    // was interrupted / canceled / failed is not finished — surfacing an
    // Apply card for its committed-so-far work would be a surprise. Those
    // recover via Continue or the archive-cascade propose path instead.
    if !last_turn_ended_cleanly(pool, thread_id).await {
        return Ok(false);
    }
    let repo_root: &Path = match kind.as_deref() {
        Some("app") => workspace_path,
        _ => lucidos_repo_root,
    };

    // Branch lookup from the latest SessionStarted.
    let branch: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'branch' FROM events \
         WHERE event_type = 'SessionStarted' AND thread_id = $1 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let branch_name = match branch {
        Some(b) if !b.is_empty() => b,
        _ => return Ok(false),
    };

    // Dedup against an existing pending row keyed on the branch (covers
    // re-entrant boot, legacy per-commit emits that never set
    // coding_agent_proposed).
    if event_bus
        .changes_projection()
        .get_pending_by_branch(&branch_name)
        .await?
        .is_some()
    {
        return Ok(false);
    }

    // Real worktree files. None → no proposable diff (branch missing,
    // empty net diff, transient git failure); skip exactly like the
    // live-session propose path skips.
    let changed_files = match proposal_files_for_branch(repo_root, &branch_name).await {
        Some(f) if !f.is_empty() => f,
        _ => return Ok(false),
    };

    let change_id = Uuid::new_v4();
    let requires_restart = files_require_restart(&changed_files);
    let fallback = change_description_fallback(pool, thread_id, &branch_name).await;
    let base = default_local_branch(repo_root).await;
    let log_range = format!("{}..{}", base, &branch_name);
    let description = describe_branch_changes(repo_root, &log_range, &fallback, None).await;
    let hardened = branch_is_hardened(
        pool,
        event_bus.changes_projection(),
        repo_root,
        &branch_name,
    )
    .await;
    // We only reach here when the last turn ended cleanly (checked above), so
    // the proposed change is never flagged incomplete.
    let incomplete = false;

    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ChangeProposed {
                    change_id: change_id.to_string(),
                    description: Some(description),
                    files: changed_files,
                    requires_restart,
                    origin: Some(MessageOrigin::engine(EngineReason::StaleSession)),
                    commit_sha: None,
                    branch_name,
                    repo_root: repo_root.to_string_lossy().to_string(),
                    hardened,
                    incomplete,
                    path: String::new(),
                    diff: String::new(),
                },
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            },
            "[Recovery] ChangeProposed (held-back-on-startup)",
        )
        .await;
    Ok(true)
}

/// Engine-startup sweep for the mirror of `propose_held_back_changes_on_startup`:
/// a pending change EXISTS but its branch diff has since gone empty.
///
/// The live path reconciles such a row at the next clean idle, which covers the
/// common case (the same session commits the revert). It cannot cover a row
/// that went stale while no session was running — the branch was emptied by an
/// engine-side apply, a hand-run `git` in the worktree, or a session that died
/// before idling. Those rows sit in Review advertising files the Diff button no
/// longer shows (real change `2cc8391f`), so reconcile them before the frontend
/// makes its first SSE connection.
///
/// Reads `repo_root` off each change row rather than guessing a root, so
/// Lucidos-source, app, and external-repo changes are all handled by the same
/// pass. Every per-row decision — pending row exists, git could actually
/// answer, the diff really is empty — belongs to
/// `reconcile_emptied_pending_change`; this function only enumerates and skips
/// the rows that obviously can't need work.
///
/// Deliberately NOT gated on the thread's last turn having ended cleanly (which
/// `propose_one_held_back_change` does check). That gate exists to avoid
/// *surfacing* half-finished work as a new Apply card; here there is no new
/// card — an existing row is being corrected to match committed git state, the
/// same state the Diff button renders. Uncommitted work in a crashed session's
/// worktree is in neither surface, and the next real proposal restores the file
/// list.
pub async fn reconcile_emptied_changes_on_startup(engine: &crate::engine::LucidosEngine) {
    let started = std::time::Instant::now();
    let pending = match engine.changes().list_pending().await {
        Ok(p) => p,
        Err(e) => {
            log!(
                "[Recovery] emptied-change sweep: list_pending: {}, skipping",
                e
            );
            return;
        }
    };
    for change in pending {
        // Nothing to correct on a row that already reads empty, and a change
        // with no thread can't carry a `ChangeProposed` (a thread event).
        if change.file_count == 0 && !change.requires_restart {
            continue;
        }
        let Some(thread_id) = change.thread_id else {
            continue;
        };
        engine
            .reconcile_emptied_pending_change(
                thread_id,
                Path::new(&change.repo_root),
                &change.branch_name,
            )
            .await;
    }
    log!(
        "[Recovery] emptied-change sweep finished in {}ms",
        started.elapsed().as_millis()
    );
}
