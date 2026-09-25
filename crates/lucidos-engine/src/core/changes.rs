use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgArgumentBuffer, PgRow, PgTypeInfo, PgValueRef};
use sqlx::{PgPool, Postgres};
use std::collections::HashMap;
use uuid::Uuid;

/// Where a change stands, as the `changes.status` column and the wire spell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeStatus {
    Pending,
    Applied,
    Discarded,
    Reverted,
}

impl ChangeStatus {
    pub const ALL: [Self; 4] = [
        Self::Pending,
        Self::Applied,
        Self::Discarded,
        Self::Reverted,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Discarded => "discarded",
            Self::Reverted => "reverted",
        }
    }
}

impl std::fmt::Display for ChangeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ChangeStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == s)
            .ok_or_else(|| format!("unknown change status {s:?}"))
    }
}

impl Serialize for ChangeStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ChangeStatus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl sqlx::Type<Postgres> for ChangeStatus {
    fn type_info() -> PgTypeInfo {
        <&str as sqlx::Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <&str as sqlx::Type<Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, Postgres> for ChangeStatus {
    fn decode(value: PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        Ok(<&str as sqlx::Decode<Postgres>>::decode(value)?.parse()?)
    }
}

impl sqlx::Encode<'_, Postgres> for ChangeStatus {
    fn encode_by_ref(
        &self,
        buf: &mut PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<Postgres>>::encode_by_ref(&self.as_str(), buf)
    }
}

/// The temp worktree and branch a Tier-3 conflict resolution merges in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeWorktree {
    pub path: String,
    pub temp_branch: String,
}

/// The merge commit an apply landed, for revert and diff. Rows applied before
/// the SHAs were recorded carry neither.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeShas {
    /// `main` before the merge.
    pub pre: Option<String>,
    /// The merge commit itself.
    pub post: Option<String>,
}

/// How far the originating thread has got with a pending change.
///
/// Not stored: `list_pending_for_readers` fills it from the thread's live
/// status, and a direct DB load reads it as settled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PendingThreadState {
    unsettled: bool,
    settling: bool,
    resolving_conflict: bool,
}

impl PendingThreadState {
    /// `conflict_pairing_open` counts only while the thread is unsettled or
    /// settling. A pairing a crash left open on a finished thread must keep
    /// the row's Discard.
    pub fn new(unsettled: bool, settling: bool, conflict_pairing_open: bool) -> Self {
        Self {
            unsettled,
            settling,
            resolving_conflict: conflict_pairing_open && (unsettled || settling),
        }
    }

    /// The thread has not finished with this change, so
    /// `available_thread_actions` withholds Apply. Two ways to be unsettled, and
    /// applying under either races what the session does next (real thread
    /// 76b4ee76):
    ///
    /// - **mid-turn**, `Running` or `WaitingForUserAnswer`;
    /// - **parked**, holding a live event wait. It wakes on the delivery and
    ///   commits on to the same branch (ADR 0106). An active sub-thread does
    ///   not count: the child writes its own worktree (ADR 0249).
    ///
    /// The frontend disables Apply for these and the bulk paths filter them
    /// out; the per-change Apply endpoint 409s via `guard_change_action`.
    pub fn unsettled(self) -> bool {
        self.unsettled
    }

    /// The thread is *settling*: running, paused, or watching an event. A
    /// *standing apply* then has a settle to wait for.
    ///
    /// The half of `unsettled` that can be waited through. A thread parked on
    /// a question is unsettled too, and arming it drops on the first look
    /// (`engine::standing_apply`). The panel needs the two apart, or it offers
    /// a control that ends the moment it is pressed.
    pub fn settling(self) -> bool {
        self.settling
    }

    /// An apply of this change is resolving merge conflicts: its conflict
    /// pairing is open (`ChangesProjection::conflict_pairing_open`) and its
    /// thread has not finished. The thread works only to finish this apply,
    /// so the panel shows the apply in flight and offers no standing apply.
    pub fn resolving_conflict(self) -> bool {
        self.resolving_conflict
    }
}

/// A change's status together with the data only that status has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeState {
    Pending {
        /// Set while a Tier-3 conflict resolution is in progress.
        merge: Option<MergeWorktree>,
        thread: PendingThreadState,
    },
    Applied(MergeShas),
    Reverted(MergeShas),
    Discarded,
}

impl ChangeState {
    pub fn status(&self) -> ChangeStatus {
        match self {
            Self::Pending { .. } => ChangeStatus::Pending,
            Self::Applied(_) => ChangeStatus::Applied,
            Self::Reverted(_) => ChangeStatus::Reverted,
            Self::Discarded => ChangeStatus::Discarded,
        }
    }
}

/// A change as returned by API queries.
#[derive(Debug, Clone)]
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
    pub state: ChangeState,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub hardened: bool,
    pub thread_title: Option<String>,
    pub commits: Vec<String>,
    /// `true` when the originating CC turn ended in `ResponseFailed` —
    /// the worktree state reflects partial work, not a deliberate
    /// completion. The frontend reads this to surface a confirm-before-Apply
    /// warning so the user knows they're about to land partial changes.
    pub incomplete: bool,
}

impl Change {
    pub fn status(&self) -> ChangeStatus {
        self.state.status()
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.state, ChangeState::Pending { .. })
    }

    pub fn merge_worktree(&self) -> Option<&MergeWorktree> {
        match &self.state {
            ChangeState::Pending { merge, .. } => merge.as_ref(),
            _ => None,
        }
    }

    /// The merge commit of an applied or reverted change.
    pub fn merge_shas(&self) -> Option<&MergeShas> {
        match &self.state {
            ChangeState::Applied(shas) | ChangeState::Reverted(shas) => Some(shas),
            _ => None,
        }
    }

    pub fn thread_state(&self) -> Option<PendingThreadState> {
        match self.state {
            ChangeState::Pending { thread, .. } => Some(thread),
            _ => None,
        }
    }
}

/// One `changes` row as stored: every state's columns side by side.
#[derive(sqlx::FromRow)]
struct ChangeRow {
    id: Uuid,
    request_id: Uuid,
    thread_id: Option<Uuid>,
    branch_name: String,
    repo_root: String,
    description: String,
    file_count: i32,
    files: Vec<String>,
    requires_restart: bool,
    status: ChangeStatus,
    created_at: chrono::DateTime<chrono::Utc>,
    resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    merge_worktree_path: Option<String>,
    merge_temp_branch: Option<String>,
    hardened: bool,
    pre_merge_sha: Option<String>,
    post_merge_sha: Option<String>,
    thread_title: Option<String>,
    commits: Vec<String>,
    incomplete: bool,
}

impl TryFrom<ChangeRow> for Change {
    type Error = sqlx::Error;

    fn try_from(row: ChangeRow) -> Result<Self, Self::Error> {
        let merge = match (row.merge_worktree_path, row.merge_temp_branch) {
            (Some(path), Some(temp_branch)) => Some(MergeWorktree { path, temp_branch }),
            (None, None) => None,
            (path, temp_branch) => {
                return Err(sqlx::Error::Decode(
                    format!(
                        "change {} has half a merge worktree: path {path:?}, temp branch {temp_branch:?}",
                        row.id
                    )
                    .into(),
                ))
            }
        };
        let shas = MergeShas {
            pre: row.pre_merge_sha,
            post: row.post_merge_sha,
        };
        let state = match row.status {
            ChangeStatus::Pending => ChangeState::Pending {
                merge,
                thread: PendingThreadState::default(),
            },
            ChangeStatus::Applied => ChangeState::Applied(shas),
            ChangeStatus::Reverted => ChangeState::Reverted(shas),
            ChangeStatus::Discarded => ChangeState::Discarded,
        };
        Ok(Change {
            id: row.id,
            request_id: row.request_id,
            thread_id: row.thread_id,
            branch_name: row.branch_name,
            repo_root: row.repo_root,
            description: row.description,
            file_count: row.file_count,
            files: row.files,
            requires_restart: row.requires_restart,
            state,
            created_at: row.created_at,
            resolved_at: row.resolved_at,
            hardened: row.hardened,
            thread_title: row.thread_title,
            commits: row.commits,
            incomplete: row.incomplete,
        })
    }
}

impl<'r> sqlx::FromRow<'r, PgRow> for Change {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        ChangeRow::from_row(row)?.try_into()
    }
}

/// The wire shape of [`Change`]: flat, with every state's fields present, so
/// the HTTP API, SSE frames and the `changes` tool keep one byte-stable JSON.
#[derive(Serialize)]
struct ChangeWire<'a> {
    id: Uuid,
    request_id: Uuid,
    thread_id: Option<Uuid>,
    branch_name: &'a str,
    repo_root: &'a str,
    description: &'a str,
    file_count: i32,
    files: &'a [String],
    requires_restart: bool,
    status: ChangeStatus,
    created_at: chrono::DateTime<chrono::Utc>,
    resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    merge_worktree_path: Option<&'a str>,
    merge_temp_branch: Option<&'a str>,
    hardened: bool,
    pre_merge_sha: Option<&'a str>,
    post_merge_sha: Option<&'a str>,
    thread_title: Option<&'a str>,
    commits: &'a [String],
    incomplete: bool,
    thread_unsettled: bool,
    thread_settling: bool,
    resolving_conflict: bool,
}

impl Serialize for Change {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let merge = self.merge_worktree();
        let shas = self.merge_shas();
        let thread = self.thread_state().unwrap_or_default();
        ChangeWire {
            id: self.id,
            request_id: self.request_id,
            thread_id: self.thread_id,
            branch_name: &self.branch_name,
            repo_root: &self.repo_root,
            description: &self.description,
            file_count: self.file_count,
            files: &self.files,
            requires_restart: self.requires_restart,
            status: self.status(),
            created_at: self.created_at,
            resolved_at: self.resolved_at,
            merge_worktree_path: merge.map(|m| m.path.as_str()),
            merge_temp_branch: merge.map(|m| m.temp_branch.as_str()),
            hardened: self.hardened,
            pre_merge_sha: shas.and_then(|s| s.pre.as_deref()),
            post_merge_sha: shas.and_then(|s| s.post.as_deref()),
            thread_title: self.thread_title.as_deref(),
            commits: &self.commits,
            incomplete: self.incomplete,
            thread_unsettled: thread.unsettled(),
            thread_settling: thread.settling(),
            resolving_conflict: thread.resolving_conflict(),
        }
        .serialize(serializer)
    }
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

/// The parked half of the same question, mirroring `has_live_event_waits` in
/// `available_thread_actions`: a thread watching an event is `idle` but will
/// wake and may commit again (ADR 0106). A running sub-thread does not park
/// its parent's change (ADR 0249), so `active_children_count` is not read.
const PARKED_THREAD_SQL: &str = "live_event_wait_count > 0";

/// Of the given thread ids, return the subset that has not finished with its
/// change: mid-turn, or parked on an event wait. One batch query. The bulk
/// paths filter their batch with this, so they never resolve a change whose
/// session is still going. That is the gate the per-change endpoint enforces
/// via `guard_change_action`.
///
/// This is the SQL mirror of `available_thread_actions`'s
/// `live || has_live_event_waits`, and the two must agree. This one drives the
/// bulk paths and the `thread_unsettled` flag the UI disables its buttons on.
/// That one drives the per-thread actions and the per-change guard.
pub async fn unsettled_thread_ids(
    pool: &PgPool,
    thread_ids: impl Iterator<Item = Uuid>,
) -> Result<std::collections::HashSet<Uuid>, sqlx::Error> {
    let ids: Vec<Uuid> = thread_ids.collect();
    if ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let rows: Vec<(Uuid,)> = sqlx::query_as(&format!(
        "SELECT thread_id FROM thread_summaries \
         WHERE thread_id = ANY($1) AND (status = ANY($2) OR {PARKED_THREAD_SQL})"
    ))
    .bind(&ids)
    .bind(&LIVE_THREAD_STATUSES[..])
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Every *sub-thread* of each root in `$1`, as `(root_id, thread_id)` rows.
///
/// **The one definition of "sub-thread" for changes.** The completion card,
/// the threads-list count and the `changes` filter all read it, so the three
/// cannot disagree on what sits below a thread. `UNION` ends the walk on a
/// corrupt parent cycle, and a reader drops the `thread_id = root_id` row such
/// a cycle yields.
const SUB_THREADS_CTE: &str = "WITH RECURSIVE sub_threads(root_id, thread_id) AS ( \
         SELECT t.parent_thread_id, t.thread_id FROM thread_summaries t \
         WHERE t.parent_thread_id = ANY($1) \
         UNION \
         SELECT s.root_id, t.thread_id FROM thread_summaries t \
         JOIN sub_threads s ON t.parent_thread_id = s.thread_id \
     )";

/// Every sub-thread of `root`, at any depth.
async fn sub_thread_ids(
    pool: &PgPool,
    root: Uuid,
) -> Result<std::collections::HashSet<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(&format!(
        "{SUB_THREADS_CTE} SELECT thread_id FROM sub_threads WHERE thread_id <> root_id"
    ))
    .bind(vec![root])
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// How many pending changes each root's sub-threads hold, at any depth. A
/// root with none is absent from the map. One query for the whole list.
pub async fn pending_sub_thread_change_counts(
    pool: &PgPool,
    roots: &[Uuid],
) -> Result<HashMap<Uuid, i64>, sqlx::Error> {
    if roots.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(&format!(
        "{SUB_THREADS_CTE} \
         SELECT s.root_id, COUNT(c.id)::bigint FROM sub_threads s \
         JOIN changes c ON c.thread_id = s.thread_id AND c.status = 'pending' \
         WHERE s.thread_id <> s.root_id \
         GROUP BY s.root_id"
    ))
    .bind(roots)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The pending changes held by `root`'s sub-threads, at any depth, oldest
/// first. The thread's own changes are not included: a completion card lists
/// those apart, so the parent can tell whose change is whose.
pub async fn sub_thread_pending_changes(
    pool: &PgPool,
    root: Uuid,
) -> Result<Vec<crate::engine::thread_events::SubThreadPendingChange>, sqlx::Error> {
    let rows: Vec<(Uuid, Uuid, Option<String>)> = sqlx::query_as(&format!(
        "{SUB_THREADS_CTE} \
         SELECT c.id, c.thread_id, ts.title FROM sub_threads s \
         JOIN changes c ON c.thread_id = s.thread_id AND c.status = 'pending' \
         LEFT JOIN thread_summaries ts ON ts.thread_id = c.thread_id \
         WHERE s.thread_id <> s.root_id \
         ORDER BY c.created_at ASC"
    ))
    .bind(vec![root])
    .fetch_all(pool)
    .await?;
    let unsettled = unsettled_thread_ids(pool, rows.iter().map(|(_, tid, _)| *tid)).await?;
    Ok(rows
        .into_iter()
        .map(|(change_id, thread_id, thread_title)| {
            crate::engine::thread_events::SubThreadPendingChange {
                change_id,
                thread_id,
                thread_title,
                thread_unsettled: unsettled.contains(&thread_id),
            }
        })
        .collect())
}

/// Return the subset of `changes` whose thread has settled: the ones Apply All
/// and Discard All may act on. Acting on the rest in bulk would merge, or delete
/// the worktree of, a branch the coding agent is still working on. The
/// per-change endpoints 409 that same hazard via `guard_change_action`, so the
/// bulk paths cannot become the quiet way around it. One batch query.
pub async fn drop_unsettled_thread_changes(
    pool: &PgPool,
    changes: Vec<Change>,
) -> Result<Vec<Change>, sqlx::Error> {
    let unsettled = unsettled_thread_ids(pool, changes.iter().filter_map(|c| c.thread_id)).await?;
    Ok(changes
        .into_iter()
        .filter(|c| !c.thread_id.is_some_and(|tid| unsettled.contains(&tid)))
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
/// An `applied` row legitimately reads zero files in old data, and
/// re-applying it must stay the idempotent `Noop`.
pub fn is_empty_pending_change(change: &Change) -> bool {
    change.is_pending() && change.file_count == 0
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

/// Which pending changes a reader asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingScope {
    All,
    /// Only changes held by this thread's *sub-threads*, at any depth. The
    /// thread's own changes are left out, as on a completion card.
    SubThreadsOf(Uuid),
}

/// The pending changes as every reader must see them: titled, and with the
/// thread-state flags filled from the thread's live status.
///
/// **The only way a pending list leaves the engine.** The flags are not
/// columns, so a bare `list_pending` serializes every change as settled. Three
/// readers drifted that way in turn: the SSE frame, then the `changes` tool,
/// which told an orchestrator a running session was done.
pub async fn list_pending_for_readers(
    pool: &PgPool,
    proj: &crate::core::changes_projection::ChangesProjection,
    scope: PendingScope,
) -> Result<Vec<Change>, sqlx::Error> {
    let mut pending = proj.list_pending().await?;
    if let PendingScope::SubThreadsOf(root) = scope {
        let below = sub_thread_ids(pool, root).await?;
        pending.retain(|c| c.thread_id.is_some_and(|tid| below.contains(&tid)));
    }
    enrich_thread_titles(pool, &mut pending).await?;
    enrich_pending_state(pool, &mut pending).await?;
    Ok(pending)
}

/// Fill the [`PendingThreadState`] of each pending Change. Three batch
/// queries, no N+1.
///
/// Every query always runs, because `settling` is NOT a subset of
/// `unsettled`. A `paused` coding-agent thread is settling, and its status is
/// outside `LIVE_THREAD_STATUSES`. Skipping the second query on an empty
/// `unsettled` hid the standing-apply control whenever no other change in the
/// batch had a live thread.
async fn enrich_pending_state(pool: &PgPool, changes: &mut [Change]) -> Result<(), sqlx::Error> {
    let pending = || {
        changes
            .iter()
            .filter(|c| c.is_pending())
            .filter_map(|c| c.thread_id.map(|tid| (tid, c.id)))
    };
    let thread_ids = || pending().map(|(tid, _)| tid);
    let unsettled = unsettled_thread_ids(pool, thread_ids()).await?;
    let settling = crate::engine::standing_apply::settling_thread_ids(pool, thread_ids()).await?;
    let resolving = crate::core::changes_projection::resolving_conflict_change_ids(
        pool,
        &pending().collect::<Vec<_>>(),
    )
    .await?;
    for change in changes.iter_mut() {
        let (Some(tid), ChangeState::Pending { thread, .. }) =
            (change.thread_id, &mut change.state)
        else {
            continue;
        };
        *thread = PendingThreadState::new(
            unsettled.contains(&tid),
            settling.contains(&tid),
            resolving.contains(&change.id),
        );
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
            state: ChangeState::Pending {
                merge: None,
                thread: PendingThreadState::default(),
            },
            created_at: chrono::Utc::now(),
            resolved_at: None,
            hardened: false,
            thread_title: None,
            commits: vec![],
            incomplete: false,
        }
    }

    fn unsettled(change: &Change) -> bool {
        change.thread_state().is_some_and(|t| t.unsettled())
    }

    fn settling(change: &Change) -> bool {
        change.thread_state().is_some_and(|t| t.settling())
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

    /// `enrich_pending_state` flags a pending change whose thread is mid-turn
    /// (`running` / `waiting_for_user_answer`), leaves idle-thread changes
    /// false, and never flags an already-applied change (its thread state no
    /// longer gates Apply).
    #[tokio::test]
    async fn enrich_pending_state_flags_only_live_pending() {
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
                c.state = ChangeState::Applied(MergeShas::default());
                c
            },
        ];

        enrich_pending_state(&pool, &mut changes)
            .await
            .expect("enrich");

        assert!(unsettled(&changes[0]), "running thread blocks apply");
        assert!(
            unsettled(&changes[1]),
            "waiting-for-answer thread blocks apply"
        );
        assert!(!unsettled(&changes[2]), "idle thread allows apply");
        assert!(!unsettled(&changes[3]), "no thread_id stays false");
        assert!(
            !unsettled(&changes[4]),
            "already-applied change is never flagged even if its thread is running"
        );

        teardown_test_db(&db).await;
    }

    /// A parked thread is `idle`. The status list alone would let its change
    /// through Apply All, and leave the button live in the Changes view. A live
    /// event wait counts (ADR 0106). An active sub-thread does not (ADR 0249),
    /// matching `available_thread_actions`.
    #[tokio::test]
    async fn only_a_live_event_wait_parks_an_idle_threads_change() {
        let (pool, db) = setup_test_db().await;

        let watching = Uuid::new_v4();
        let with_child = Uuid::new_v4();
        let settled = Uuid::new_v4();
        for tid in [watching, with_child, settled] {
            insert_thread_summary_with_status(&pool, tid, "idle").await;
        }
        sqlx::query("UPDATE thread_summaries SET live_event_wait_count = 1 WHERE thread_id = $1")
            .bind(watching)
            .execute(&pool)
            .await
            .expect("arm a wait");
        sqlx::query("UPDATE thread_summaries SET active_children_count = 1 WHERE thread_id = $1")
            .bind(with_child)
            .execute(&pool)
            .await
            .expect("give it a child");

        let mut changes = vec![
            make_change(Some(watching)),
            make_change(Some(with_child)),
            make_change(Some(settled)),
        ];
        enrich_pending_state(&pool, &mut changes)
            .await
            .expect("enrich");

        assert!(unsettled(&changes[0]), "a live event wait blocks apply");
        assert!(
            !unsettled(&changes[1]),
            "an active child alone does not block apply"
        );
        assert!(
            !unsettled(&changes[2]),
            "a settled idle thread still allows apply"
        );

        // The bulk paths read the same predicate, so Apply All drops the
        // watching one and keeps the delegating parent and the settled one.
        let kept = drop_unsettled_thread_changes(&pool, changes)
            .await
            .expect("filter");
        let kept_threads: Vec<_> = kept.iter().map(|c| c.thread_id).collect();
        assert_eq!(kept_threads, vec![Some(with_child), Some(settled)]);

        teardown_test_db(&db).await;
    }

    /// A `paused` coding-agent thread is settling but not unsettled, so
    /// `settling` must be set even when nothing in the batch is
    /// unsettled. Otherwise the Changes panel hides the standing-apply control
    /// that `available_thread_actions` offers for the same thread.
    #[tokio::test]
    async fn a_paused_thread_is_settling_even_when_nothing_is_unsettled() {
        let (pool, db) = setup_test_db().await;

        let paused = Uuid::new_v4();
        insert_thread_summary_with_status(&pool, paused, "paused").await;
        mark_coding_agent(&pool, paused).await;

        let mut changes = vec![make_change(Some(paused))];
        enrich_pending_state(&pool, &mut changes)
            .await
            .expect("enrich");

        assert!(
            !unsettled(&changes[0]),
            "a paused thread is not mid-turn, so Apply stays offered"
        );
        assert!(
            settling(&changes[0]),
            "a standing apply on a paused thread still has a settle to wait for"
        );

        teardown_test_db(&db).await;
    }

    /// A coding-agent thread watching an event is both: Apply is withheld
    /// (ADR 0106), and a standing apply waits the wait out (ADR 0266).
    #[tokio::test]
    async fn a_thread_watching_an_event_is_unsettled_and_settling() {
        let (pool, db) = setup_test_db().await;

        let watching = Uuid::new_v4();
        insert_thread_summary_with_status(&pool, watching, "idle").await;
        mark_coding_agent(&pool, watching).await;
        sqlx::query("UPDATE thread_summaries SET live_event_wait_count = 1 WHERE thread_id = $1")
            .bind(watching)
            .execute(&pool)
            .await
            .expect("arm a wait");

        let mut changes = vec![make_change(Some(watching))];
        enrich_pending_state(&pool, &mut changes)
            .await
            .expect("enrich");

        assert!(unsettled(&changes[0]), "a live event wait blocks apply");
        assert!(
            settling(&changes[0]),
            "a standing apply waits through the wait"
        );

        teardown_test_db(&db).await;
    }

    async fn mark_coding_agent(pool: &PgPool, thread_id: Uuid) {
        sqlx::query("UPDATE thread_summaries SET is_coding_agent = TRUE WHERE thread_id = $1")
            .bind(thread_id)
            .execute(pool)
            .await
            .expect("mark it a coding-agent thread");
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
        applied_zero.state = ChangeState::Applied(MergeShas::default());

        let (normal_id, applied_id) = (normal.id, applied_zero.id);
        let kept = drop_empty_changes(vec![normal, emptied, applied_zero]);
        let kept_ids: Vec<_> = kept.iter().map(|c| c.id).collect();
        assert_eq!(kept_ids, vec![normal_id, applied_id]);
    }

    /// Every surface that serves a pending list reads it through
    /// `list_pending_for_readers`. A behavioural test cannot catch a new
    /// reader that forgets: it has no flags to assert against, only `false`.
    #[test]
    fn every_pending_reader_goes_through_the_loader() {
        let readers = [
            ("api/changes.rs", include_str!("../api/changes.rs")),
            (
                "engine/change_ops_emitters.rs",
                include_str!("../engine/change_ops_emitters.rs"),
            ),
            (
                "engine/tools/mod.rs",
                include_str!("../engine/tools/mod.rs"),
            ),
        ];
        for (path, source) in readers {
            assert!(
                source.contains("list_pending_for_readers("),
                "{path} serves pending changes without the loader"
            );
        }
        // These two serve nothing but readers, so a bare list is always a
        // list with every flag false. `api/changes.rs` keeps one for Discard
        // All, which filters by the gate itself and serves no list.
        for (path, source) in &readers[1..] {
            assert!(
                !source.contains(".list_pending()"),
                "{path} lists pending changes without their thread state"
            );
        }
    }

    #[test]
    fn is_empty_pending_change_is_scoped_to_pending() {
        let mut c = make_change(None);
        c.file_count = 0;
        assert!(is_empty_pending_change(&c));
        c.state = ChangeState::Discarded;
        assert!(
            !is_empty_pending_change(&c),
            "only a pending change can be refused an Apply"
        );
    }

    #[test]
    fn every_change_status_round_trips_through_its_wire_string() {
        let wire: Vec<&str> = ChangeStatus::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(wire, ["pending", "applied", "discarded", "reverted"]);
        for status in ChangeStatus::ALL {
            assert_eq!(status.as_str().parse::<ChangeStatus>(), Ok(status));
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, format!("\"{}\"", status.as_str()));
            assert_eq!(serde_json::from_str::<ChangeStatus>(&json).unwrap(), status);
        }
    }

    #[test]
    fn an_unknown_change_status_is_an_error() {
        assert!("bogus".parse::<ChangeStatus>().is_err());
        assert!("Pending".parse::<ChangeStatus>().is_err());
        assert!(serde_json::from_str::<ChangeStatus>("\"bogus\"").is_err());
    }

    async fn insert_raw_change(pool: &PgPool, status: &str, merge_path: Option<&str>) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO changes (id, request_id, branch_name, repo_root, status, merge_worktree_path) \
             VALUES ($1, $2, $3, '/repo', $4, $5)",
        )
        .bind(id)
        .bind(Uuid::new_v4())
        .bind(format!("b-{id}"))
        .bind(status)
        .bind(merge_path)
        .execute(pool)
        .await
        .expect("insert raw change");
        id
    }

    /// The column decodes into the enum and binds back as the same text.
    #[tokio::test]
    async fn every_change_status_round_trips_through_postgres() {
        let (pool, db) = setup_test_db().await;
        for status in ChangeStatus::ALL {
            let (decoded, text): (ChangeStatus, String) =
                sqlx::query_as("SELECT $1::text, $1::text")
                    .bind(status)
                    .fetch_one(&pool)
                    .await
                    .expect("round trip");
            assert_eq!(decoded, status);
            assert_eq!(text, status.as_str());
        }
        teardown_test_db(&db).await;
    }

    /// A status the engine never writes fails the load, rather than reading
    /// as some plausible default.
    #[tokio::test]
    async fn a_row_with_an_unknown_status_fails_to_load() {
        let (pool, db) = setup_test_db().await;
        let id = insert_raw_change(&pool, "bogus", None).await;
        let proj = crate::core::changes_projection::ChangesProjection::new(pool.clone());
        let err = proj
            .get_by_id(id)
            .await
            .expect_err("bogus status must not load");
        assert!(err.to_string().contains("bogus"), "{err}");
        teardown_test_db(&db).await;
    }

    /// `MergeWorktree` holds both columns or neither, so the row boundary
    /// refuses a path without its temp branch.
    #[tokio::test]
    async fn half_a_merge_worktree_is_rejected_at_the_boundary() {
        let (pool, db) = setup_test_db().await;
        let id = insert_raw_change(&pool, "pending", Some("/tmp/wt")).await;
        let proj = crate::core::changes_projection::ChangesProjection::new(pool.clone());
        let err = proj
            .get_by_id(id)
            .await
            .expect_err("half a pair must not load");
        assert!(err.to_string().contains("half a merge worktree"), "{err}");
        teardown_test_db(&db).await;
    }

    #[test]
    fn resolving_conflict_needs_a_thread_that_has_not_finished() {
        assert!(!PendingThreadState::new(false, false, true).resolving_conflict());
        assert!(PendingThreadState::new(true, false, true).resolving_conflict());
        assert!(PendingThreadState::new(false, true, true).resolving_conflict());
        assert!(!PendingThreadState::new(true, true, false).resolving_conflict());
    }

    fn fixed_change(state: ChangeState) -> Change {
        let at = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        Change {
            id: Uuid::from_u128(1),
            request_id: Uuid::from_u128(2),
            thread_id: Some(Uuid::from_u128(3)),
            branch_name: "b".into(),
            repo_root: "/repo".into(),
            description: "d".into(),
            file_count: 1,
            files: vec!["a.rs".into()],
            requires_restart: false,
            state,
            created_at: at("2026-01-02T03:04:05Z"),
            resolved_at: None,
            hardened: true,
            thread_title: Some("T".into()),
            commits: vec!["c".into()],
            incomplete: false,
        }
    }

    const IDS: &str = r#""id":"00000000-0000-0000-0000-000000000001","request_id":"00000000-0000-0000-0000-000000000002","thread_id":"00000000-0000-0000-0000-000000000003""#;
    const HEAD: &str = r#""branch_name":"b","repo_root":"/repo","description":"d","file_count":1,"files":["a.rs"],"requires_restart":false"#;

    /// The wire keeps every state's fields, in the order the flat struct
    /// had, so the API, SSE frames and the `changes` tool stay byte-stable.
    #[test]
    fn a_pending_change_serializes_to_the_flat_wire_shape() {
        let change = fixed_change(ChangeState::Pending {
            merge: Some(MergeWorktree {
                path: "/tmp/wt".into(),
                temp_branch: "merge-tmp/x".into(),
            }),
            thread: PendingThreadState::new(true, true, true),
        });
        let expected = format!(
            r#"{{{IDS},{HEAD},"status":"pending","created_at":"2026-01-02T03:04:05Z","resolved_at":null,"merge_worktree_path":"/tmp/wt","merge_temp_branch":"merge-tmp/x","hardened":true,"pre_merge_sha":null,"post_merge_sha":null,"thread_title":"T","commits":["c"],"incomplete":false,"thread_unsettled":true,"thread_settling":true,"resolving_conflict":true}}"#
        );
        assert_eq!(serde_json::to_string(&change).unwrap(), expected);
    }

    #[test]
    fn an_applied_change_serializes_to_the_flat_wire_shape() {
        let change = fixed_change(ChangeState::Applied(MergeShas {
            pre: Some("aaa".into()),
            post: Some("bbb".into()),
        }));
        let expected = format!(
            r#"{{{IDS},{HEAD},"status":"applied","created_at":"2026-01-02T03:04:05Z","resolved_at":null,"merge_worktree_path":null,"merge_temp_branch":null,"hardened":true,"pre_merge_sha":"aaa","post_merge_sha":"bbb","thread_title":"T","commits":["c"],"incomplete":false,"thread_unsettled":false,"thread_settling":false,"resolving_conflict":false}}"#
        );
        assert_eq!(serde_json::to_string(&change).unwrap(), expected);
    }
}
