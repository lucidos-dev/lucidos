//! Deleting a thread: the one sanctioned removal from the event log.
//!
//! ADR 0192 states the decision and what it deliberately leaves behind. The
//! design is `docs/plans/2026-09-15-deleting-a-thread.md`. Three properties
//! are load-bearing here, and each is the reason for a line of code below.
//!
//! **Only the owner's own device.** `require_owner_device` refuses a verified
//! agent-origin token, the machine-local token, another workspace and an
//! unregistered device id. It runs before anything is read or written.
//!
//! **Memory before events.** A `memory_entries` row's `source` is an event id,
//! so the events are the only way to find one. Delete the events first and the
//! rows are unreachable orphans forever.
//!
//! **One transaction.** Every destructive statement is in
//! [`delete_family_rows`], so a failure anywhere rolls the whole family back.
//! What follows the commit (the audit event, the worktrees, the projection
//! repair) is best effort and never rolled back: the data is already gone.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::worktree_cleanup::BranchDisposal;

use super::extract_thread_uuid;
use super::family::{
    classify_family, coding_agent_members, every_member, load_family, FamilyDecision, FamilyRow,
    FamilyVerb,
};

/// A rejection body in the `{reason, ...}` shape archive already answers with,
/// so one frontend formatter reads both.
type Rejection = (StatusCode, Json<serde_json::Value>);

/// Render an [`ApiError`] from the owner gate as this route's `{reason, message}`.
/// The slug comes off the status, so the two cannot drift.
fn gate_rejection(e: ApiError) -> Rejection {
    let reason = match e.status {
        StatusCode::UNAUTHORIZED => "unidentified_caller",
        StatusCode::FORBIDDEN => "not_the_owners_device",
        _ => "internal_error",
    };
    (
        e.status,
        Json(serde_json::json!({ "reason": reason, "message": e.message })),
    )
}

fn internal(e: impl std::fmt::Display) -> Rejection {
    log!("[Delete] {}", e);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "reason": "internal_error",
            "message": e.to_string(),
        })),
    )
}

/// One family member the cascade refuses to run over, for the dialog and for
/// the 409 body.
#[derive(Debug, Serialize)]
pub(in crate::api) struct BlockingMember {
    thread_id: Uuid,
    title: Option<String>,
    /// `running`, `waiting_for_user_answer`, `pending_change` or
    /// `agent_session_live`. The first three are DB state; the last is the
    /// in-memory session set, which no row records.
    reason: &'static str,
}

/// Everything the confirmation dialog needs, from one locked read.
///
/// It answers "may I" and "what should I say" together, which is what keeps
/// the dialog from claiming something untrue. Two reads of the same family a
/// moment apart could disagree.
#[derive(Debug, Serialize)]
pub(in crate::api) struct DeletePreflight {
    /// Family size, target included.
    thread_count: usize,
    /// Descendant titles, for the expandable list. The one content-bearing
    /// field, and the user is already looking at those rows in the drawer.
    sub_thread_titles: Vec<String>,
    /// `memory_entries` rows sourced to the family's events.
    memory_count: i64,
    /// Any coding-agent member with a diff on disk, or a recorded branch some
    /// repo still holds. Both halves are needed: `ThreadArchived` clears
    /// `coding_agent_has_diff`, so the column alone reads false for exactly the
    /// archived thread whose branch the delete is about to take.
    has_unapplied_branch_work: bool,
    /// Any `changes` row for the family already at `applied`.
    has_applied_changes: bool,
    /// A backup run has succeeded, so an archive taken earlier still holds this
    /// thread. Deliberately NOT "a provider is configured": turning backups off
    /// yesterday does not unmake the five archives that already hold it.
    backups_present: bool,
    /// Empty when the family is deletable.
    blocked_by: Vec<BlockingMember>,
}

#[derive(Debug, Deserialize)]
pub(in crate::api) struct PreflightQuery {
    thread_id: Uuid,
}

/// GET /api/v1/threads/delete-preflight?thread_id=<uuid>
///
/// Owner-gated like the delete itself. It carries sub-thread titles, so an
/// agent that cannot delete must not be able to read the family through it
/// either.
pub(in crate::api) async fn delete_preflight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PreflightQuery>,
) -> Result<Json<DeletePreflight>, Rejection> {
    crate::api::actor::require_owner_device(&headers, &state.pool)
        .await
        .map_err(gate_rejection)?;

    let mut tx = state.engine.pool().begin().await.map_err(internal)?;
    let family = load_family(&mut tx, query.thread_id)
        .await
        .map_err(internal)?;
    if family.is_empty() {
        let _ = tx.rollback().await;
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "reason": "thread_not_found" })),
        ));
    }
    let ids = every_member(&family);
    let event_ids = family_event_ids(&mut tx, &ids).await.map_err(internal)?;
    let memory_count = count_memory_rows(&mut tx, &event_ids)
        .await
        .map_err(internal)?;
    let facts = load_preflight_facts(&mut tx, &ids, query.thread_id)
        .await
        .map_err(internal)?;
    let coding_agents = recorded_branches(&mut tx, &coding_agent_members(&family))
        .await
        .map_err(internal)?;
    // Release the lock before the reads that do not need it: the in-memory
    // session set, git, and the backup history on the `ops` aggregate.
    tx.commit().await.map_err(internal)?;

    let blocked_by = blocking_members(&state, &family, query.thread_id, &facts.titles).await;
    let backups_present = crate::core::backup::load_last_successful_backup(&state.pool)
        .await
        .map_err(internal)?
        .is_some();
    // `coding_agent_has_diff` alone under-reports, and it under-reports on the
    // case this feature exists for. `ThreadArchived` clears the column, so an
    // archived coding-agent thread always reads false while its branch may
    // still hold commits nothing merged. Ask git as well.
    let mut has_unapplied_branch_work = facts.has_unapplied_branch_work;
    if !has_unapplied_branch_work {
        for member in &coding_agents {
            if branch_home(&state, member).await.is_some() {
                has_unapplied_branch_work = true;
                break;
            }
        }
    }

    Ok(Json(DeletePreflight {
        thread_count: ids.len(),
        sub_thread_titles: facts.sub_thread_titles,
        memory_count,
        has_unapplied_branch_work,
        has_applied_changes: facts.has_applied_changes,
        backups_present,
        blocked_by,
    }))
}

/// POST /api/v1/threads/delete, body `{ thread_id }`.
///
/// Response `{"deleted": [<uuid>, ...], "event_count": n, "memory_count": n}`.
/// A refusal is 409 with `{reason, blocking}`, the shape archive answers with.
pub(in crate::api) async fn delete_thread_family(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, Rejection> {
    let thread_uuid = extract_thread_uuid(&request).map_err(|(s, m)| {
        (
            s,
            Json(serde_json::json!({ "reason": "bad_request", "message": m })),
        )
    })?;
    // Before anything is read or written, so a refusal leaves no trace.
    let actor = crate::api::actor::require_owner_device(&headers, &state.pool)
        .await
        .map_err(gate_rejection)?;

    let mut tx = state.engine.pool().begin().await.map_err(internal)?;
    let family = load_family(&mut tx, thread_uuid).await.map_err(internal)?;

    if let FamilyDecision::Reject { status, body } =
        classify_family(&family, thread_uuid, FamilyVerb::Delete)
    {
        let _ = tx.rollback().await;
        return Err((status, Json(body)));
    }
    // The classifier reads rows; a live subprocess is in memory and no row
    // records it. Both halves are I5, and this one is why the worktree removal
    // below cannot pull a directory out from under a running session.
    let live = blocking_members(&state, &family, thread_uuid, &Default::default()).await;
    if !live.is_empty() {
        let _ = tx.rollback().await;
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "reason": "descendants_blocking",
                "blocking": live,
            })),
        ));
    }

    let ids = every_member(&family);
    // Read before the delete, because both live on rows that are about to go.
    // `parent_thread_id` names the ancestor chain to repair. The recorded
    // branches are the only way to reach a branch whose worktree the cleanup
    // worker already reclaimed.
    let surviving_parent = parent_outside_family(&mut tx, thread_uuid)
        .await
        .map_err(internal)?;
    let coding_agents = recorded_branches(&mut tx, &coding_agent_members(&family))
        .await
        .map_err(internal)?;

    let counts = delete_family_rows(&mut tx, &ids).await.map_err(internal)?;
    tx.commit().await.map_err(internal)?;

    // Past this point the data is gone. Everything below is best effort, and a
    // failure is logged rather than propagated: a 500 here would tell the user
    // the delete did not happen.
    //
    // The in-memory drops come first. A live event wait can still deliver onto
    // one of these ids. The thread event it emits would raise the thread from
    // the dead through the projection's upsert.
    drop_live_subscriptions(&state, &ids).await;
    let worktrees_removed = reclaim_worktrees(&state, &coding_agents).await;
    // Before the emit. The frame is what makes every client re-read the list,
    // and a re-read that lands first reads the stale counts.
    repair_ancestor_counts(&state, surviving_parent).await;

    state
        .engine
        .event_bus
        .emit_or_log(
            crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::ThreadsDeleted {
                    thread_ids: ids.clone(),
                    event_count: counts.events,
                    memory_count: counts.memory,
                    worktrees_removed,
                    actor: Some(actor),
                },
            ),
            "[Delete] ThreadsDeleted",
        )
        .await;

    state.engine.broadcast_changes_updated().await;

    Ok(Json(serde_json::json!({
        "deleted": ids,
        "event_count": counts.events,
        "memory_count": counts.memory,
        "worktrees_removed": worktrees_removed,
    })))
}

/// How much the one transaction removed, for the audit record.
struct DeletedCounts {
    events: i64,
    memory: i64,
}

/// Every destructive statement, in one transaction and in one function.
///
/// Private on purpose. `core::announced_surfaces` allows a raw write to an
/// announced table only from a declared owner file. It also forces a reachable
/// writer to announce. Keeping this one unreachable is what makes the cascade's
/// single `ThreadsDeleted` the honest record.
///
/// **Step 1 must stay first.** A `memory_entries` row is found only by joining
/// its `source` event id against `events`, so deleting the events first strands
/// every row it named.
///
/// `apply_all_batches` is deliberately untouched, and ADR 0192's Consequences
/// lists it beside the other three things a delete leaves behind.
async fn delete_family_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<DeletedCounts, sqlx::Error> {
    let event_ids = family_event_ids(tx, ids).await?;
    let event_id_texts: Vec<String> = event_ids.iter().map(|id| id.to_string()).collect();

    // 1. Memory, first, and by the text form of the id: that is what
    //    `memory_entries_source_event_id_idx` is an expression index on
    //    (created at runtime in memory/pgvector.rs, since the table is).
    let memory = if memory_table_exists(tx).await? {
        sqlx::query(
            "DELETE FROM memory_entries \
             WHERE source->>'type' = 'event' AND source->>'id' = ANY($1)",
        )
        .bind(&event_id_texts)
        .execute(&mut **tx)
        .await?
        .rows_affected() as i64
    } else {
        0
    };

    // 2. Notifications, by thread AND by the event they deep-link to. A row
    //    naming a deleted event would open a thread that no longer exists.
    sqlx::query("DELETE FROM notifications WHERE thread_id = ANY($1) OR event_id = ANY($2)")
        .bind(ids)
        .bind(&event_ids)
        .execute(&mut **tx)
        .await?;

    // 3. The change rows. An applied change's merge commit is untouched: it is
    //    on main, and ADR 0192 says so out loud.
    sqlx::query("DELETE FROM changes WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;

    sqlx::query("DELETE FROM thread_queue WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM standing_applies WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;

    // 4. The events themselves, by BOTH columns. A `SystemEvent` about a thread
    //    may carry one and not the other, and `persist` only mirrors
    //    `aggregate_id` into `thread_id` for the `thread` aggregate.
    let events = sqlx::query(
        "DELETE FROM events \
         WHERE thread_id = ANY($1) OR (aggregate = 'thread' AND aggregate_id = ANY($2))",
    )
    .bind(ids)
    .bind(thread_id_texts(ids))
    .execute(&mut **tx)
    .await?
    .rows_affected() as i64;

    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;

    Ok(DeletedCounts { events, memory })
}

/// Thread ids as the text `events.aggregate_id` stores them.
fn thread_id_texts(ids: &[Uuid]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

/// Every event id the family owns, by the same predicate the delete uses.
///
/// Both arms ride an existing index: `idx_events_thread_seq` and
/// `idx_events_aggregate_aggregate_id_seq`.
async fn family_event_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM events \
         WHERE thread_id = ANY($1) OR (aggregate = 'thread' AND aggregate_id = ANY($2))",
    )
    .bind(ids)
    .bind(thread_id_texts(ids))
    .fetch_all(&mut **tx)
    .await
}

/// Does this workspace have a `memory_entries` table at all?
///
/// It is created at runtime by `PgVectorIndex::new` rather than by a migration,
/// and that call is allowed to fail. A workspace whose pgvector extension will
/// not load boots with memory search disabled and no table. Asking first
/// matters, because a missing relation inside a transaction aborts every
/// statement after it. The whole cascade would then fail on exactly the
/// workspaces that have no memory to delete.
async fn memory_table_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT to_regclass('public.memory_entries') IS NOT NULL")
        .fetch_one(&mut **tx)
        .await
}

async fn count_memory_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_ids: &[Uuid],
) -> Result<i64, sqlx::Error> {
    if !memory_table_exists(tx).await? {
        return Ok(0);
    }
    let texts: Vec<String> = event_ids.iter().map(|id| id.to_string()).collect();
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_entries \
         WHERE source->>'type' = 'event' AND source->>'id' = ANY($1)",
    )
    .bind(texts)
    .fetch_one(&mut **tx)
    .await
}

/// The read-only half of the preflight, from the already-locked family.
#[derive(Default)]
struct PreflightFacts {
    titles: std::collections::HashMap<Uuid, String>,
    sub_thread_titles: Vec<String>,
    has_unapplied_branch_work: bool,
    has_applied_changes: bool,
}

async fn load_preflight_facts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
    target: Uuid,
) -> Result<PreflightFacts, sqlx::Error> {
    let rows: Vec<(Uuid, Option<String>, bool)> = sqlx::query_as(
        "SELECT thread_id, title, coding_agent_has_diff \
         FROM thread_summaries WHERE thread_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(&mut **tx)
    .await?;

    let has_applied_changes: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM changes WHERE thread_id = ANY($1) AND status = 'applied')",
    )
    .bind(ids)
    .fetch_one(&mut **tx)
    .await?;

    let mut facts = PreflightFacts {
        has_applied_changes,
        ..Default::default()
    };
    // Family order, so the dialog's list reads parent-first like the drawer.
    for id in ids {
        let Some((_, title, has_diff)) = rows.iter().find(|(tid, _, _)| tid == id) else {
            continue;
        };
        facts.has_unapplied_branch_work |= *has_diff;
        let title = title.clone().unwrap_or_default();
        if *id != target {
            facts.sub_thread_titles.push(title.clone());
        }
        facts.titles.insert(*id, title);
    }
    Ok(facts)
}

/// Which members refuse the cascade, with the reason, for the dialog and for
/// the 409 body.
///
/// It re-derives what `classify_family` decides rather than reading its
/// rejection body. The dialog wants every blocker at once, and the classifier
/// short-circuits on the first. The live-session arm has no DB row at all,
/// which is the other reason this is its own pass.
async fn blocking_members(
    state: &AppState,
    family: &[FamilyRow],
    target: Uuid,
    titles: &std::collections::HashMap<Uuid, String>,
) -> Vec<BlockingMember> {
    use crate::engine::thread_lifecycle::{ArchiveState, ThreadStatus, ThreadType};

    let mut blocked = Vec::new();
    for row in family {
        let status = ThreadStatus::parse(&row.status);
        let thread_type = if row.is_coding_agent {
            ThreadType::CodingAgent
        } else {
            ThreadType::Chat
        };
        // The target itself is judged as if it were in the inbox, exactly as
        // `thread_is_deletable` does. A descendant keeps its real section, so
        // an archived one holding a pending change does not block.
        let archive_state = if row.thread_id == target {
            ArchiveState::Inbox
        } else {
            row.archive_state_enum()
        };
        let reason = if crate::engine::thread_lifecycle::is_blocking(
            thread_type,
            status,
            archive_state,
            row.coding_agent_proposed,
            row.coding_agent_is_external_repo,
        ) {
            match status {
                ThreadStatus::Running => Some("running"),
                ThreadStatus::WaitingForUserAnswer => Some("waiting_for_user_answer"),
                _ => Some("pending_change"),
            }
        } else if state.engine.is_agent_running_for(row.thread_id).await {
            Some("agent_session_live")
        } else {
            None
        };
        if let Some(reason) = reason {
            blocked.push(BlockingMember {
                thread_id: row.thread_id,
                title: titles.get(&row.thread_id).cloned(),
                reason,
            });
        }
    }
    blocked
}

/// One coding-agent member, and the branch its events recorded.
///
/// The branch is read from the newest `SessionStarted`, most-recent-wins, the
/// way the recovery sweep reads it. `None` for a coding-agent thread that never
/// started a session.
struct CodingAgentMember {
    thread_id: Uuid,
    branch: Option<String>,
}

/// Each coding-agent member's recorded branch, read inside the transaction.
///
/// This has to happen before the events go, and it is what closes the gap the
/// worktree alone leaves. The cleanup worker reclaims a spent tree at Tier 1 or
/// Tier 2 while deliberately KEEPING a branch that holds unmerged commits. So a
/// long-idle coding-agent thread reaches delete with no worktree and a live
/// branch, which is exactly the archived garbage this feature is for.
async fn recorded_branches(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    coding_agents: &[Uuid],
) -> Result<Vec<CodingAgentMember>, sqlx::Error> {
    let mut members = Vec::with_capacity(coding_agents.len());
    for thread_id in coding_agents {
        let branch: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'branch' FROM events \
             WHERE event_type = 'SessionStarted' AND thread_id = $1 \
               AND payload->>'branch' IS NOT NULL AND payload->>'branch' != '' \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(&mut **tx)
        .await?
        .flatten();
        members.push(CodingAgentMember {
            thread_id: *thread_id,
            branch,
        });
    }
    Ok(members)
}

/// Drop every live *event wait* the deleted family holds, in memory and with no
/// event of its own.
///
/// Archive cancels a thread's waits through `EventWaitCanceled`, and delete
/// cannot. The thread has no rows left. A `BusEvent::Thread` on that id would
/// re-insert an `events` row and raise a `thread_summaries` row through the
/// projection's upsert. The wait's own `EventWaitStarted` row went with the
/// family, so the boot rebuild will not see it either.
///
/// Leaving one live is the failure this prevents. The dispatcher's set is in
/// memory, so a matching event would deliver onto a thread that no longer
/// exists and start a turn on it.
async fn drop_live_subscriptions(state: &AppState, ids: &[Uuid]) {
    let mut dropped = 0usize;
    for tid in ids {
        dropped += state.engine.drop_waits_for_thread(*tid).await;
    }
    if dropped > 0 {
        log!("[Delete] Dropped {} live event wait(s)", dropped);
    }
}

/// Remove each coding-agent member's worktree and branch, and sweep the two
/// build-gate marker rows keyed on that branch.
///
/// Best effort, and never rolled back. The data is already gone, so a stuck
/// worktree is the lesser failure. It is also the only chance: once the events
/// are removed, `lookup_thread_by_short` answers `NotFound` and the cleanup
/// worker's orphan path keeps any tree whose branch has commits (ADR 0035).
///
/// **A missing worktree is not a skip.** The worker may already have reclaimed
/// the tree and kept the branch, so the recorded branch is chased through
/// [`branch_home`]. Skipping there left a branch nothing could ever name again.
///
/// The dirtiness gate every worker caller applies is deliberately skipped. The
/// owner confirmed a destructive action naming this thread, and the cascade
/// already refused while anything in the family was live.
async fn reclaim_worktrees(state: &AppState, coding_agents: &[CodingAgentMember]) -> usize {
    use crate::engine::worktree_cleanup::{
        deterministic_worktree_for, remove_worktree_and_optionally_delete_branch,
    };

    let mut removed = 0usize;
    for member in coding_agents {
        let worktree = deterministic_worktree_for(state.engine.workspace_path(), member.thread_id);
        if !worktree.exists() {
            delete_orphaned_branch(state, member).await;
            continue;
        }
        // `Some(0)` rather than `None`. Otherwise the helper walks the whole
        // tree to size it, synchronously on this request task. The log line
        // below is the only reader of that number.
        let Some(outcome) = remove_worktree_and_optionally_delete_branch(
            &worktree,
            Some(0),
            BranchDisposal::Always,
        )
        .await
        else {
            log!(
                "[Delete] Could not remove the worktree at {}; it is left on disk",
                worktree.display()
            );
            delete_orphaned_branch(state, member).await;
            continue;
        };
        removed += 1;
        log!(
            "[Delete] Removed the worktree at {} (branch_deleted={})",
            worktree.display(),
            outcome.branch_deleted
        );
        if let Some(branch) = outcome.branch.as_deref() {
            sweep_branch_markers(state, &outcome.repo_root, branch).await;
        }
    }
    removed
}

/// Delete a recorded branch whose worktree is already gone, from whichever repo
/// [`branch_home`] finds it in.
async fn delete_orphaned_branch(state: &AppState, member: &CodingAgentMember) {
    use crate::engine::git_ops::git_cmd;

    let Some(branch) = member.branch.as_deref() else {
        return;
    };
    let Some(root) = branch_home(state, member).await else {
        log!(
            "[Delete] No repo holds the recorded branch {}; nothing to delete",
            branch
        );
        return;
    };
    match git_cmd(&["branch", "-D", branch], &root).await {
        Ok(o) if o.status.success() => {
            log!(
                "[Delete] Deleted the orphaned branch {} in {}",
                branch,
                root.display()
            );
            sweep_branch_markers(state, &root, branch).await;
        }
        Ok(o) => log!(
            "[Delete] git branch -D {} in {} failed: {}",
            branch,
            root.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log!("[Delete] git branch -D {} errored: {}", branch, e),
    }
}

/// Which repo holds `member`'s recorded branch, if any still does.
///
/// The repo set is `recovery_repo_roots`, the same enumeration the orphan sweep
/// walks, and the first repo whose refs hold the branch wins. That is the
/// lost-branch fallback that helper documents, used here for the same reason:
/// the thread's own row cannot say which repo it worked in any more.
///
/// Two callers ask the same question for opposite purposes. The preflight asks
/// whether there is branch work left to warn about, and the reclaim asks where
/// to delete it.
async fn branch_home(state: &AppState, member: &CodingAgentMember) -> Option<std::path::PathBuf> {
    use crate::engine::git_ops::{git_answer, main_worktree};

    let branch = member.branch.as_deref()?;
    let external = crate::core::repositories::RepositoryStore::list(&state.pool)
        .await
        .unwrap_or_else(|e| {
            log!("[Delete] Could not list external repos: {}", e);
            Vec::new()
        });
    let roots = crate::engine::agent_recovery::recovery_repo_roots(
        &main_worktree().await,
        state.engine.workspace_path(),
        &external,
    );
    for (root, _) in roots {
        // An unanswerable probe is not a yes, so an unreadable repo is skipped
        // rather than being handed a `git branch -D`.
        let holds = git_answer(
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
            &root,
        )
        .await
        .or_unknown(false);
        if holds {
            return Some(root);
        }
    }
    None
}

/// Drop the `hardened_branches` and `planned_branches` rows for a branch that
/// no longer exists. Both are keyed `(repo_root, branch_name)` and hold nothing
/// but build-gate bookkeeping, so this is hygiene rather than correctness.
///
/// The key goes through `canonical_repo_root`, the same way both markers are
/// written. Their own writers canonicalize because the path is derived from
/// `git rev-parse --git-common-dir`, which can answer relatively or through a
/// symlink. Binding the raw path here would match nothing and log nothing.
async fn sweep_branch_markers(state: &AppState, repo_root: &std::path::Path, branch: &str) {
    let key = crate::engine::git_ops::canonical_repo_root(repo_root);
    let statements = [
        (
            "hardened_branches",
            "DELETE FROM hardened_branches WHERE repo_root = $1 AND branch_name = $2",
        ),
        (
            "planned_branches",
            "DELETE FROM planned_branches WHERE repo_root = $1 AND branch_name = $2",
        ),
    ];
    for (table, sql) in statements {
        if let Err(e) = sqlx::query(sql)
            .bind(&key)
            .bind(branch)
            .execute(&state.pool)
            .await
        {
            log!(
                "[Delete] Could not sweep the {} row for {}: {}",
                table,
                branch,
                e
            );
        }
    }
}

/// The target's parent, when it survives the cascade.
///
/// Read BEFORE the delete, because it lives on a row that is about to go. Every
/// other family member's parent is inside the family, so this is the only edge
/// the cascade cuts.
async fn parent_outside_family(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    Ok(
        sqlx::query_scalar("SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(target)
            .fetch_optional(&mut **tx)
            .await?
            .flatten(),
    )
}

/// Recompute the descendant counters a surviving ancestor still holds for the
/// subtree that just vanished.
///
/// Nothing else does. The projection's incremental propagation keys on a live
/// event, and a delete emits none on the threads it removed. A stale
/// `blocking_descendant_count` hides the parent's own Archive and Delete for
/// good, so this is correctness rather than tidying.
///
/// Both helpers are the engine's own boot-time repairs, run over the whole
/// table. A delete is rare and takes one thread at a time. Recomputing from
/// ground truth is cheaper than a bespoke query, and cannot disagree with the
/// boot pass.
async fn repair_ancestor_counts(state: &AppState, surviving_parent: Option<Uuid>) {
    let Some(parent) = surviving_parent else {
        return;
    };
    use crate::engine::event_bus::EventBus;
    if let Err(e) = EventBus::rebuild_active_children_count(&state.pool).await {
        log!("[Delete] Could not rebuild active_children_count: {}", e);
    }
    if let Err(e) = EventBus::rebuild_blocking_descendant_count(&state.pool).await {
        log!("[Delete] Could not rebuild the descendant counts: {}", e);
    }
    // `total_children_count` has no rebuild of its own, because until now
    // nothing could reduce it. It counts children a thread ever had, and
    // archive deliberately leaves it alone so a lifted family keeps its
    // chevron. A delete is the first thing that removes children. A count of
    // one with no child left draws a chevron that expands to nothing.
    if let Err(e) = sqlx::query(
        "UPDATE thread_summaries p SET total_children_count = ( \
             SELECT COUNT(*) FROM thread_summaries c WHERE c.parent_thread_id = p.thread_id \
         ) WHERE p.thread_id = $1 \
           AND p.total_children_count <> ( \
             SELECT COUNT(*) FROM thread_summaries c WHERE c.parent_thread_id = p.thread_id \
         )",
    )
    .bind(parent)
    .execute(&state.pool)
    .await
    {
        log!("[Delete] Could not repair total_children_count: {}", e);
    }
}

#[cfg(test)]
#[path = "delete_tests.rs"]
mod tests;
