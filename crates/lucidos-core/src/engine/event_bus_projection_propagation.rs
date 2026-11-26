//! INVARIANT: `thread_summaries` blocking/attention propagation helpers for the
//! `EventBus` projection — the before/after `BlockingSample`, the ancestor
//! `active_children_count` / `blocking_descendant_count` /
//! `attention_descendant_count` reconciles, and the one-shot rebuild.
//!
//! Every recursive CTE below walks `parent_thread_id` with plain `UNION`, never
//! `UNION ALL`. A cycle in that column would otherwise never terminate, and the
//! boot path awaits `rebuild_blocking_descendant_count`, so one bad row would
//! hang every future startup. The dedup changes no count. A thread has one
//! parent, so exactly one path reaches it from any ancestor. Each walk already
//! selects a tuple unique per (root, node). The read path made the same swap,
//! and `fetch_family_extension_terminates_on_cycle` pins it.

use uuid::Uuid;

use super::super::EventBus;
use crate::engine::thread_lifecycle::{
    is_attention_needing, is_blocking, ArchiveState, ChangeStateKind, ThreadStatus, ThreadType,
};

pub(crate) struct BlockingSample {
    thread_type: ThreadType,
    status: ThreadStatus,
    archive_state: ArchiveState,
    has_pending_changes: bool,
    is_external_repo: bool,
    is_stopped_child: bool,
    is_held: bool,
}

impl BlockingSample {
    pub(crate) fn is_blocking(&self) -> bool {
        is_blocking(
            self.thread_type,
            self.status,
            self.archive_state,
            self.has_pending_changes,
            self.is_external_repo,
        )
    }

    pub(crate) fn is_attention_needing(&self) -> bool {
        is_attention_needing(
            self.thread_type,
            self.status,
            self.archive_state,
            self.has_pending_changes,
            self.is_external_repo,
            self.is_stopped_child,
            self.is_held,
        )
    }
}

/// INVARIANT: Read the blocking- and attention-relevant columns for `thread_id`. Returns
/// `Ok(None)` when the row doesn't exist (e.g. a thread-start event running
/// before its own INSERT). The wrapper that calls this treats "no row" as
/// "was not blocking", so the freshly-inserted row's first sample propagates
/// a clean +1 if blocking.
pub(crate) async fn load_blocking_sample(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<Option<BlockingSample>, sqlx::Error> {
    let row: Option<(bool, String, String, bool, bool, bool, bool)> = sqlx::query_as(
        "SELECT is_coding_agent, status, archive_state, coding_agent_change_state = $2, \
                coding_agent_is_external_repo, is_stopped_child, hold IS NOT NULL \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .bind(ChangeStateKind::Proposed)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(
        |(
            is_coding_agent,
            status_str,
            archive_state_str,
            is_proposed,
            coding_agent_is_external_repo,
            is_stopped_child,
            is_held,
        )| {
            BlockingSample {
                thread_type: if is_coding_agent {
                    ThreadType::CodingAgent
                } else {
                    ThreadType::Chat
                },
                status: ThreadStatus::parse(&status_str),
                archive_state: ArchiveState::parse(&archive_state_str),
                has_pending_changes: is_proposed,
                is_external_repo: coding_agent_is_external_repo,
                is_stopped_child,
                is_held,
            }
        },
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub(crate) struct ArchiveInputSample {
    parent_thread_id: Option<Uuid>,
    status: String,
    archive_state: String,
    state: String,
    is_saved: bool,
    change_state: String,
    has_output: bool,
    waits_on_event: bool,
    has_draft: bool,
}

impl ArchiveInputSample {
    pub(crate) fn recount_anchors(
        prev: Option<&Self>,
        curr: Option<&Self>,
        forced: bool,
    ) -> Vec<Uuid> {
        if prev == curr && !forced {
            return Vec::new();
        }
        let mut anchors: Vec<Uuid> = [prev, curr]
            .into_iter()
            .flatten()
            .filter_map(|s| s.parent_thread_id)
            .collect();
        anchors.dedup();
        anchors
    }
}

pub(crate) async fn load_archive_input_sample(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<Option<ArchiveInputSample>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT t.parent_thread_id, t.status, t.archive_state, t.state, t.is_saved, \
                t.coding_agent_change_state AS change_state, t.has_response AS has_output, \
                t.live_event_wait_count > 0 AS waits_on_event, {draft} AS has_draft \
         FROM thread_summaries t WHERE t.thread_id = $1",
        draft = crate::core::store::HAS_DRAFT_SQL,
    ))
    .bind(thread_id)
    .fetch_optional(&mut **tx)
    .await
}

/// INVARIANT: Recompute the direct parent's `active_children_count` from children
/// still in flight, per the shared `active_thread_statuses()` predicate.
/// Returns the parent's `thread_id` when one exists (so the caller can
/// rebroadcast the parent's aggregate over SSE), or `None` when
/// `child_id` has no parent.
///
/// In-flight definition matches the increment side: MessageReceived
/// stamps `+1`, no projection arm decrements when a child enters
/// `WaitingForUserAnswer`, and `reincrement_parent_active_count_if_revived`
/// explicitly opts out of re-incrementing on `UserQuestionAnswered`
/// because the counter was never decremented. Excluding WfUA from this
/// COUNT would silently drop the count below the true number of
/// unfinished children whenever a sibling's terminal event triggers a
/// reconcile. Mirrors the filter used by
/// `reconcile_descendant_counts_from` (`status IN (...)`).
///
/// Single in-tx writer of `active_children_count` on the terminal-event
/// path. Each terminal-event arm in `update_thread_projection`
/// (CodingAgentIdled, ResponseGenerated, ResponseFailed, ResponseCanceled,
/// a ResponseAborted that promises no resume, non-transient SessionEnded,
/// ThreadArchived) calls this
/// after flipping the child's `status` to its terminal value, so the COUNT
/// already excludes the just-terminated child. The companion `+1` paths
/// (MessageReceived spawn, `reincrement_parent_active_count_if_revived`)
/// continue to do delta updates — they always run AFTER the child row is
/// at `status='running'`, so the next reconcile fired by a sibling event
/// will observe and count them.
pub(crate) async fn reconcile_parent_active_children_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let Some(pid) = hold_parent_recount_lock(tx, child_id).await? else {
        return Ok(None);
    };
    recount_active_children(tx, pid).await?;
    Ok(Some(pid))
}

async fn recount_active_children(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    parent_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE thread_summaries p \
         SET active_children_count = COALESCE(rc.cnt, 0)::int \
         FROM ( \
             SELECT COUNT(*) AS cnt \
             FROM thread_summaries c \
             WHERE c.parent_thread_id = $1 \
               AND c.status = ANY($2) \
         ) rc \
         WHERE p.thread_id = $1 \
           AND p.active_children_count != COALESCE(rc.cnt, 0)::int",
    )
    .bind(parent_id)
    .bind(&crate::core::store::active_thread_statuses()[..])
    .execute(&mut **tx)
    .await?;
    Ok(())
}

const PARENT_RECOUNT_LOCK_CLASS: i32 = 0x6368_6c64;

/// INVARIANT: Serialize recounts of one parent and return its id, or `None` for a top
/// thread.
///
/// **Take this before counting a parent's children.** Siblings emit in their
/// own transactions. Under READ COMMITTED a count runs against its statement's
/// snapshot, which misses a sibling's uncommitted flip. So the later writer
/// stored a stale count over the right one: a parent read idle with a child
/// still waiting. Holding the lock makes the next statement's snapshot see the
/// earlier sibling's commit.
///
/// It is an advisory lock, not `FOR UPDATE` on the parent row. A recount that
/// changes nothing then never holds the row a parent's own event locks first.
///
/// **Every writer of a child count takes it BEFORE writing the parent row**,
/// the two `+1` paths included. Taking it after is a lock-order inversion
/// against a sibling's recount, and Postgres aborts one of the two events.
pub(crate) async fn hold_parent_recount_lock(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let Some(pid) = parent_of(tx, child_id).await? else {
        return Ok(None);
    };
    lock_parent_recount(tx, pid).await?;
    Ok(Some(pid))
}

pub(crate) async fn parent_of(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    Ok(
        sqlx::query_scalar("SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_optional(&mut **tx)
            .await?
            .flatten(),
    )
}

async fn lock_parent_recount(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    parent_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2::text))")
        .bind(PARENT_RECOUNT_LOCK_CLASS)
        .bind(parent_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

static AWAITED_BY_PARENT_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "(parent_thread_id IS NOT NULL \
          AND wake_mode IS DISTINCT FROM '{no_wake}' \
          AND (status = ANY($1) OR live_event_wait_count > 0 OR awaited_children_count > 0))",
        no_wake = crate::engine::thread_events::WakeMode::NoWake.as_str(),
    )
});

/// INVARIANT: Bring `thread_id`'s `awaited_by_parent` up to date, then climb: while a
/// flag moved, recount its parent's `awaited_children_count`, and while that
/// moved, refresh the parent's own flag. Returns every row that moved, for
/// the SSE rebroadcast.
///
/// Each level takes the parent's recount lock before it writes the parent
/// row. So the climb locks child before parent, as a terminal and a detach do.
///
/// The projection runs it at the end of every thread event. So it sees every
/// input to the flag: status, waits, wake mode at spawn and the row's count.
pub(crate) async fn reconcile_awaited_from(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut moved = Vec::new();
    let mut row = thread_id;
    while refresh_awaited_flag(tx, row).await? {
        moved.push(row);
        let Some(pid) = hold_parent_recount_lock(tx, row).await? else {
            break;
        };
        if moved.contains(&pid) || !recount_awaited_children(tx, pid).await? {
            break;
        }
        moved.push(pid);
        row = pid;
    }
    Ok(moved)
}

async fn recount_awaited_from_parent(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    parent_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    if !recount_awaited_children(tx, parent_id).await? {
        return Ok(Vec::new());
    }
    let mut moved = vec![parent_id];
    moved.extend(reconcile_awaited_from(tx, parent_id).await?);
    Ok(moved)
}

async fn refresh_awaited_flag(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let awaited = &*AWAITED_BY_PARENT_SQL;
    let moved: Option<Uuid> = sqlx::query_scalar(&format!(
        "UPDATE thread_summaries SET awaited_by_parent = {awaited} \
         WHERE thread_id = $2 AND awaited_by_parent IS DISTINCT FROM {awaited} \
         RETURNING thread_id"
    ))
    .bind(&crate::core::store::active_thread_statuses()[..])
    .bind(thread_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(moved.is_some())
}

async fn recount_awaited_children(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    parent_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let moved: Option<Uuid> = sqlx::query_scalar(
        "UPDATE thread_summaries p \
         SET awaited_children_count = w.cnt \
         FROM ( \
             SELECT COUNT(*)::int AS cnt FROM thread_summaries \
             WHERE parent_thread_id = $1 AND awaited_by_parent \
         ) w \
         WHERE p.thread_id = $1 AND p.awaited_children_count <> w.cnt \
         RETURNING p.thread_id",
    )
    .bind(parent_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(moved.is_some())
}

pub(crate) async fn reconcile_proposal_lifecycle_end(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
    extra_ancestors: &mut Vec<Uuid>,
    descendant_recount_anchors: &mut Vec<Uuid>,
) -> Result<(), sqlx::Error> {
    if let Some(parent_id) = reconcile_parent_active_children_count(tx, child_id).await? {
        extra_ancestors.push(parent_id);
        descendant_recount_anchors.push(parent_id);
    }
    Ok(())
}

pub(crate) async fn mark_parent_callback_pending(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE thread_summaries SET parent_callback_pending = TRUE, is_stopped_child = FALSE, \
                parent_wake_requested = NULL \
         WHERE thread_id = $1 AND parent_thread_id IS NOT NULL AND successor_thread_id IS NULL",
    )
    .bind(child_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn mark_stopped_child(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE thread_summaries SET is_stopped_child = TRUE \
         WHERE thread_id = $1 AND parent_thread_id IS NOT NULL AND parent_callback_pending",
    )
    .bind(child_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// INVARIANT: Settle what `child_id` owed its parent, because the parent's
/// `ChildThreadCompleted` or `ChildReportFiled` for it just landed. Clears the
/// marker and `is_stopped_child`, and returns whether the child WAS stopped.
///
/// It also spends the child's wake decision: a decision governs the one report
/// it was made for. A chat child woken by its own child's card writes no start
/// event, so its next turn then reads undecided.
///
/// The caller needs that answer because this write lands on the CHILD's row
/// inside the PARENT's event. The projection samples attention only for the
/// thread whose event it is, so a stopped child's settle has to reconcile its
/// ancestors itself. The row lock keeps the returned value honest under a
/// concurrent emit.
pub(crate) async fn settle_parent_callback(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let was_stopped: Option<bool> = sqlx::query_scalar(
        "UPDATE thread_summaries t \
         SET parent_callback_pending = FALSE, is_stopped_child = FALSE, \
             parent_wake_requested = NULL \
         FROM (SELECT thread_id, is_stopped_child FROM thread_summaries \
               WHERE thread_id = $1 FOR UPDATE) prev \
         WHERE t.thread_id = prev.thread_id \
         RETURNING prev.is_stopped_child",
    )
    .bind(child_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(was_stopped.unwrap_or(false))
}

/// INVARIANT: Bump the direct parent's `active_children_count` when an event flipped
/// `child_id` from a state outside the in-flight set back to
/// `status='running'`. Returns `Some(parent_id)` when the bump happened (so the
/// caller can add it to the SSE rebroadcast list), `None` when the child was
/// already in flight (idempotent on duplicates) or has no parent.
///
/// Why an explicit re-increment per revive arm rather than a generic
/// function-boundary delta: the child's earlier terminal already recounted
/// the parent in its own transaction. By the time a follow-up event lands,
/// the parent's counter excludes the child. There is no in-tx prev/curr
/// delta the projection can derive from.
///
/// The gate is the shared in-flight predicate (`active_thread_statuses()`,
/// the single definition `reconcile_parent_active_children_count` also uses),
/// NOT `is_blocking()` (which the sibling `ContinuationRequested` arm uses):
/// the user-driven bug case is an idle CC child with a pending change
/// (`is_blocking=true` via clause 3) receiving a follow-up, and an
/// `is_blocking` gate would incorrectly skip the re-increment.
///
/// Two ways a child can already be counted, and both must skip:
/// - `Running`, the obvious one.
/// - `WaitingForUserAnswer`. This function is not called from
///   `UserQuestionAnswered` / `CodingAgentPermissionResolved` (those resume
///   from a parked state where the counter was never decremented, because the
///   corresponding Asked event is not terminal), but a `MessageReceived` can
///   land on a parked child directly: a human's message would be routed to
///   `UserQuestionAnswered` instead, while an **Agent**-mode message
///   deliberately falls through to the injection path, which is exactly what a
///   parent's child follow-up is. Re-incrementing there over-counts by one
///   until the child's own next terminal reconciles it away.
pub(crate) async fn reincrement_parent_active_count_if_revived(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
    prev_sample: &Option<BlockingSample>,
) -> Result<Option<Uuid>, sqlx::Error> {
    let was_in_flight = prev_sample
        .as_ref()
        .map(|s| crate::core::store::active_thread_statuses().contains(&s.status.as_str()))
        .unwrap_or(false);
    if was_in_flight {
        return Ok(None);
    }
    let Some(pid) = hold_parent_recount_lock(tx, child_id).await? else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE thread_summaries SET active_children_count = \
         active_children_count + 1 WHERE thread_id = $1",
    )
    .bind(pid)
    .execute(&mut **tx)
    .await?;
    Ok(Some(pid))
}

static BLOCKING_DESCENDANT_FILTER: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "d.status IN ({running},{waiting_for_user_answer}) \
         OR (d.archive_state <> 'archived' \
             AND d.is_proposed AND d.is_coding_agent \
             AND NOT d.coding_agent_is_external_repo)",
        running = ThreadStatus::Running.sql_literal(),
        waiting_for_user_answer = ThreadStatus::WaitingForUserAnswer.sql_literal(),
    )
});

static ATTENTION_DESCENDANT_FILTER: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "(d.status = {waiting_for_user_answer} AND NOT d.is_held) \
         OR (d.archive_state <> 'archived' \
             AND ((d.is_proposed AND d.is_coding_agent \
                   AND NOT d.coding_agent_is_external_repo) \
                  OR d.is_stopped_child))",
        waiting_for_user_answer = ThreadStatus::WaitingForUserAnswer.sql_literal(),
    )
});

/// INVARIANT: Recompute `blocking_descendant_count` AND `attention_descendant_count` for
/// `thread_id` and every ancestor, from each row's transitive descendants under
/// the `is_blocking` and `is_attention_needing` predicates. Mirrors
/// `rebuild_blocking_descendant_count` for both columns. Returns the rows it
/// touched, for the SSE rebroadcast.
///
/// One SQL roundtrip, both columns updated in lockstep, so they cannot desync
/// under concurrent emits. It writes ancestor rows without their recount lock,
/// so the projection runs it after the awaited climb.
pub(crate) async fn reconcile_descendant_counts_from(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(&format!(
        "WITH RECURSIVE ancestors AS ( \
            SELECT $1::uuid AS thread_id \
            UNION \
            SELECT t.parent_thread_id AS thread_id \
            FROM thread_summaries t \
            JOIN ancestors a ON t.thread_id = a.thread_id \
            WHERE t.parent_thread_id IS NOT NULL \
         ), \
         descendants AS ( \
            SELECT a.thread_id AS root_id, c.thread_id, c.status, c.archive_state, \
                   c.coding_agent_change_state = '{proposed}' AS is_proposed, c.is_coding_agent, c.coding_agent_is_external_repo, \
                   c.is_stopped_child, c.hold IS NOT NULL AS is_held \
            FROM ancestors a \
            JOIN thread_summaries c ON c.parent_thread_id = a.thread_id \
            UNION \
            SELECT d.root_id, c.thread_id, c.status, c.archive_state, \
                   c.coding_agent_change_state = '{proposed}' AS is_proposed, c.is_coding_agent, c.coding_agent_is_external_repo, \
                   c.is_stopped_child, c.hold IS NOT NULL AS is_held \
            FROM descendants d \
            JOIN thread_summaries c ON c.parent_thread_id = d.thread_id \
         ), \
         new_counts AS ( \
            SELECT a.thread_id AS root_id, \
                   COALESCE(COUNT(*) FILTER (WHERE {blocking}), 0)::int \
                       AS blocking_cnt, \
                   COALESCE(COUNT(*) FILTER (WHERE {attention}), 0)::int \
                       AS attention_cnt \
            FROM ancestors a \
            LEFT JOIN descendants d ON d.root_id = a.thread_id \
            GROUP BY a.thread_id \
         ) \
         UPDATE thread_summaries u \
         SET blocking_descendant_count = nc.blocking_cnt, \
             attention_descendant_count = nc.attention_cnt \
         FROM new_counts nc \
         WHERE u.thread_id = nc.root_id \
           AND (u.blocking_descendant_count != nc.blocking_cnt \
                OR u.attention_descendant_count != nc.attention_cnt) \
         RETURNING u.thread_id",
        blocking = &*BLOCKING_DESCENDANT_FILTER,
        attention = &*ATTENTION_DESCENDANT_FILTER,
        proposed = ChangeStateKind::Proposed.as_str(),
    ))
    .bind(thread_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub(crate) async fn detach_child_from_parent(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    parent_id: Uuid,
    child_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let old_depth: Option<i32> = sqlx::query_scalar(
        "UPDATE thread_summaries c \
         SET parent_thread_id = NULL, parent_callback_pending = FALSE, \
             is_stopped_child = FALSE, depth = 0 \
         FROM (SELECT depth FROM thread_summaries WHERE thread_id = $1 FOR UPDATE) prev \
         WHERE c.thread_id = $1 AND c.parent_thread_id = $2 \
         RETURNING prev.depth",
    )
    .bind(child_id)
    .bind(parent_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(old_depth) = old_depth else {
        return Ok(Vec::new());
    };
    if old_depth > 0 {
        sqlx::query(
            "WITH RECURSIVE subtree AS ( \
                SELECT thread_id FROM thread_summaries WHERE parent_thread_id = $1 \
                UNION \
                SELECT c.thread_id FROM thread_summaries c \
                JOIN subtree s ON c.parent_thread_id = s.thread_id \
             ) \
             UPDATE thread_summaries SET depth = GREATEST(0, depth - $2) \
             WHERE thread_id IN (SELECT thread_id FROM subtree)",
        )
        .bind(child_id)
        .bind(old_depth)
        .execute(&mut **tx)
        .await?;
    }

    refresh_awaited_flag(tx, child_id).await?;

    lock_parent_recount(tx, parent_id).await?;
    recount_active_children(tx, parent_id).await?;
    sqlx::query(
        "UPDATE thread_summaries \
         SET total_children_count = GREATEST(0, total_children_count - 1) \
         WHERE thread_id = $1",
    )
    .bind(parent_id)
    .execute(&mut **tx)
    .await?;

    let mut touched = vec![child_id, parent_id];
    touched.extend(recount_awaited_from_parent(tx, parent_id).await?);
    Ok(touched)
}

pub(crate) async fn propagate_blocking_change(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
    blocking_delta: i32,
    attention_delta: i32,
) -> Result<Vec<Uuid>, sqlx::Error> {
    if blocking_delta == 0 && attention_delta == 0 {
        return Ok(Vec::new());
    }
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "WITH RECURSIVE ancestors AS ( \
            SELECT parent_thread_id AS thread_id \
            FROM thread_summaries \
            WHERE thread_id = $1 AND parent_thread_id IS NOT NULL \
            UNION \
            SELECT t.parent_thread_id AS thread_id \
            FROM thread_summaries t \
            JOIN ancestors a ON t.thread_id = a.thread_id \
            WHERE t.parent_thread_id IS NOT NULL \
         ) \
         UPDATE thread_summaries \
         SET blocking_descendant_count = blocking_descendant_count + $2, \
             attention_descendant_count = attention_descendant_count + $3 \
         WHERE thread_id IN (SELECT thread_id FROM ancestors) \
         RETURNING thread_id",
    )
    .bind(thread_id)
    .bind(blocking_delta)
    .bind(attention_delta)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

impl EventBus {
    pub async fn recount_family_counts(
        &self,
        thread_id: Uuid,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut tx = self.pool.begin().await?;
        lock_parent_recount(&mut tx, thread_id).await?;
        recount_active_children(&mut tx, thread_id).await?;
        let mut touched = vec![thread_id];
        touched.extend(recount_awaited_from_parent(&mut tx, thread_id).await?);
        touched.extend(reconcile_descendant_counts_from(&mut tx, thread_id).await?);
        touched.sort();
        touched.dedup();
        let mut aggregates = Vec::with_capacity(touched.len());
        for id in &touched {
            if let Some(agg) = crate::core::store::fetch_thread_aggregate(&mut *tx, *id).await? {
                aggregates.push((*id, agg));
            }
        }
        tx.commit().await?;
        for (id, agg) in aggregates {
            let (active, total) = (agg.active_children_count, agg.total_children_count);
            self.send_children_count_event(id, active, total, Some(agg));
        }
        Ok(())
    }

    pub async fn store_archivable_sub_thread_counts(
        &self,
        counts: &[(Uuid, i64)],
    ) -> Result<Vec<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
        if counts.is_empty() {
            return Ok(Vec::new());
        }
        let (ids, values): (Vec<Uuid>, Vec<i64>) = counts.iter().copied().unzip();
        let mut tx = self.pool.begin().await?;
        let moved: Vec<Uuid> = sqlx::query_scalar(
            "UPDATE thread_summaries t SET archivable_sub_thread_count = c.n::int \
             FROM UNNEST($1::uuid[], $2::bigint[]) AS c(id, n) \
             WHERE t.thread_id = c.id AND t.archivable_sub_thread_count <> c.n::int \
             RETURNING t.thread_id",
        )
        .bind(&ids)
        .bind(&values)
        .fetch_all(&mut *tx)
        .await?;
        let mut aggregates = Vec::with_capacity(moved.len());
        for id in &moved {
            if let Some(agg) = crate::core::store::fetch_thread_aggregate(&mut *tx, *id).await? {
                aggregates.push((*id, agg));
            }
        }
        tx.commit().await?;
        for (id, agg) in aggregates {
            let (active, total) = (agg.active_children_count, agg.total_children_count);
            self.send_children_count_event(id, active, total, Some(agg));
        }
        Ok(moved)
    }

    /// INVARIANT: Recompute every parent's `active_children_count`,
    /// `awaited_children_count` and `total_children_count` from ground truth,
    /// and every `awaited_by_parent`.
    ///
    /// Called at engine startup to repair drift in either direction.
    /// Over-count: a child coding-agent session canceled before emitting
    /// `CodingAgentIdled` leaves the parent with a stale non-zero count.
    /// Under-count: recovery's synthetic
    /// `CodingAgentIdled{reason=engine_restart_interrupt}` decrements as if the
    /// child were terminal, but the child is only parked, and the user's
    /// Continue click re-increments via the projection. If the user restarts
    /// again before clicking Continue, the now-zero count stays stuck without
    /// this sweep (the projection's `+1` fires once per park/resume pair, not
    /// as a safety net for drifted rows). A `> 0` guard would skip exactly the
    /// rows that need repair.
    ///
    /// Uses the shared `active_thread_statuses()` predicate, the same one
    /// `reconcile_parent_active_children_count` uses in-tx. It filtered
    /// `status = 'running'` alone until 2026-08-05, which meant every boot
    /// recomputed a parent's count *without* a child parked on a question or a
    /// permission card, contradicting the in-tx reconcile. Question-parked
    /// threads are deliberately preserved across a restart and
    /// `UserQuestionAnswered` does not re-increment (by design, see the revive
    /// helper's doc), so that under-count persisted until some sibling terminal
    /// fired the reconcile.
    pub async fn rebuild_children_counts(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "WITH child_counts AS ( \
                 SELECT parent_thread_id, \
                        COUNT(*) FILTER (WHERE status = ANY($1))::int AS active, \
                        COUNT(*)::int AS total \
                 FROM thread_summaries \
                 WHERE parent_thread_id IS NOT NULL \
                 GROUP BY parent_thread_id \
             ) \
             UPDATE thread_summaries p \
             SET active_children_count = cc.active, \
                 total_children_count = cc.total \
             FROM child_counts cc \
             WHERE p.thread_id = cc.parent_thread_id \
               AND (p.active_children_count != cc.active \
                    OR p.total_children_count != cc.total)",
        )
        .bind(&crate::core::store::active_thread_statuses()[..])
        .execute(pool)
        .await?;
        sqlx::query(
            "UPDATE thread_summaries p \
             SET active_children_count = 0, total_children_count = 0 \
             WHERE (p.active_children_count <> 0 OR p.total_children_count <> 0) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM thread_summaries c WHERE c.parent_thread_id = p.thread_id \
               )",
        )
        .execute(pool)
        .await?;
        Self::rebuild_awaited_children(pool).await
    }

    /// INVARIANT: Recompute every `awaited_by_parent` and `awaited_children_count`. A
    /// row's flag reads its own count, and its count reads its children's
    /// flags. So it clears every flag, then recounts and sets flags in rounds
    /// until a round sets none.
    ///
    /// A round only ever sets flags, so the loop ends even over a corrupt
    /// `parent_thread_id` cycle. It never reads `depth`, which legacy rows
    /// hold wrong.
    async fn rebuild_awaited_children(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
        let awaited = &*AWAITED_BY_PARENT_SQL;
        let mut tx = pool.begin().await?;
        sqlx::query(
            "UPDATE thread_summaries SET awaited_by_parent = FALSE WHERE awaited_by_parent",
        )
        .execute(&mut *tx)
        .await?;
        loop {
            sqlx::query(
                "UPDATE thread_summaries p \
                 SET awaited_children_count = w.cnt \
                 FROM ( \
                     SELECT t.thread_id, COUNT(c.thread_id)::int AS cnt \
                     FROM thread_summaries t \
                     LEFT JOIN thread_summaries c \
                       ON c.parent_thread_id = t.thread_id AND c.awaited_by_parent \
                     GROUP BY t.thread_id \
                 ) w \
                 WHERE p.thread_id = w.thread_id AND p.awaited_children_count <> w.cnt",
            )
            .execute(&mut *tx)
            .await?;
            let set = sqlx::query(&format!(
                "UPDATE thread_summaries SET awaited_by_parent = TRUE \
                 WHERE NOT awaited_by_parent AND {awaited}"
            ))
            .bind(&crate::core::store::active_thread_statuses()[..])
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if set == 0 {
                break;
            }
        }
        tx.commit().await
    }

    /// INVARIANT: Recompute `thread_summaries.blocking_descendant_count` AND
    /// `thread_summaries.attention_descendant_count` for every row from
    /// scratch via a recursive CTE that walks each thread's transitive
    /// descendants. Counts those matching `is_blocking` and
    /// `is_attention_needing` respectively.
    ///
    /// Called at engine startup (`main.rs`) to reconcile counts that drifted
    /// while the engine was down — the recovery sweep flips stuck descendants
    /// to idle via direct UPDATEs that bypass the projection's sampling
    /// wrapper, so the materialized ancestor counts must be recomputed once
    /// from scratch. The incremental delta propagation in
    /// `update_thread_projection` keeps the counts current during live events,
    /// but a post-restart drift (or any future wipe-and-replay rebuild of
    /// `thread_summaries` from the event log) needs this same one-shot
    /// recompute — the one the initial backfills (in
    /// `20260517193910_add_blocking_descendant_count_to_thread_summaries.sql`
    /// and `20260522091904_add_attention_descendant_count.sql`) perform.
    /// Predicates match `is_blocking` / `is_attention_needing` in
    /// `thread_lifecycle.rs`.
    pub async fn rebuild_blocking_descendant_count(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
        sqlx::query(&format!(
            "WITH RECURSIVE descendants AS ( \
                SELECT t.thread_id AS root_id, \
                       c.thread_id, c.status, c.archive_state, \
                       c.coding_agent_change_state = '{proposed}' AS is_proposed, c.is_coding_agent, \
                       c.coding_agent_is_external_repo, c.is_stopped_child, \
                       c.hold IS NOT NULL AS is_held \
                FROM thread_summaries t \
                JOIN thread_summaries c ON c.parent_thread_id = t.thread_id \
                UNION \
                SELECT d.root_id, \
                       c.thread_id, c.status, c.archive_state, \
                       c.coding_agent_change_state = '{proposed}' AS is_proposed, c.is_coding_agent, \
                       c.coding_agent_is_external_repo, c.is_stopped_child, \
                       c.hold IS NOT NULL AS is_held \
                FROM descendants d \
                JOIN thread_summaries c ON c.parent_thread_id = d.thread_id \
             ) \
             UPDATE thread_summaries u \
             SET blocking_descendant_count = COALESCE(sub.blocking_cnt, 0), \
                 attention_descendant_count = COALESCE(sub.attention_cnt, 0) \
             FROM ( \
                 SELECT d.root_id, \
                        COUNT(*) FILTER (WHERE {blocking}) AS blocking_cnt, \
                        COUNT(*) FILTER (WHERE {attention}) AS attention_cnt \
                 FROM descendants d \
                 GROUP BY d.root_id \
             ) sub \
             WHERE u.thread_id = sub.root_id",
            blocking = &*BLOCKING_DESCENDANT_FILTER,
            attention = &*ATTENTION_DESCENDANT_FILTER,
            proposed = ChangeStateKind::Proposed.as_str(),
        ))
        .execute(pool)
        .await?;
        // INVARIANT: Reset any row that no longer appears as a root (e.g. all its
        // descendants got pruned). The UPDATE…FROM above only touches rows
        // with at least one descendant; rows with zero descendants must drop
        // their stale counts to 0 explicitly.
        sqlx::query(
            "UPDATE thread_summaries u \
             SET blocking_descendant_count = 0, \
                 attention_descendant_count = 0 \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM thread_summaries c WHERE c.parent_thread_id = u.thread_id \
             ) AND (u.blocking_descendant_count <> 0 OR u.attention_descendant_count <> 0)",
        )
        .execute(pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    async fn insert_thread(
        pool: &sqlx::PgPool,
        parent: Option<Uuid>,
        status: &str,
        live_waits: i32,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, parent_thread_id, status, live_event_wait_count) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(parent)
        .bind(status)
        .bind(live_waits)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    async fn counts(pool: &sqlx::PgPool, parent: Uuid) -> (i32, i32) {
        sqlx::query_as(
            "SELECT active_children_count, awaited_children_count \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(parent)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn overlap(
        pool: &sqlx::PgPool,
        first: &str,
        first_child: Uuid,
        second: &str,
        second_child: Uuid,
        reconcile_active: bool,
    ) {
        let mut a = pool.begin().await.unwrap();
        sqlx::query(first)
            .bind(first_child)
            .execute(&mut *a)
            .await
            .unwrap();
        if reconcile_active {
            reconcile_parent_active_children_count(&mut a, first_child)
                .await
                .unwrap();
        } else {
            reconcile_awaited_from(&mut a, first_child).await.unwrap();
        }

        let pool_b = pool.clone();
        let second = second.to_string();
        let b = tokio::spawn(async move {
            let mut b = pool_b.begin().await.unwrap();
            sqlx::query(&second)
                .bind(second_child)
                .execute(&mut *b)
                .await
                .unwrap();
            if reconcile_active {
                reconcile_parent_active_children_count(&mut b, second_child)
                    .await
                    .unwrap();
            } else {
                reconcile_awaited_from(&mut b, second_child).await.unwrap();
            }
            b.commit().await.unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !b.is_finished(),
            "the second recount must wait for the first to commit, or this test proves nothing"
        );
        a.commit().await.unwrap();
        b.await.unwrap();
    }

    #[tokio::test]
    async fn overlapping_wait_changes_leave_the_true_awaited_count() {
        let (pool, db_name) = setup_test_db().await;
        let parent = insert_thread(&pool, None, "idle", 0).await;
        let arming = insert_thread(&pool, Some(parent), "idle", 0).await;
        let ending = insert_thread(&pool, Some(parent), "idle", 1).await;
        sqlx::query("UPDATE thread_summaries SET awaited_by_parent = TRUE WHERE thread_id = $1")
            .bind(ending)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE thread_summaries SET awaited_children_count = 1 WHERE thread_id = $1")
            .bind(parent)
            .execute(&pool)
            .await
            .unwrap();

        overlap(
            &pool,
            "UPDATE thread_summaries SET live_event_wait_count = 1 WHERE thread_id = $1",
            arming,
            "UPDATE thread_summaries SET live_event_wait_count = 0 WHERE thread_id = $1",
            ending,
            false,
        )
        .await;

        assert_eq!(counts(&pool, parent).await.1, 1);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_climb_racing_a_sibling_terminal_leaves_the_true_awaited_count() {
        let (pool, db_name) = setup_test_db().await;
        let top = insert_thread(&pool, None, "idle", 0).await;
        let child = insert_thread(&pool, Some(top), "idle", 0).await;
        let grandchild = insert_thread(&pool, Some(child), "running", 0).await;
        let sibling = insert_thread(&pool, Some(top), "running", 0).await;
        sqlx::query(
            "UPDATE thread_summaries SET awaited_by_parent = TRUE WHERE thread_id = ANY($1)",
        )
        .bind([child, grandchild, sibling])
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE thread_summaries SET awaited_children_count = \
               CASE WHEN thread_id = $1 THEN 2 ELSE 1 END \
             WHERE thread_id = ANY(ARRAY[$1, $2])",
        )
        .bind(top)
        .bind(child)
        .execute(&pool)
        .await
        .unwrap();

        let idle = "UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1";
        overlap(&pool, idle, grandchild, idle, sibling, false).await;

        assert_eq!(counts(&pool, child).await.1, 0);
        assert_eq!(counts(&pool, top).await.1, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_stopped_childs_card_racing_an_uncles_terminal_does_not_deadlock() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let grandparent = insert_thread(&pool, None, "idle", 0).await;
        let parent = insert_thread(&pool, Some(grandparent), "running", 0).await;
        let stopped = insert_thread(&pool, Some(parent), "idle", 0).await;
        let uncle = insert_thread(&pool, Some(grandparent), "running", 0).await;
        sqlx::query(
            "UPDATE thread_summaries SET parent_callback_pending = TRUE, is_stopped_child = TRUE \
             WHERE thread_id = $1",
        )
        .bind(stopped)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE thread_summaries SET awaited_by_parent = TRUE WHERE thread_id = $1")
            .bind(uncle)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE thread_summaries SET attention_descendant_count = 1, \
               active_children_count = CASE WHEN thread_id = $1 THEN 1 ELSE 0 END, \
               awaited_children_count = CASE WHEN thread_id = $1 THEN 1 ELSE 0 END \
             WHERE thread_id = ANY(ARRAY[$1, $2])",
        )
        .bind(grandparent)
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();

        let mut uncle_tx = pool.begin().await.unwrap();
        sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1")
            .bind(uncle)
            .execute(&mut *uncle_tx)
            .await
            .unwrap();
        hold_parent_recount_lock(&mut uncle_tx, uncle)
            .await
            .unwrap();

        let card = tokio::spawn({
            let pool = pool.clone();
            async move {
                let mut tx = pool.begin().await.unwrap();
                bus.update_thread_projection(
                    &mut tx,
                    parent,
                    &crate::engine::thread_events::ThreadEvent::ChildThreadCompleted {
                        child_thread_id: stopped,
                        child_thread_title: None,
                        status: crate::engine::thread_events::ChildCompletionStatus::Interrupted,
                        summary: "stopped".into(),
                        pending_change_ids: vec![],
                        sub_thread_pending_changes: vec![],
                        integrated_branch: None,
                    },
                    &crate::engine::thread_events::EventMeta::NONE,
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
            }
        });
        crate::test_support::wait_until_blocked_on_locks(&pool, 1).await;

        reconcile_parent_active_children_count(&mut uncle_tx, uncle)
            .await
            .unwrap();
        reconcile_awaited_from(&mut uncle_tx, uncle).await.unwrap();
        uncle_tx.commit().await.unwrap();
        card.await.unwrap();

        assert_eq!(counts(&pool, grandparent).await, (1, 1));
        let attention: i32 = sqlx::query_scalar(
            "SELECT attention_descendant_count FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(grandparent)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(attention, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn overlapping_terminals_leave_the_true_active_count() {
        let (pool, db_name) = setup_test_db().await;
        let parent = insert_thread(&pool, None, "idle", 0).await;
        let first = insert_thread(&pool, Some(parent), "running", 0).await;
        let second = insert_thread(&pool, Some(parent), "running", 0).await;
        sqlx::query("UPDATE thread_summaries SET active_children_count = 2 WHERE thread_id = $1")
            .bind(parent)
            .execute(&pool)
            .await
            .unwrap();

        let idle = "UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1";
        overlap(&pool, idle, first, idle, second, true).await;

        assert_eq!(counts(&pool, parent).await.0, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

#[cfg(test)]
mod archive_input_sample_tests {
    use super::ArchiveInputSample;
    use uuid::Uuid;

    fn sample(parent: Option<Uuid>, status: &str) -> ArchiveInputSample {
        ArchiveInputSample {
            parent_thread_id: parent,
            status: status.into(),
            archive_state: "inbox".into(),
            state: "active".into(),
            is_saved: false,
            change_state: "none".into(),
            has_output: true,
            waits_on_event: false,
            has_draft: false,
        }
    }

    #[test]
    fn an_unchanged_sample_names_no_anchor() {
        let parent = Some(Uuid::new_v4());
        let s = sample(parent, "idle");
        assert!(ArchiveInputSample::recount_anchors(Some(&s), Some(&s), false).is_empty());
    }

    #[test]
    fn a_flip_or_an_off_row_input_names_the_parent() {
        let parent = Uuid::new_v4();
        let (idle, running) = (
            sample(Some(parent), "idle"),
            sample(Some(parent), "running"),
        );
        assert_eq!(
            ArchiveInputSample::recount_anchors(Some(&idle), Some(&running), false),
            vec![parent]
        );
        assert_eq!(
            ArchiveInputSample::recount_anchors(Some(&idle), Some(&idle), true),
            vec![parent]
        );
    }

    #[test]
    fn a_detach_names_the_former_parent() {
        let former = Uuid::new_v4();
        let (before, after) = (sample(Some(former), "idle"), sample(None, "idle"));
        assert_eq!(
            ArchiveInputSample::recount_anchors(Some(&before), Some(&after), false),
            vec![former]
        );
    }

    #[test]
    fn a_top_thread_names_no_anchor() {
        let (idle, running) = (sample(None, "idle"), sample(None, "running"));
        assert!(ArchiveInputSample::recount_anchors(Some(&idle), Some(&running), false).is_empty());
    }
}
