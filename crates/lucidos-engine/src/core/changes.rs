use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

/// A change as returned by API queries.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Change {
    pub id: Uuid,
    pub request_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub branch_name: String,
    pub repo_root: String,
    pub description: String,
    pub file_count: i32,
    pub files: Vec<String>,
    pub requires_restart: bool,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub merge_worktree_path: Option<String>,
    pub merge_temp_branch: Option<String>,
    pub hardened: bool,
    pub pre_merge_sha: Option<String>,
    pub post_merge_sha: Option<String>,
    pub thread_title: Option<String>,
    pub commits: Vec<String>,
    /// `true` when the originating CC turn ended in `ResponseFailed` —
    /// the worktree state reflects partial work, not a deliberate
    /// completion. The frontend reads this to surface a confirm-before-Apply
    /// warning so the user knows they're about to land partial changes.
    pub incomplete: bool,
    /// `true` when the originating thread is mid-turn — `Running` or
    /// `WaitingForUserAnswer`, the two states where `available_thread_actions`
    /// withholds Apply. Applying then races the session's next proposal (real
    /// thread 76b4ee76). NOT a DB column — populated by `enrich_thread_active`
    /// at serialize time (defaults to `false` for direct DB loads). The
    /// frontend disables Apply for these and Apply All filters them out; the
    /// per-change Apply endpoint already 409s via `guard_change_action`.
    #[sqlx(default)]
    #[serde(default)]
    pub thread_active: bool,
}

/// One thread's contribution to the current restart-required toast: the
/// originating thread's title and the list of commit subjects that will land
/// once the engine restarts.
#[derive(Debug, Clone, Serialize)]
pub struct RestartGroup {
    pub thread_id: Option<Uuid>,
    pub thread_title: Option<String>,
    pub commits: Vec<String>,
}

/// Fill `thread_title` on each Change by looking up the originating thread's
/// title in `thread_summaries`. The in-memory ChangesProjection doesn't track
/// titles (separate aggregate), so callers that serialize `Change` to JSON
/// for the UI call this once per response. Single batch query, no N+1.
pub async fn enrich_thread_titles(
    pool: &PgPool,
    changes: &mut [Change],
) -> Result<(), sqlx::Error> {
    enrich_titles(
        pool,
        changes,
        |c| c.thread_id,
        |c, t| c.thread_title = Some(t),
    )
    .await
}

/// The two thread statuses in which the coding agent is mid-turn, so Apply must
/// be withheld (mirrors the `live` clause in `available_thread_actions`).
const LIVE_THREAD_STATUSES: [&str; 2] = ["running", "waiting_for_user_answer"];

/// Of the given thread ids, return the subset whose `thread_summaries.status`
/// is mid-turn (`running` / `waiting_for_user_answer`). Single batch query.
/// Apply All filters its batch with this so it never applies a change whose
/// session is still working — the gate the per-change endpoint already enforces
/// via `guard_change_action`.
pub async fn live_thread_ids(
    pool: &PgPool,
    thread_ids: impl Iterator<Item = Uuid>,
) -> Result<std::collections::HashSet<Uuid>, sqlx::Error> {
    let ids: Vec<Uuid> = thread_ids.collect();
    if ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT thread_id FROM thread_summaries \
         WHERE thread_id = ANY($1) AND status = ANY($2)",
    )
    .bind(&ids)
    .bind(&LIVE_THREAD_STATUSES[..])
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Return the subset of `changes` whose thread is NOT mid-turn — the ones
/// Apply All / Discard All may act on. Filters out Running /
/// WaitingForUserAnswer threads: the per-change endpoints already 409 those via
/// `guard_change_action`, and acting on them in bulk would merge (or delete the
/// worktree of) a branch the coding agent is still working on. One batch query.
pub async fn drop_live_thread_changes(
    pool: &PgPool,
    changes: Vec<Change>,
) -> Result<Vec<Change>, sqlx::Error> {
    let live = live_thread_ids(pool, changes.iter().filter_map(|c| c.thread_id)).await?;
    Ok(changes
        .into_iter()
        .filter(|c| !c.thread_id.is_some_and(|tid| live.contains(&tid)))
        .collect())
}

/// A pending change with nothing in it: its branch's commits cancelled out, so
/// `reconcile_emptied_pending_change` re-synced the row to zero files.
///
/// Applying one can only push no-op commits onto `main` and — for an unhardened
/// Lucidos-source change — burn a whole harden-at-apply session on an empty
/// diff. `Discard` is the meaningful resolution, so Apply is refused: the UI
/// disables the button, `guard_change_action` 409s the per-change endpoint, and
/// `drop_empty_changes` keeps them out of Apply All (which calls
/// `engine.apply_change` directly and would otherwise bypass the gate).
///
/// Scoped to `pending` on purpose. An `applied` row legitimately reads zero
/// files in old data, and re-applying it must stay the idempotent `Noop`.
pub fn is_empty_pending_change(change: &Change) -> bool {
    change.status == "pending" && change.file_count == 0
}

/// Return the subset of `changes` that Apply All may act on — everything except
/// the empty pending ones. Discard All deliberately does NOT filter: discarding
/// is exactly how an empty change is resolved.
pub fn drop_empty_changes(changes: Vec<Change>) -> Vec<Change> {
    changes
        .into_iter()
        .filter(|c| !is_empty_pending_change(c))
        .collect()
}

/// Set `thread_active` on each pending Change by batch-loading thread run-state.
/// Applied changes are left alone (their thread state no longer gates Apply).
/// Single batch query, no N+1 — the serialize-time companion to
/// `enrich_thread_titles`.
pub async fn enrich_thread_active(
    pool: &PgPool,
    changes: &mut [Change],
) -> Result<(), sqlx::Error> {
    let live = live_thread_ids(
        pool,
        changes
            .iter()
            .filter(|c| c.status == "pending")
            .filter_map(|c| c.thread_id),
    )
    .await?;
    if live.is_empty() {
        return Ok(());
    }
    for change in changes.iter_mut() {
        if let Some(tid) = change.thread_id {
            change.thread_active = change.status == "pending" && live.contains(&tid);
        }
    }
    Ok(())
}

/// Same as `enrich_thread_titles` but for `RestartGroup`.
pub async fn enrich_restart_group_titles(
    pool: &PgPool,
    groups: &mut [RestartGroup],
) -> Result<(), sqlx::Error> {
    enrich_titles(
        pool,
        groups,
        |g| g.thread_id,
        |g, t| g.thread_title = Some(t),
    )
    .await
}

async fn enrich_titles<T>(
    pool: &PgPool,
    items: &mut [T],
    get_id: impl Fn(&T) -> Option<Uuid>,
    set_title: impl Fn(&mut T, String),
) -> Result<(), sqlx::Error> {
    let titles = fetch_titles_for(pool, items.iter().filter_map(&get_id)).await?;
    if titles.is_empty() {
        return Ok(());
    }
    for item in items.iter_mut() {
        if let Some(tid) = get_id(item) {
            if let Some(t) = titles.get(&tid) {
                set_title(item, t.clone());
            }
        }
    }
    Ok(())
}

async fn fetch_titles_for(
    pool: &PgPool,
    thread_ids: impl Iterator<Item = Uuid>,
) -> Result<HashMap<Uuid, String>, sqlx::Error> {
    let ids: Vec<Uuid> = thread_ids.collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT thread_id, title FROM thread_summaries WHERE thread_id = ANY($1)")
            .bind(&ids)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, title)| title.map(|t| (id, t)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    async fn insert_thread_summary(pool: &PgPool, thread_id: Uuid, title: &str) {
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_saved) \
             VALUES ($1, $2, 'chat', 0, NOW(), false, false)"
        )
        .bind(thread_id).bind(title)
        .execute(pool).await.expect("insert thread_summary");
    }

    fn make_change(thread_id: Option<Uuid>) -> Change {
        Change {
            id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            thread_id,
            branch_name: "branch-x".into(),
            repo_root: "/repo".into(),
            description: "desc".into(),
            file_count: 1,
            files: vec!["a.rs".into()],
            requires_restart: false,
            status: "pending".into(),
            created_at: chrono::Utc::now(),
            resolved_at: None,
            merge_worktree_path: None,
            merge_temp_branch: None,
            hardened: false,
            pre_merge_sha: None,
            post_merge_sha: None,
            thread_title: None,
            commits: vec![],
            incomplete: false,
            thread_active: false,
        }
    }

    async fn insert_thread_summary_with_status(pool: &PgPool, thread_id: Uuid, status: &str) {
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_saved, status) \
             VALUES ($1, 'T', 'chat', 0, NOW(), false, false, $2)",
        )
        .bind(thread_id)
        .bind(status)
        .execute(pool)
        .await
        .expect("insert thread_summary with status");
    }

    /// `enrich_thread_active` flags a pending change whose thread is mid-turn
    /// (`running` / `waiting_for_user_answer`), leaves idle-thread changes
    /// false, and never flags an already-applied change (its thread state no
    /// longer gates Apply).
    #[tokio::test]
    async fn enrich_thread_active_flags_only_live_pending() {
        let (pool, db) = setup_test_db().await;

        let running = Uuid::new_v4();
        let waiting_answer = Uuid::new_v4();
        let idle = Uuid::new_v4();
        let running_but_applied = Uuid::new_v4();
        insert_thread_summary_with_status(&pool, running, "running").await;
        insert_thread_summary_with_status(&pool, waiting_answer, "waiting_for_user_answer").await;
        insert_thread_summary_with_status(&pool, idle, "idle").await;
        insert_thread_summary_with_status(&pool, running_but_applied, "running").await;

        let mut changes = vec![
            make_change(Some(running)),
            make_change(Some(waiting_answer)),
            make_change(Some(idle)),
            make_change(None),
            {
                let mut c = make_change(Some(running_but_applied));
                c.status = "applied".into();
                c
            },
        ];

        enrich_thread_active(&pool, &mut changes)
            .await
            .expect("enrich");

        assert!(changes[0].thread_active, "running thread blocks apply");
        assert!(
            changes[1].thread_active,
            "waiting-for-answer thread blocks apply"
        );
        assert!(!changes[2].thread_active, "idle thread allows apply");
        assert!(!changes[3].thread_active, "no thread_id stays false");
        assert!(
            !changes[4].thread_active,
            "already-applied change is never flagged even if its thread is running"
        );

        teardown_test_db(&db).await;
    }

    /// `enrich_thread_titles` populates `thread_title` from `thread_summaries`
    /// in a single batch query, leaves `None` for changes whose thread has no
    /// title (or no thread_id), and is a no-op when called with no changes.
    #[tokio::test]
    async fn enrich_thread_titles_populates_from_summaries() {
        let (pool, db) = setup_test_db().await;

        let with_title = Uuid::new_v4();
        let no_title = Uuid::new_v4();
        insert_thread_summary(&pool, with_title, "Refactor auth").await;
        // no_title thread is intentionally never inserted

        let mut changes = vec![
            make_change(Some(with_title)),
            make_change(Some(no_title)),
            make_change(None),
        ];

        enrich_thread_titles(&pool, &mut changes)
            .await
            .expect("enrich");

        assert_eq!(changes[0].thread_title.as_deref(), Some("Refactor auth"));
        assert_eq!(
            changes[1].thread_title, None,
            "thread without summary stays None"
        );
        assert_eq!(changes[2].thread_title, None, "no thread_id stays None");

        // Empty input is a no-op (and doesn't issue any query)
        let mut empty: Vec<Change> = vec![];
        enrich_thread_titles(&pool, &mut empty)
            .await
            .expect("empty no-op");

        teardown_test_db(&db).await;
    }

    /// Apply All must not do what the per-change Apply button refuses. A
    /// pending change reconciled to zero files (its branch's commits cancelled
    /// out) is dropped from the batch; everything else survives.
    #[test]
    fn drop_empty_changes_removes_only_empty_pending_rows() {
        let mut normal = make_change(None);
        normal.file_count = 2;

        let mut emptied = make_change(None);
        emptied.file_count = 0;

        // An `applied` row legitimately reads zero files in old data, and
        // re-applying it must stay the idempotent Noop rather than a refusal.
        let mut applied_zero = make_change(None);
        applied_zero.file_count = 0;
        applied_zero.status = "applied".into();

        let (normal_id, applied_id) = (normal.id, applied_zero.id);
        let kept = drop_empty_changes(vec![normal, emptied, applied_zero]);
        let kept_ids: Vec<_> = kept.iter().map(|c| c.id).collect();
        assert_eq!(kept_ids, vec![normal_id, applied_id]);
    }

    #[test]
    fn is_empty_pending_change_is_scoped_to_pending() {
        let mut c = make_change(None);
        c.file_count = 0;
        assert!(is_empty_pending_change(&c));
        c.status = "discarded".into();
        assert!(
            !is_empty_pending_change(&c),
            "only a pending change can be refused an Apply"
        );
    }
}
