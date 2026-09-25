//! `thread_summaries` blocking/attention propagation helpers for the
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
    is_attention_needing, is_blocking, ArchiveState, ThreadStatus, ThreadType,
};

/// Sample of the `thread_summaries` columns that feed `is_blocking` and
/// `is_attention_needing`.
/// Loaded before and after every projection update so the projection can
/// detect a flip and propagate the delta to ancestors via
/// `blocking_descendant_count`.
pub(crate) struct BlockingSample {
    thread_type: ThreadType,
    status: ThreadStatus,
    archive_state: ArchiveState,
    has_pending_changes: bool,
    is_external_repo: bool,
    is_stopped_child: bool,
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
        )
    }
}

/// Read the blocking- and attention-relevant columns for `thread_id`. Returns
/// `Ok(None)` when the row doesn't exist (e.g. a thread-start event running
/// before its own INSERT). The wrapper that calls this treats "no row" as
/// "was not blocking", so the freshly-inserted row's first sample propagates
/// a clean +1 if blocking.
pub(crate) async fn load_blocking_sample(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<Option<BlockingSample>, sqlx::Error> {
    let row: Option<(bool, String, String, bool, bool, bool)> = sqlx::query_as(
        "SELECT is_coding_agent, status, archive_state, coding_agent_proposed, \
                coding_agent_is_external_repo, is_stopped_child \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(
        |(
            is_coding_agent,
            status_str,
            archive_state_str,
            coding_agent_proposed,
            coding_agent_is_external_repo,
            is_stopped_child,
        )| {
            BlockingSample {
                thread_type: if is_coding_agent {
                    ThreadType::CodingAgent
                } else {
                    ThreadType::Chat
                },
                status: ThreadStatus::parse(&status_str),
                archive_state: ArchiveState::parse(&archive_state_str),
                has_pending_changes: coding_agent_proposed,
                is_external_repo: coding_agent_is_external_repo,
                is_stopped_child,
            }
        },
    ))
}

/// Recompute the direct parent's `active_children_count` from children
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
/// `reconcile_blocking_descendant_count_for_ancestors` (`status IN (...)`).
///
/// Single in-tx writer of `active_children_count` on the terminal-event
/// path. Each terminal-event arm in `update_thread_projection`
/// (CodingAgentIdled, ResponseGenerated, ResponseFailed, ResponseCanceled,
/// non-transient ResponseAborted, non-transient SessionEnded) calls this
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

/// Recount `parent_id`'s `active_children_count`, then its
/// `waiting_children_count`, from its children. The caller holds the parent's
/// recount lock.
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
    // A child that just left the in-flight set may now be a waiting child.
    recount_waiting_children(tx, parent_id).await?;
    Ok(())
}

/// Advisory-lock class for `hold_parent_recount_lock`. The two-key form keeps
/// it apart from the one-key locks elsewhere in the engine.
const PARENT_RECOUNT_LOCK_CLASS: i32 = 0x6368_6c64;

/// Serialize recounts of one parent and return its id, or `None` for a top
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
/// **Every writer of either count takes it BEFORE writing the parent row**,
/// the two `+1` paths included. Taking it after is a lock-order inversion
/// against a sibling's recount, and Postgres aborts one of the two events.
pub(crate) async fn hold_parent_recount_lock(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let parent_id: Option<Uuid> =
        sqlx::query_scalar("SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(child_id)
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
    let Some(pid) = parent_id else {
        return Ok(None);
    };
    lock_parent_recount(tx, pid).await?;
    Ok(Some(pid))
}

/// Take `parent_id`'s recount lock. [`hold_parent_recount_lock`] says why, and
/// when to take it.
async fn lock_parent_recount(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    parent_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2::text))")
        .bind(PARENT_RECOUNT_LOCK_CLASS)
        .bind(parent_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Which children `waiting_children_count` counts, with the in-flight statuses
/// bound as `$1`: not in flight, and holding a live event wait. Not in flight
/// keeps it disjoint from `active_children_count`, so the two can be added.
const WAITING_CHILD_FILTER: &str = "live_event_wait_count > 0 AND status <> ALL($1)";

/// Recompute the direct parent's `waiting_children_count` from ground truth.
/// Returns the parent's id only when the count moved. An `EventWait*` arm then
/// rebroadcasts the parent, and a top thread's waits broadcast nothing extra.
///
/// Callers are every place either input can move: the child's own `EventWait*`
/// arms (its wait count) and the two active-count helpers (its status).
pub(crate) async fn reconcile_parent_waiting_children_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let Some(pid) = hold_parent_recount_lock(tx, child_id).await? else {
        return Ok(None);
    };
    recount_waiting_children(tx, pid).await
}

/// Recount `parent_id`'s `waiting_children_count`, returning the id only when
/// the count moved. The caller holds the parent's recount lock.
async fn recount_waiting_children(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    parent_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "UPDATE thread_summaries p \
         SET waiting_children_count = w.cnt \
         FROM ( \
             SELECT COUNT(*)::int AS cnt FROM thread_summaries \
             WHERE parent_thread_id = $2 AND {WAITING_CHILD_FILTER} \
         ) w \
         WHERE p.thread_id = $2 AND p.waiting_children_count <> w.cnt \
         RETURNING p.thread_id"
    ))
    .bind(&crate::core::store::active_thread_statuses()[..])
    .bind(parent_id)
    .fetch_optional(&mut **tx)
    .await
}

/// Run both Apply/Discard reconciles back-to-back, accumulating affected
/// ancestor IDs into `extra_ancestors` for the SSE rebroadcast path.
/// Both proposal-end events (`ChangeApplied`, `ChangeDiscarded`) need
/// the same recovery work — extracted to keep the two arms in lockstep.
pub(crate) async fn reconcile_proposal_lifecycle_end(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
    extra_ancestors: &mut Vec<Uuid>,
) -> Result<(), sqlx::Error> {
    if let Some(parent_id) = reconcile_parent_active_children_count(tx, child_id).await? {
        extra_ancestors.push(parent_id);
    }
    extra_ancestors.extend(reconcile_blocking_descendant_count_for_ancestors(tx, child_id).await?);
    Ok(())
}

/// Arm the child's `parent_callback_pending` marker because a **start event**
/// arrived: `MessageReceived`, `CodingAgentUserMessageSent`,
/// `UserPromptInjected`, a non-empty `CodingAgentPromptSent`, or
/// `ContinuationRequested`. That is the set `preserving_verdict`
/// (`event_bus/mod.rs`) already names as "new work was actually requested".
///
/// The marker means "the parent has NOT yet been told about the child's
/// **current** turn", so a start event owes the parent a fresh card while a
/// mere extra `CodingAgentIdled` (auto-harden, a background agent) does not.
/// Keeping that distinction is what leaves the dedup guard in
/// `notify_parent_if_child` doing its stated job.
///
/// Deliberately NOT a start event: the **empty** `CodingAgentPromptSent`
/// question-resume marker (`agent_question::emit_resume_marker_for_cc_answer`).
/// It exists only so the timeline shows a Thinking step and asserts no new
/// agent intent, which is why its own projection arm already skips the status
/// write. Arming the marker there would re-open the parent callback after a
/// card was already sent, so the very extra idle the dedup guard exists for
/// would produce a spurious second card. Pinned by
/// `an_empty_resume_marker_is_not_a_start_event`.
///
/// This is gated differently from the re-increment below, which is why the two
/// are separate operations: the marker does not care whether the child was in
/// flight, only that new work was requested. A parked child that receives a
/// follow-up owes its parent a card for the resulting turn even though its
/// place on the parent's counter was never given up.
///
/// `AND parent_thread_id IS NOT NULL` keeps the write off a parentless row,
/// which under this polarity would otherwise be told it owes some parent a
/// card. The storage default is FALSE for exactly that reason.
///
/// New work also ends a *stopped child*: the turn it starts is what will
/// report, so `is_stopped_child` clears in the same write (ADR 0252).
pub(crate) async fn mark_parent_callback_pending(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE thread_summaries SET parent_callback_pending = TRUE, is_stopped_child = FALSE \
         WHERE thread_id = $1 AND parent_thread_id IS NOT NULL",
    )
    .bind(child_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Make `child_id` a *stopped child*, because a user Stop just ended its turn
/// (ADR 0252). Only a child still owed a card qualifies: with the marker
/// already clear, the parent has heard about this turn and is owed nothing.
/// A top-thread never matches, having no parent.
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

/// Settle what `child_id` owed its parent, because the parent's
/// `ChildThreadCompleted` for it just landed. Clears both the marker and
/// `is_stopped_child`, and returns whether the child WAS stopped.
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
         SET parent_callback_pending = FALSE, is_stopped_child = FALSE \
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

/// Bump the direct parent's `active_children_count` when an event flipped
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
    // A revived child still holding a wait stops counting as waiting.
    reconcile_parent_waiting_children_count(tx, child_id).await?;
    Ok(Some(pid))
}

/// Reconcile `blocking_descendant_count` AND `attention_descendant_count`
/// for every ancestor of `child_id` by recomputing the counts from the
/// ancestor's transitive descendants using the `is_blocking` and
/// `is_attention_needing` predicates respectively. Mirrors
/// `rebuild_blocking_descendant_count` (and the migration backfills) for
/// both columns. Returns the list of ancestors whose row was touched —
/// suitable for the SSE rebroadcast path.
///
/// Defense in depth for the `ChangeApplied` / `ChangeDiscarded` arms:
/// the function-boundary `propagate_blocking_change` already handles the
/// child's own predicate flips on Apply/Discard. Recomputing the full
/// ancestor chain here catches any latent drift from prior missed
/// propagates (e.g. an event the apply replays over) and matches the
/// user-stated invariant that resolving a proposal must leave parent
/// counters consistent without waiting for a restart.
///
/// One SQL roundtrip, both columns updated in lockstep — keeps them from
/// desyncing under concurrent emits.
pub(crate) async fn reconcile_blocking_descendant_count_for_ancestors(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    child_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    reconcile_descendant_counts(
        tx,
        "SELECT parent_thread_id AS thread_id FROM thread_summaries \
         WHERE thread_id = $1 AND parent_thread_id IS NOT NULL",
        child_id,
    )
    .await
}

/// [`reconcile_blocking_descendant_count_for_ancestors`], anchored on
/// `thread_id` itself as well as its ancestors. For a write that changed WHO
/// `thread_id`'s descendants are, rather than one descendant's predicates.
pub(crate) async fn reconcile_descendant_counts_from(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    reconcile_descendant_counts(tx, "SELECT $1::uuid AS thread_id", thread_id).await
}

/// `is_blocking` in SQL, over a descendant row aliased `d`. Shared by the
/// in-tx reconcile and the boot rebuild, so the two cannot drift apart.
const BLOCKING_DESCENDANT_FILTER: &str = "d.status IN ('running','waiting_for_user_answer') \
     OR (d.archive_state <> 'archived' \
         AND d.coding_agent_proposed AND d.is_coding_agent \
         AND NOT d.coding_agent_is_external_repo)";

/// `is_attention_needing` in SQL, over a descendant row aliased `d`.
const ATTENTION_DESCENDANT_FILTER: &str = "d.status = 'waiting_for_user_answer' \
     OR (d.archive_state <> 'archived' \
         AND ((d.coding_agent_proposed AND d.is_coding_agent \
               AND NOT d.coding_agent_is_external_repo) \
              OR d.is_stopped_child))";

/// The shared recompute. `base` seeds the rows to reconcile from `$1`, and the
/// walk then climbs from those to every ancestor.
async fn reconcile_descendant_counts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    base: &str,
    anchor: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(&format!(
        "WITH RECURSIVE ancestors AS ( \
            {base} \
            UNION \
            SELECT t.parent_thread_id AS thread_id \
            FROM thread_summaries t \
            JOIN ancestors a ON t.thread_id = a.thread_id \
            WHERE t.parent_thread_id IS NOT NULL \
         ), \
         descendants AS ( \
            SELECT a.thread_id AS root_id, c.thread_id, c.status, c.archive_state, \
                   c.coding_agent_proposed, c.is_coding_agent, c.coding_agent_is_external_repo, \
                   c.is_stopped_child \
            FROM ancestors a \
            JOIN thread_summaries c ON c.parent_thread_id = a.thread_id \
            UNION \
            SELECT d.root_id, c.thread_id, c.status, c.archive_state, \
                   c.coding_agent_proposed, c.is_coding_agent, c.coding_agent_is_external_repo, \
                   c.is_stopped_child \
            FROM descendants d \
            JOIN thread_summaries c ON c.parent_thread_id = d.thread_id \
         ), \
         new_counts AS ( \
            SELECT a.thread_id AS root_id, \
                   COALESCE(COUNT(*) FILTER (WHERE {BLOCKING_DESCENDANT_FILTER}), 0)::int \
                       AS blocking_cnt, \
                   COALESCE(COUNT(*) FILTER (WHERE {ATTENTION_DESCENDANT_FILTER}), 0)::int \
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
         RETURNING u.thread_id"
    ))
    .bind(anchor)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Cut the edge from `parent_id` to `child_id`, for `ChildThreadDetached`
/// (ADR 0278). Returns every row whose aggregate may have moved, the child
/// included, for the SSE rebroadcast.
///
/// Returns nothing when `child_id` is not `parent_id`'s child, so an event
/// applied twice changes nothing the second time.
///
/// The order is load-bearing:
/// 1. The emit's Validate phase already locked the subtree, deepest first
///    (`lock_edge_for_detach`). The parent's recount lock comes after, the
///    order a child's own terminal takes them in.
/// 2. Recount after the cut, anchored on the parent id. The child-anchored
///    helpers find the parent from the child's row, which no longer names it.
/// 3. Rebase `depth` over the whole subtree. The recursion guard reads it, so
///    a stale depth would refuse spawns levels too early.
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
    touched.extend(reconcile_descendant_counts_from(tx, parent_id).await?);
    Ok(touched)
}

/// Walk ancestors of `thread_id` via `parent_thread_id` and apply the
/// `blocking_delta` / `attention_delta` to their `blocking_descendant_count`
/// and `attention_descendant_count` columns respectively. No-op when
/// `thread_id` has no parent or when BOTH deltas are 0 (early exit avoids
/// a pointless SQL roundtrip on the common case where the event leaves both
/// predicates unchanged).
///
/// Returns the list of ancestor `thread_id`s whose row was touched. The
/// caller uses this to rebroadcast each ancestor's aggregate over SSE after
/// the transaction commits — without that, the DB columns move but the
/// frontend's `meta.blockingDescendantCount` / `meta.attentionDescendantCount`
/// stay stale until a full `/api/v1/threads` reload. Empty vec when both deltas
/// are 0 (no SQL run) or the thread has no ancestors.
///
/// One SQL roundtrip, two column updates. Keeps both counters atomic: a
/// crash mid-tx can't desync `blocking` and `attention`.
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
    /// Recompute every parent's `active_children_count`,
    /// `waiting_children_count` and `total_children_count` from ground truth.
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
        sqlx::query(&format!(
            "WITH child_counts AS ( \
                 SELECT parent_thread_id, \
                        COUNT(*) FILTER (WHERE status = ANY($1))::int AS active, \
                        COUNT(*) FILTER (WHERE {WAITING_CHILD_FILTER})::int AS waiting, \
                        COUNT(*)::int AS total \
                 FROM thread_summaries \
                 WHERE parent_thread_id IS NOT NULL \
                 GROUP BY parent_thread_id \
             ) \
             UPDATE thread_summaries p \
             SET active_children_count = cc.active, \
                 waiting_children_count = cc.waiting, \
                 total_children_count = cc.total \
             FROM child_counts cc \
             WHERE p.thread_id = cc.parent_thread_id \
               AND (p.active_children_count != cc.active \
                    OR p.waiting_children_count != cc.waiting \
                    OR p.total_children_count != cc.total)"
        ))
        .bind(&crate::core::store::active_thread_statuses()[..])
        .execute(pool)
        .await?;
        // Reset a row that is nobody's parent any more. The UPDATE above only
        // touches rows still named by some child, so a thread whose children
        // were all deleted or moved to top level keeps its stale counts. A
        // stale count of 1 leaves the parent's own Archive and Delete hidden
        // for good. Mirrors the second statement in
        // `rebuild_blocking_descendant_count`.
        sqlx::query(
            "UPDATE thread_summaries p \
             SET active_children_count = 0, waiting_children_count = 0, \
                 total_children_count = 0 \
             WHERE (p.active_children_count <> 0 OR p.waiting_children_count <> 0 \
                    OR p.total_children_count <> 0) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM thread_summaries c WHERE c.parent_thread_id = p.thread_id \
               )",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Recompute `thread_summaries.blocking_descendant_count` AND
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
                       c.coding_agent_proposed, c.is_coding_agent, \
                       c.coding_agent_is_external_repo, c.is_stopped_child \
                FROM thread_summaries t \
                JOIN thread_summaries c ON c.parent_thread_id = t.thread_id \
                UNION \
                SELECT d.root_id, \
                       c.thread_id, c.status, c.archive_state, \
                       c.coding_agent_proposed, c.is_coding_agent, \
                       c.coding_agent_is_external_repo, c.is_stopped_child \
                FROM descendants d \
                JOIN thread_summaries c ON c.parent_thread_id = d.thread_id \
             ) \
             UPDATE thread_summaries u \
             SET blocking_descendant_count = COALESCE(sub.blocking_cnt, 0), \
                 attention_descendant_count = COALESCE(sub.attention_cnt, 0) \
             FROM ( \
                 SELECT d.root_id, \
                        COUNT(*) FILTER (WHERE {BLOCKING_DESCENDANT_FILTER}) AS blocking_cnt, \
                        COUNT(*) FILTER (WHERE {ATTENTION_DESCENDANT_FILTER}) AS attention_cnt \
                 FROM descendants d \
                 GROUP BY d.root_id \
             ) sub \
             WHERE u.thread_id = sub.root_id",
        ))
        .execute(pool)
        .await?;
        // Reset any row that no longer appears as a root (e.g. all its
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
    //! Two siblings recounting their parent in overlapping transactions. The
    //! later transaction must count after the earlier one commits, or it stores
    //! a stale count over the right one.

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
            "SELECT active_children_count, waiting_children_count \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(parent)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Run `first` in one transaction and hold it open. Start `second` in
    /// another, give it time to reach the parent, then commit `first`.
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
            reconcile_parent_waiting_children_count(&mut a, first_child)
                .await
                .unwrap();
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
                reconcile_parent_waiting_children_count(&mut b, second_child)
                    .await
                    .unwrap();
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

    /// One child arms a wait while its sibling's wait ends. The truth is one
    /// waiting child, and the parent must not read idle.
    #[tokio::test]
    async fn overlapping_wait_changes_leave_the_true_waiting_count() {
        let (pool, db_name) = setup_test_db().await;
        let parent = insert_thread(&pool, None, "idle", 0).await;
        let arming = insert_thread(&pool, Some(parent), "idle", 0).await;
        let ending = insert_thread(&pool, Some(parent), "idle", 1).await;
        sqlx::query("UPDATE thread_summaries SET waiting_children_count = 1 WHERE thread_id = $1")
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

    /// Two running children finish at once. The truth is none active, and the
    /// parent must not stay Waiting on a finished pair.
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
