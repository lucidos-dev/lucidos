use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::thread_lifecycle::Blocker;
use crate::engine::worktree_cleanup::BranchDisposal;
use crate::memory::pgvector::PgVectorIndex;

use super::extract_thread_uuid;
use crate::engine::archive_family_rules::{
    classify_family, coding_agent_members, every_member, load_family, with_blocker, FamilyDecision,
    FamilyRow, FamilyVerb, HOME_THREAD,
};

type Rejection = (StatusCode, Json<serde_json::Value>);

fn gate_rejection(e: ApiError) -> Rejection {
    let reason = e.reason.unwrap_or(match e.status {
        StatusCode::UNAUTHORIZED => "unidentified_caller",
        StatusCode::FORBIDDEN => "not_the_owners_device",
        _ => "internal_error",
    });
    (
        e.status,
        Json(serde_json::json!({ "reason": reason, "message": e.message })),
    )
}

fn internal(e: impl std::fmt::Display) -> Rejection {
    log_legacy!("[Delete] {}", e);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "reason": "internal_error",
            "message": e.to_string(),
        })),
    )
}

#[derive(Debug, Serialize)]
pub(in crate::api) struct BlockingMember {
    thread_id: Uuid,
    title: Option<String>,
    reason: &'static str,
}

#[derive(Debug, Serialize)]
pub(in crate::api) struct DeletePreflight {
    thread_count: usize,
    sub_thread_titles: Vec<String>,
    memory_count: i64,
    summary_rebuild_count: i64,
    /// INVARIANT: Any coding-agent member whose change state holds work, or a recorded
    /// branch some repo still holds. Both halves are needed: `ThreadArchived`
    /// clears the branch work, so the state alone reads `none` for exactly the
    /// archived thread whose branch the delete is about to take.
    has_unapplied_branch_work: bool,
    has_applied_changes: bool,
    /// INVARIANT: A backup run has succeeded, so an archive taken earlier still holds this
    /// thread. Deliberately NOT "a provider is configured": turning backups off
    /// yesterday does not unmake the five archives that already hold it.
    backups_present: bool,
    blocked_by: Vec<BlockingMember>,
}

#[derive(Debug, Deserialize)]
pub(in crate::api) struct PreflightQuery {
    thread_id: Uuid,
}

pub(in crate::api) async fn delete_preflight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PreflightQuery>,
) -> Result<Json<DeletePreflight>, Rejection> {
    crate::api::actor::require_owner_device(
        &headers,
        &state.pool,
        &state.engine.event_bus,
        None,
        "preview deleting a thread",
    )
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
    let summary_rebuild_count =
        crate::engine::summary_tree::store::family_rebuild_count(&mut tx, &ids)
            .await
            .map_err(internal)?;
    let facts = load_preflight_facts(&mut tx, &ids, query.thread_id)
        .await
        .map_err(internal)?;
    let coding_agents = recorded_branches(&mut tx, &coding_agent_members(&family))
        .await
        .map_err(internal)?;
    tx.commit().await.map_err(internal)?;

    let blocked_by = blocking_members(&state, &family, query.thread_id, &facts.titles).await;
    let backups_present = crate::core::backup::load_last_successful_backup(&state.pool)
        .await
        .map_err(internal)?
        .is_some();
    // INVARIANT: The change state alone under-reports, and it under-reports on the case
    // this feature exists for. `ThreadArchived` clears the branch work, so an
    // archived coding-agent thread always reads `none` while its branch may
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
        summary_rebuild_count,
        has_unapplied_branch_work,
        has_applied_changes: facts.has_applied_changes,
        backups_present,
        blocked_by,
    }))
}

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
    let actor = crate::api::actor::require_owner_device(
        &headers,
        &state.pool,
        &state.engine.event_bus,
        None,
        "delete a thread and its sub-threads",
    )
    .await
    .map_err(gate_rejection)?;

    let mut tx = state.engine.pool().begin().await.map_err(internal)?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(crate::engine::user_only_message_delivery::DELIVERY_LOCK)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    crate::engine::event_bus::EventBus::lock_card_topology_for_removal(&mut tx)
        .await
        .map_err(internal)?;
    let family = load_family(&mut tx, thread_uuid).await.map_err(internal)?;

    if let FamilyDecision::Reject { status, body } =
        classify_family(&family, thread_uuid, FamilyVerb::Delete)
    {
        let _ = tx.rollback().await;
        return Err((status, Json(body)));
    }
    let live = blocking_members(&state, &family, thread_uuid, &Default::default()).await;
    if !live.is_empty() {
        let _ = tx.rollback().await;
        return Err((
            StatusCode::CONFLICT,
            Json(live_session_body(live, thread_uuid)),
        ));
    }

    let ids = every_member(&family);
    let surviving_parent = parent_outside_family(&mut tx, thread_uuid)
        .await
        .map_err(internal)?;
    let owed_card = match surviving_parent {
        Some(_) => {
            state
                .engine
                .event_bus
                .owed_child_card(thread_uuid, crate::engine::event_bus::ChildSettle::Deleted)
                .await
        }
        None => None,
    };
    let coding_agents = recorded_branches(&mut tx, &coding_agent_members(&family))
        .await
        .map_err(internal)?;

    let counts = delete_family_rows(&mut tx, &ids).await.map_err(internal)?;
    tx.commit().await.map_err(internal)?;

    drop_live_subscriptions(&state, &ids).await;
    let worktrees_removed = reclaim_worktrees(&state, &coding_agents).await;
    let widgets_removed = remove_owned_widgets(&state, &ids, &actor).await;
    if let Some(parent) = surviving_parent {
        repair_ancestor_counts(&state, parent).await;
    }
    if let Some(card) = owed_card {
        state.engine.event_bus.deliver_owed_child_card(card).await;
    }

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
                    widgets_removed,
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
        "widgets_removed": widgets_removed,
    })))
}

async fn remove_owned_widgets(
    state: &AppState,
    ids: &[Uuid],
    actor: &crate::engine::thread_events::MessageOrigin,
) -> usize {
    let owned = match state.app_manager.widgets_owned_by(ids) {
        Ok(owned) => owned,
        Err(e) => {
            log_legacy!(
                "[Delete] Could not read the deleted threads' widgets: {}",
                e
            );
            return 0;
        }
    };
    if owned.is_empty() {
        return 0;
    }
    match state
        .app_manager
        .delete_widgets(&state.engine.event_bus, &owned, Some(actor.clone()))
        .await
    {
        Ok(removed) => removed.len(),
        Err(e) => {
            log_legacy!(
                "[Delete] Could not remove every widget of {:?}: {}",
                owned,
                e
            );
            owned
                .iter()
                .filter(|id| !state.app_manager.app_exists(id))
                .count()
        }
    }
}

struct DeletedCounts {
    events: i64,
    memory: i64,
}

async fn delete_family_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<DeletedCounts, sqlx::Error> {
    let event_ids = family_event_ids(tx, ids).await?;
    let event_id_texts: Vec<String> = event_ids.iter().map(|id| id.to_string()).collect();

    let memory = if PgVectorIndex::table_exists(&mut **tx).await? {
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

    let thread_texts: Vec<String> = ids.iter().map(ToString::to_string).collect();
    sqlx::query("DELETE FROM user_only_message_notices WHERE event_id = ANY($1) OR notice->>'thread_id' = ANY($2)")
        .bind(&event_ids).bind(&thread_texts).execute(&mut **tx).await?;

    sqlx::query("DELETE FROM notifications WHERE thread_id = ANY($1) OR event_id = ANY($2)")
        .bind(ids)
        .bind(&event_ids)
        .execute(&mut **tx)
        .await?;

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
    sqlx::query("DELETE FROM thread_input_chains WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM permissions WHERE holder_kind = 'thread' AND holder_id = ANY($1)")
        .bind(&thread_texts)
        .execute(&mut **tx)
        .await?;

    let events = sqlx::query(&format!(
        "DELETE FROM events WHERE {FAMILY_EVENT_PREDICATE}"
    ))
    .bind(ids)
    .bind(&thread_texts)
    .bind(crate::engine::event_bus::SystemEvent::SUB_THREAD_CARD_AGGREGATE)
    .execute(&mut **tx)
    .await?
    .rows_affected() as i64;

    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;

    crate::engine::summary_tree::store::delete_family(tx, ids).await?;

    Ok(DeletedCounts { events, memory })
}

fn thread_id_texts(ids: &[Uuid]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

const FAMILY_EVENT_PREDICATE: &str =
    "thread_id = ANY($1) OR (aggregate = 'thread' AND aggregate_id = ANY($2))
     OR (aggregate = $3 AND (
         aggregate_id = ANY($2)
         OR payload#>>'{data,source_thread_id}' = ANY($2)
         OR payload#>>'{data,child_thread_id}' = ANY($2)
     ))";

async fn family_event_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT id FROM events WHERE {FAMILY_EVENT_PREDICATE}"
    ))
    .bind(ids)
    .bind(thread_id_texts(ids))
    .bind(crate::engine::event_bus::SystemEvent::SUB_THREAD_CARD_AGGREGATE)
    .fetch_all(&mut **tx)
    .await
}

async fn count_memory_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_ids: &[Uuid],
) -> Result<i64, sqlx::Error> {
    if !PgVectorIndex::table_exists(&mut **tx).await? {
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
        "SELECT thread_id, title, coding_agent_change_state <> $2 \
         FROM thread_summaries WHERE thread_id = ANY($1)",
    )
    .bind(ids)
    .bind(crate::engine::thread_lifecycle::ChangeStateKind::None)
    .fetch_all(&mut **tx)
    .await?;

    let has_applied_changes: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM changes WHERE thread_id = ANY($1) AND status = $2)",
    )
    .bind(ids)
    .bind(crate::core::changes::ChangeStatus::Applied)
    .fetch_one(&mut **tx)
    .await?;

    let mut facts = PreflightFacts {
        has_applied_changes,
        ..Default::default()
    };
    for id in ids {
        let Some((_, title, has_work)) = rows.iter().find(|(tid, _, _)| tid == id) else {
            continue;
        };
        facts.has_unapplied_branch_work |= *has_work;
        let title = title.clone().unwrap_or_default();
        if *id != target {
            facts.sub_thread_titles.push(title.clone());
        }
        facts.titles.insert(*id, title);
    }
    Ok(facts)
}

fn live_session_body(live: Vec<BlockingMember>, target: Uuid) -> serde_json::Value {
    let blocker = if live.iter().any(|m| m.thread_id == target) {
        Blocker::Running
    } else {
        Blocker::DescendantRunning
    };
    with_blocker(
        serde_json::json!({ "reason": "descendants_blocking", "blocking": live }),
        blocker,
    )
}

async fn blocking_members(
    state: &AppState,
    family: &[FamilyRow],
    target: Uuid,
    titles: &std::collections::HashMap<Uuid, String>,
) -> Vec<BlockingMember> {
    use crate::engine::thread_lifecycle::{ArchiveState, OwnBlocker};

    let mut blocked = Vec::new();
    for row in family {
        let archive_state = if row.thread_id == target {
            ArchiveState::Inbox
        } else {
            row.archive_state_enum()
        };
        let reason = if row.is_home {
            Some(HOME_THREAD)
        } else if let Some(own) = row.own_blocker(archive_state) {
            Some(match own {
                OwnBlocker::Running => "running",
                OwnBlocker::Question => "waiting_for_user_answer",
                OwnBlocker::PendingChange => "pending_change",
                OwnBlocker::HeldProposal => "held_proposal",
            })
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

struct CodingAgentMember {
    thread_id: Uuid,
    branch: Option<String>,
}

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

async fn drop_live_subscriptions(state: &AppState, ids: &[Uuid]) {
    let mut dropped = 0usize;
    for tid in ids {
        dropped += state.engine.drop_waits_for_thread(*tid).await;
    }
    if dropped > 0 {
        log_legacy!("[Delete] Dropped {} live event wait(s)", dropped);
    }
}

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
        let on_recorded_branch =
            match detach_unless_on_recorded_branch(&worktree, member.branch.as_deref()).await {
                Ok(on_recorded) => on_recorded,
                Err(e) => {
                    log_legacy!(
                        "[Delete] Could not detach the worktree at {}, so it is left on disk: {}",
                        worktree.display(),
                        e
                    );
                    delete_orphaned_branch(state, member).await;
                    continue;
                }
            };
        // INVARIANT: `Some(0)` rather than `None`. Otherwise the helper walks the whole
        // tree to size it, synchronously on this request task. The log line
        // below is the only reader of that number.
        let Some(outcome) = remove_worktree_and_optionally_delete_branch(
            &worktree,
            Some(0),
            BranchDisposal::Always,
        )
        .await
        else {
            log_legacy!(
                "[Delete] Could not remove the worktree at {}; it is left on disk",
                worktree.display()
            );
            delete_orphaned_branch(state, member).await;
            continue;
        };
        removed += 1;
        log_legacy!(
            "[Delete] Removed the worktree at {} (branch_deleted={})",
            worktree.display(),
            outcome.branch_deleted
        );
        if let Some(branch) = outcome.branch.as_deref() {
            sweep_branch_markers(state, &outcome.repo_root, branch).await;
        }
        if !on_recorded_branch {
            delete_orphaned_branch(state, member).await;
        }
    }
    removed
}

async fn detach_unless_on_recorded_branch(
    worktree: &std::path::Path,
    recorded: Option<&str>,
) -> Result<bool, String> {
    use crate::engine::git_ops::{git_cmd, worktree_current_branch};

    let current = worktree_current_branch(worktree).await;
    if current.is_some() && current.as_deref() == recorded {
        return Ok(true);
    }
    match git_cmd(&["checkout", "--detach"], worktree).await {
        Ok(o) if o.status.success() => Ok(false),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(e),
    }
}

async fn delete_orphaned_branch(state: &AppState, member: &CodingAgentMember) {
    use crate::engine::git_ops::git_cmd;

    let Some(branch) = member.branch.as_deref() else {
        return;
    };
    let Some(root) = branch_home(state, member).await else {
        log_legacy!(
            "[Delete] No repo holds the recorded branch {}; nothing to delete",
            branch
        );
        return;
    };
    match git_cmd(&["branch", "-D", branch], &root).await {
        Ok(o) if o.status.success() => {
            log_legacy!(
                "[Delete] Deleted the orphaned branch {} in {}",
                branch,
                root.display()
            );
            sweep_branch_markers(state, &root, branch).await;
        }
        Ok(o) => log_legacy!(
            "[Delete] git branch -D {} in {} failed: {}",
            branch,
            root.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log_legacy!("[Delete] git branch -D {} errored: {}", branch, e),
    }
}

async fn branch_home(state: &AppState, member: &CodingAgentMember) -> Option<std::path::PathBuf> {
    use crate::engine::git_ops::{git_answer, main_worktree};

    let branch = member.branch.as_deref()?;
    let external = crate::core::repositories::RepositoryStore::list(&state.pool)
        .await
        .unwrap_or_else(|e| {
            log_legacy!("[Delete] Could not list external repos: {}", e);
            Vec::new()
        });
    let roots = crate::engine::agent_recovery::recovery_repo_roots(
        &main_worktree().await,
        state.engine.workspace_path(),
        &external,
    );
    for (root, _) in roots {
        // INVARIANT: An unanswerable probe is not a yes, so an unreadable repo is skipped
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
            log_legacy!(
                "[Delete] Could not sweep the {} row for {}: {}",
                table,
                branch,
                e
            );
        }
    }
}

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

async fn repair_ancestor_counts(state: &AppState, surviving_parent: Uuid) {
    use crate::engine::event_bus::EventBus;
    if let Err(e) = EventBus::rebuild_children_counts(&state.pool).await {
        log_legacy!("[Delete] Could not rebuild the child counts: {}", e);
    }
    if let Err(e) = EventBus::rebuild_blocking_descendant_count(&state.pool).await {
        log_legacy!("[Delete] Could not rebuild the descendant counts: {}", e);
    }
    state
        .engine
        .event_bus
        .request_archive_recount(surviving_parent);
}

#[cfg(test)]
#[path = "delete_tests.rs"]
mod tests;
