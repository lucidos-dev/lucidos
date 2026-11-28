use uuid::Uuid;

use super::super::{
    branch_work_sql, keeping_status_across_redirect, preserving_verdict, quoted_list, BusEvent,
    EventBus, CLEAR_CODING_AGENT_FLAGS, STATUS_FROM_PROPOSED_CHANGE,
};
use super::propagation::{
    detach_child_from_parent, hold_parent_recount_lock, load_blocking_sample,
    mark_parent_callback_pending, mark_stopped_child, parent_of, propagate_blocking_change,
    reconcile_awaited_from, reconcile_descendant_counts_from,
    reconcile_parent_active_children_count, reconcile_proposal_lifecycle_end,
    reincrement_parent_active_count_if_revived, settle_parent_callback,
};
use crate::core::store::integrated::queued_spawn_is_integrated;
use crate::core::store::{
    ComposeReport, ComposeReportRow, EventWaitSummary, LegacyInitiator, COMPOSE_REPORT_COLS,
};
use crate::engine::thread_events::{
    ActorMode, CancelCause, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::thread_lifecycle::{
    resolve_transition, ArchiveState, ChangeStateKind, ThreadStatus, WAITING_FOR_USER_ANSWER_EVENTS,
};

const IDLE: &str = ThreadStatus::Idle.sql_literal();
const RUNNING: &str = ThreadStatus::Running.sql_literal();
const WAITING_FOR_USER_ANSWER: &str = ThreadStatus::WaitingForUserAnswer.sql_literal();

const PROJECT_HOLD_SUMMARY_SQL: &str = "\
    UPDATE thread_summaries SET hold = ( \
        SELECT CASE WHEN e.event_type IN ('ThreadHoldPlaced', 'AutomationPaused') \
            THEN jsonb_strip_nulls(jsonb_build_object( \
                'reason', COALESCE(e.payload->>'reason', ''), \
                'root_thread_id', CASE WHEN e.event_type = 'ThreadHoldPlaced' \
                    THEN COALESCE(e.payload->>'root_thread_id', e.thread_id::text) \
                    ELSE e.thread_id::text END, \
                'placed_at', to_jsonb(e.created), \
                'placed_by', e.payload->'actor')) END \
        FROM events e \
        WHERE e.thread_id = $1 AND e.event_type = ANY($2) \
        ORDER BY e.sequence DESC LIMIT 1) \
    WHERE thread_id = $1";

async fn project_hold_summary(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<(), sqlx::Error> {
    use crate::core::store::thread_hold::HOLD_EVENT_TYPES;
    sqlx::query(PROJECT_HOLD_SUMMARY_SQL)
        .bind(thread_id)
        .bind(&HOLD_EVENT_TYPES[..])
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn status_after_session_bookkeeping(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<String, sqlx::Error> {
    let is_coding_agent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1 AND is_coding_agent)",
    )
    .bind(thread_id)
    .fetch_one(&mut **tx)
    .await?;
    if !is_coding_agent {
        return Ok(keeping_status_across_redirect(&preserving_verdict(
            STATUS_FROM_PROPOSED_CHANGE,
        )));
    }
    let target = if crate::engine::pending_handoff::successor_to_start_on(&mut *tx, thread_id)
        .await?
        .is_some()
    {
        RUNNING
    } else {
        STATUS_FROM_PROPOSED_CHANGE
    };
    Ok(keeping_status_across_redirect(&preserving_verdict(
        &format!(
            "CASE WHEN status = {WAITING_FOR_USER_ANSWER} AND {} THEN status ELSE {target} END",
            crate::engine::handoff_sql::pending_handoff_sql("$1"),
        ),
    )))
}

fn tool_result_keeps_park_sql() -> String {
    format!(
        "(CASE WHEN status = {WAITING_FOR_USER_ANSWER} THEN NOT EXISTS ( \
           SELECT 1 FROM (SELECT event_type, payload FROM events \
                          WHERE thread_id = $1 AND event_type IN ({parks}) \
                          ORDER BY sequence DESC LIMIT 1) q \
           WHERE q.event_type = 'UserQuestionAsked' AND {own}) \
         ELSE FALSE END)",
        parks = quoted_list(WAITING_FOR_USER_ANSWER_EVENTS.iter().copied()),
        own = ThreadEvent::question_owns_result_id_sql("q", "$2"),
    )
}

async fn a_call_makes_the_thread_real(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<Vec<BusEvent>, sqlx::Error> {
    let promoted: Option<ComposeReportRow> = sqlx::query_as(&format!(
        "UPDATE thread_summaries \
         SET state = 'active', \
             compose_text = '', \
             compose_images = '[]'::jsonb, \
             compose_mode = NULL, \
             compose_selection = NULL, \
             compose_restorations = '[]'::jsonb, \
             compose_epoch = compose_epoch + 1 \
         WHERE thread_id = $1 AND state = 'composing' \
         RETURNING {COMPOSE_REPORT_COLS}"
    ))
    .bind(thread_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(promoted
        .map(|row| ComposeReport::from(row).broadcast(thread_id, None))
        .into_iter()
        .collect())
}

async fn write_thread_title(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
    title: &str,
) -> Result<bool, sqlx::Error> {
    let applied = sqlx::query("UPDATE thread_summaries SET title = $2 WHERE thread_id = $1")
        .bind(thread_id)
        .bind(title)
        .execute(&mut **tx)
        .await?;
    Ok(applied.rows_affected() > 0)
}

/// INVARIANT: `live_event_waits` with the wait bound as `$2` removed, as a scalar
/// expression over the row being updated. Shared by all four `EventWait*` arms:
/// the arm filters before appending, so a replayed start replaces rather than
/// duplicates, and each resolution is exactly this filter on its own.
const EVENT_WAIT_WITHOUT: &str = "COALESCE((SELECT jsonb_agg(w) \
     FROM jsonb_array_elements(live_event_waits) w \
     WHERE w->>'wait_id' <> $2), '[]'::jsonb)";

/// INVARIANT: One `EventWait*` projection statement. `waits` is the new array expression;
/// `extra_set` is whatever timestamp this arm owns beyond `last_activity`.
///
/// **`live_event_wait_count` is DERIVED from the array's length, never
/// incremented or decremented.** That is what makes the count and the list
/// unable to disagree, which is the whole reason the list is stored at all. Two
/// arithmetic expressions kept side by side would still drift. A resolution
/// naming an absent `wait_id` used to decrement the count alone, putting the
/// Waiting dot out under a panel still showing a wait. Deriving also retires
/// the old `GREATEST(0, ...)` floor, since an array length cannot go negative.
///
/// **`waits` is interpolated TWICE on purpose, and a CTE here would be a
/// lost-update bug.** Both copies read the row's own `live_event_waits`, so
/// Postgres evaluates them against one row version and they always agree.
/// Reading the array into a CTE and joining it back instead pins it to the
/// statement's snapshot, which a `SET` expression re-read never is: under READ
/// COMMITTED the second writer re-evaluates its `SET` list against the row the
/// first writer left, exactly as `count + 1` used to. Losing that would let an
/// expiry racing an arm write back an array still holding the expired wait.
fn event_wait_sql(waits: &str, extra_set: &str) -> String {
    format!(
        "UPDATE thread_summaries SET last_activity = NOW(), {extra_set}\
         live_event_waits = {waits}, \
         live_event_wait_count = jsonb_array_length({waits}) \
         WHERE thread_id = $1"
    )
}

static EVENT_WAIT_UPSERT_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    event_wait_sql(
        &format!("({EVENT_WAIT_WITHOUT} || jsonb_build_array($3::jsonb))"),
        "last_agent_action = NOW(), ",
    )
});

static EVENT_WAIT_DROP_SQL: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| event_wait_sql(EVENT_WAIT_WITHOUT, ""));

static EVENT_WAIT_CANCEL_SQL: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| event_wait_sql(EVENT_WAIT_WITHOUT, "last_user_action = NOW(), "));

static STATUS_AFTER_PROPOSAL_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    preserving_verdict(&format!(
        "CASE WHEN status = {RUNNING} OR {parked} THEN status ELSE {IDLE} END",
        parked = crate::engine::question_park_sql::unanswered_question_exists_sql("$1::text"),
    ))
});

impl EventBus {
    pub async fn update_thread_projection(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: Uuid,
        event: &ThreadEvent,
        meta: &EventMeta,
    ) -> Result<(Vec<BusEvent>, Vec<Uuid>), Box<dyn std::error::Error + Send + Sync>> {
        // INVARIANT: Per-token streaming events (TextStreamed, ThoughtStreamed,
        // CodingAgentTextStreamed) fire many times per turn — a 30s CC turn
        // streams ~200 tokens. Their projection arms only set status='running'
        // (idempotent on already-Running threads), so the blocking predicate
        // cannot flip. Short-circuit the before/after sample + ancestor walk
        // to drop ~400 SELECTs per turn from the hot path. The scheduler uses
        // the same predicate to skip its trigger matcher (see
        // `crates/lucidos-engine/src/scheduler/mod.rs`).
        let skip_blocking_sample = event.is_per_token_streaming();

        if self.skips_for_keep_alive(tx, thread_id, event).await? {
            return Ok((Vec::new(), Vec::new()));
        }

        // INVARIANT: Sample is_blocking-relevant state BEFORE the projection mutates it,
        // so the propagation step (after the section transition) can compute
        // an exact delta and walk ancestors via parent_thread_id. None when
        // the row doesn't exist yet — treated as "was not blocking" so the
        // first MessageReceived on a child propagates a clean +1 if blocking.
        // Wrapping at function boundary (rather than per arm) catches every
        // event including the section transition that happens after the
        // metadata match, so no future variant can silently skip propagation.
        let prev_sample = if skip_blocking_sample {
            None
        } else {
            load_blocking_sample(tx, thread_id).await?
        };

        let mut extra_ancestors: Vec<Uuid> = Vec::new();
        // INVARIANT: Arms that fully recompute `blocking_descendant_count` from ground
        // truth (Apply/Discard) flip this so the function-boundary
        // `propagate_blocking_change` doesn't double-subtract on top of
        // the recompute.
        let mut skip_blocking_propagate = false;
        let mut descendant_recount_anchors: Vec<Uuid> = Vec::new();

        let match_side_effects: Vec<BusEvent> = match event {
            ThreadEvent::MessageReceived { text, parent_thread_id, spawning_event_id, wake_mode, mode, .. } => {
                let source = meta.channel.as_ref().map(|c| c.as_str()).unwrap_or("chat");
                let msg_initiator = match mode {
                    ActorMode::Human => LegacyInitiator::User.as_str(),
                    ActorMode::Agent | ActorMode::Engine => LegacyInitiator::System.as_str(),
                };
                let human = matches!(mode, ActorMode::Human);
                let (child_depth, initiator) = if let Some(pid) = parent_thread_id {
                    let parent_row: Option<(i32, String)> = sqlx::query_as(
                        "SELECT COALESCE(depth, 0), initiator FROM thread_summaries WHERE thread_id = $1"
                    )
                    .bind(pid)
                    .fetch_optional(&mut **tx)
                    .await?;
                    match parent_row {
                        Some((d, init)) if init == "system" => (d + 1, "system"),
                        Some((d, _)) => (d + 1, msg_initiator),
                        None => (1, msg_initiator),
                    }
                } else {
                    (0, msg_initiator)
                };
                // INVARIANT: An integrated child is integrated from its first row, so a
                // first run that fails before `SessionStarted` still reads it.
                // Only a spawning message carries a parent link.
                let integrated_spawn = match parent_thread_id {
                    Some(_) => queued_spawn_is_integrated(tx, thread_id).await?,
                    None => false,
                };
                sqlx::query(&format!(
                    r#"INSERT INTO thread_summaries (thread_id, first_message, source, initiator, created_at, last_activity, message_count, parent_thread_id, spawning_event_id, depth, status, last_revived_at, state, parent_callback_pending, wake_mode, coding_agent_integrated, successor_of)
                       VALUES ($1, $2, $3, $6, NOW(), NOW(), 1, $4, $7, $5, {RUNNING}, NOW(), 'active', $4 IS NOT NULL, $9, $10,
                               (SELECT s.thread_id FROM thread_summaries s WHERE s.successor_thread_id = $1 LIMIT 1))
                       ON CONFLICT (thread_id) DO UPDATE
                       SET last_activity = NOW(),
                           parent_callback_pending = CASE WHEN $4 IS NOT NULL THEN TRUE ELSE thread_summaries.parent_callback_pending END,
                           -- Monotone, as in the `SessionStarted` arm.
                           coding_agent_integrated = thread_summaries.coding_agent_integrated OR $10,
                           -- Attributed recency: a human follow-up bumps
                           -- last_user_action (the drawer sort key); an
                           -- agent/engine follow-up bumps last_agent_action.
                           last_user_action = CASE WHEN $8 THEN NOW() ELSE thread_summaries.last_user_action END,
                           last_agent_action = CASE WHEN $8 THEN thread_summaries.last_agent_action ELSE NOW() END,
                           -- The user wrote again, so the reply they were
                           -- asked to read is behind them (ADR 0409).
                           read_requested = CASE WHEN $8 THEN FALSE ELSE thread_summaries.read_requested END,
                           message_count = thread_summaries.message_count + 1,
                           status = CASE WHEN $11 THEN thread_summaries.status ELSE {RUNNING} END,
                           last_revived_at = CASE WHEN $11 THEN thread_summaries.last_revived_at ELSE NOW() END,
                           state = 'active',
                           first_message = COALESCE(thread_summaries.first_message, EXCLUDED.first_message),
                           -- composing → active: the actual send's channel wins
                           -- (the lagged compose-mode source must not survive the
                           -- transition). Active follow-ups already passed the
                           -- continuity check, so 'chat' fall-through covers
                           -- legacy rows missing an explicit assertion.
                           source = CASE
                               WHEN thread_summaries.state = 'composing' THEN EXCLUDED.source
                               WHEN thread_summaries.source = 'chat' THEN EXCLUDED.source
                               ELSE thread_summaries.source
                           END,
                           -- The send decides who started the thread, so an
                           -- agent's draft the owner sends reads as theirs.
                           initiator = CASE
                               WHEN thread_summaries.state = 'composing' THEN EXCLUDED.initiator
                               ELSE thread_summaries.initiator
                           END
                       -- Defense in depth: refuse to resurrect a discarded thread if a
                       -- stale MessageReceived slips past the API-layer guard.
                       WHERE thread_summaries.state != 'discarded'"#
                ))
                .bind(thread_id)
                .bind(text)
                .bind(source)
                .bind(parent_thread_id)
                .bind(child_depth)
                .bind(initiator)
                .bind(spawning_event_id)
                .bind(human)
                .bind(wake_mode.map(|m| m.as_str()))
                .bind(integrated_spawn)
                .bind(meta.queued_for_permission)
                .execute(&mut **tx)
                .await?;

                if let Some(pid) = parent_thread_id {
                    hold_parent_recount_lock(tx, thread_id).await?;
                    sqlx::query(
                        "UPDATE thread_summaries SET active_children_count = active_children_count + 1, \
                         total_children_count = total_children_count + 1 WHERE thread_id = $1"
                    )
                    .bind(pid)
                    .execute(&mut **tx)
                    .await?;
                } else if !meta.queued_for_permission {
                    // INVARIANT: Follow-up MessageReceived: payload's parent_thread_id
                    // is set only on the spawning message, so a follow-up
                    // bypasses the inline +1 above and must re-increment
                    // via the helper if the prior terminal event already
                    // decremented the parent.
                    //
                    // Two operations under two different gates, which is why
                    // they are separate calls: a follow-up always owes the
                    // parent a card for the turn it starts, but it only owes a
                    // re-increment when the child had left the in-flight set.
                    mark_parent_callback_pending(tx, thread_id).await?;
                    if let Some(pid) = reincrement_parent_active_count_if_revived(
                        tx,
                        thread_id,
                        &prev_sample,
                    )
                    .await?
                    {
                        extra_ancestors.push(pid);
                    }
                }

                Vec::new()
            }
            ThreadEvent::SessionStarted { .. } | ThreadEvent::ContinuationStarted { .. } => {
                // INVARIANT: Does this event assert coding-agent identity? `SessionStarted`
                // inherently does — only an agent session emits it.
                // `ContinuationStarted` does NOT: it is the resume boundary for
                // chat and trigger threads too (`chat/rerun.rs`'s
                // `emit_resume_anchor`, reached from `continue_chat` via
                // `POST /api/v1/threads/:id/continue` and from the chat
                // answer-resume path), so it speaks for a coding agent only on
                // the ClaudeCode channel. This arm used to hardcode
                // `is_coding_agent = TRUE`, which permanently relabeled a chat
                // thread on its first Continue click — and since
                // `continue_thread` dispatches on that flag, the NEXT click then
                // took the coding-agent branch and never reached `continue_chat`
                // at all.
                let is_coding_agent_event = matches!(event, ThreadEvent::SessionStarted { .. })
                    || meta.channel == Some(EventChannel::ClaudeCode);
                // INVARIANT: Gated by `$7` below, so the fallback is load-bearing only on
                // the INSERT path (row absent) — where the row's channel still
                // has to come from somewhere. A channel-less
                // `ContinuationStarted` can no longer stamp 'claude_code' onto
                // an existing chat thread.
                let source = meta.channel.as_ref().map(|c| c.as_str()).unwrap_or(
                    if is_coding_agent_event {
                        "claude_code"
                    } else {
                        "chat"
                    },
                );
                let (session_repo_id, session_kind, session_folder, session_agent, session_integrated) = match &event {
                    ThreadEvent::SessionStarted {
                        repo_id,
                        coding_agent_kind,
                        coding_agent_folder,
                        coding_agent,
                        integrated,
                        ..
                    } => (
                        repo_id.as_deref(),
                        Some(coding_agent_kind.as_str()),
                        if coding_agent_folder.is_empty() {
                            None
                        } else {
                            Some(coding_agent_folder.as_str())
                        },
                        Some(coding_agent.as_str()),
                        *integrated,
                    ),
                    _ => (None, None, None, None, false),
                };
                sqlx::query(&format!(
                    r#"INSERT INTO thread_summaries (thread_id, source, is_coding_agent, created_at, last_activity, message_count, status, last_revived_at, cc_repo_id, coding_agent_kind, coding_agent_folder, coding_agent, coding_agents, coding_agent_integrated, state)
                       VALUES ($1, $2, $7, NOW(), NOW(), 0, {RUNNING}, NOW(), $3, $4, $5, $6,
                               CASE WHEN $6::text IS NULL THEN '{{}}'::text[] ELSE ARRAY[$6::text] END, $8, 'active')
                       ON CONFLICT (thread_id) DO UPDATE
                       -- Monotone: a coding-agent event sets the flag and
                       -- nothing here clears it, so no ordering of events can
                       -- downgrade a real coding-agent thread. Repairing rows
                       -- the old hardcoded TRUE already corrupted is the
                       -- migration's job, not the projection's.
                       SET is_coding_agent = thread_summaries.is_coding_agent OR $7,
                           -- Only a coding-agent event may (re)assert the
                           -- thread's channel; a chat/trigger resume boundary
                           -- says nothing about it.
                           source = CASE WHEN $7 THEN $2 ELSE thread_summaries.source END,
                           initiator = COALESCE(thread_summaries.initiator, 'unknown'),
                           -- Existing value wins: a thread's repo is locked at first SessionStarted.
                           -- The chat handler enforces that follow-ups can't pick a different repo,
                           -- but defend the projection so any drift (legacy data, replay) doesn't
                           -- silently flip the thread to a different repo's skill set.
                           cc_repo_id = COALESCE(thread_summaries.cc_repo_id, $3),
                           coding_agent_kind = COALESCE(thread_summaries.coding_agent_kind, $4),
                           coding_agent_folder = COALESCE(thread_summaries.coding_agent_folder, $5),
                           -- The backend is a fact of each session, so the
                           -- newest SessionStarted names it. Only a successor
                           -- session may start on another one.
                           coding_agent = COALESCE($6, thread_summaries.coding_agent),
                           -- Every backend the thread has run on, first use
                           -- first. A replayed session leaves it unchanged.
                           coding_agents = CASE
                               WHEN $6::text IS NULL OR $6::text = ANY(thread_summaries.coding_agents)
                               THEN thread_summaries.coding_agents
                               ELSE array_append(thread_summaries.coding_agents, $6::text)
                           END,
                           -- Monotone like `is_coding_agent`: an integrated
                           -- thread stays integrated for its whole life.
                           coding_agent_integrated = thread_summaries.coding_agent_integrated OR $8,
                           -- The compose fields are not touched: the send that
                           -- started this session consumed the compose slot
                           -- when the engine accepted it (ADR 0450).
                           state = 'active'
                       -- Defense in depth (see MessageReceived above for rationale).
                       WHERE thread_summaries.state != 'discarded'"#
                ))
                .bind(thread_id)
                .bind(source)
                .bind(session_repo_id)
                .bind(session_kind)
                .bind(session_folder)
                .bind(session_agent)
                .bind(is_coding_agent_event)
                .bind(session_integrated)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::TriggerStarted { trigger_id, trigger_name, go_to_review, continue_key, .. } => {
                let source = meta.channel.as_ref().map(|c| c.as_str()).unwrap_or("trigger");
                sqlx::query(&format!(
                    r#"INSERT INTO thread_summaries (thread_id, first_message, source, initiator, created_at, last_activity, message_count, status, last_revived_at, trigger_id, trigger_name, trigger_go_to_review, state, continue_key)
                       VALUES ($1, $2, $3, 'system', NOW(), NOW(), 1, {RUNNING}, NOW(), $4, $5, $6, 'active', $7)
                       ON CONFLICT (thread_id) DO UPDATE
                       SET last_activity = NOW(),
                           -- A trigger firing is autonomous agent activity.
                           last_agent_action = NOW(),
                           message_count = thread_summaries.message_count + 1,
                           status = {RUNNING},
                           last_revived_at = NOW(),
                           state = 'active',
                           trigger_id = COALESCE(thread_summaries.trigger_id, EXCLUDED.trigger_id),
                           trigger_name = COALESCE(thread_summaries.trigger_name, EXCLUDED.trigger_name),
                           continue_key = COALESCE(EXCLUDED.continue_key, thread_summaries.continue_key)
                       -- Defense in depth (see MessageReceived above for rationale).
                       WHERE thread_summaries.state != 'discarded'"#
                ))
                .bind(thread_id)
                .bind(trigger_name.as_deref())
                .bind(source)
                .bind(trigger_id.as_str())
                .bind(trigger_name.as_deref())
                .bind(*go_to_review)
                .bind(continue_key.as_deref())
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            ThreadEvent::AutomationUnpaused { .. } | ThreadEvent::ThreadHoldLifted { .. } => {
                project_hold_summary(tx, thread_id).await?;
                Vec::new()
            }
            ThreadEvent::SessionHandoffWithdrawn
            | ThreadEvent::AutomationPaused { .. }
            | ThreadEvent::ThreadHoldPlaced { .. } => {
                if matches!(
                    event,
                    ThreadEvent::AutomationPaused { .. } | ThreadEvent::ThreadHoldPlaced { .. }
                ) {
                    project_hold_summary(tx, thread_id).await?;
                }
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET status = {STATUS_FROM_PROPOSED_CHANGE} \
                     WHERE thread_id = $1 AND status = {RUNNING} AND {}",
                    super::super::between_turns_sql("$1"),
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                if let Some(pid) = reconcile_parent_active_children_count(tx, thread_id).await? {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::SessionHandedOff { .. } => {
                if crate::engine::pending_handoff::successor_to_start_on(&mut *tx, thread_id)
                    .await?
                    .is_some()
                {
                    sqlx::query("UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1")
                        .bind(thread_id)
                        .execute(&mut **tx)
                        .await?;
                    if let Some(pid) = reincrement_parent_active_count_if_revived(tx, thread_id, &prev_sample).await? {
                        extra_ancestors.push(pid);
                    }
                }
                Vec::new()
            }
            ThreadEvent::ResponseGenerated { .. } => {
                let status = status_after_session_bookkeeping(tx, thread_id).await?;
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_agent_action = NOW(), \
                     has_response = TRUE, \
                     status = {status} WHERE thread_id = $1"
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                // INVARIANT: In-tx parent recount, the sole writer of the parent's
                // `active_children_count` on a terminal. The captured aggregate
                // and the affected-ancestor broadcast then carry the
                // post-terminal count. Without it the parent stays "waiting on
                // children" until some later event refreshes its aggregate. See
                // `test_cc_idle_in_tx_broadcast_carries_decremented_parent_active`.
                if let Some(pid) =
                    reconcile_parent_active_children_count(tx, thread_id).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::ResponseAborted { cause, .. } => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_agent_action = NOW(), \
                     has_response = TRUE, \
                     status = {} WHERE thread_id = $1",
                    cause.status_sql(meta.actor.as_ref())
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                // INVARIANT: Mirror the out-of-tx `notify_parent_if_child` gate. Only the
                // user's own switch promises a resume, so only its abort is
                // mid-retry. Reconciling it would drop the parent's count
                // under a child that comes straight back. A crash resumes
                // nothing, so its child is no longer in flight. See
                // ResponseGenerated for the IN-TX vs out-of-tx broadcast
                // rationale.
                if !cause.promises_auto_resume(meta.actor.as_ref()) {
                    if let Some(pid) =
                        reconcile_parent_active_children_count(tx, thread_id).await?
                    {
                        extra_ancestors.push(pid);
                    }
                }
                Vec::new()
            }
            ThreadEvent::CodingAgentIdled {
                is_external_repo,
                has_changes,
                ..
            } => {
                // INVARIANT: Authoritative for `is_external_repo` and for whether the branch
                // holds work (git truth, `wt_has_changes` from
                // `branch_changed_files`, as the seed and refresh paths read
                // it). A pending change keeps the state `proposed`; the
                // proposal itself stays the aggregate `ChangeProposed`'s job.
                // Work that stays unproposed keeps the reason a
                // `ProposalWithheld` gave it, which may land before this idle.
                // `has_response = TRUE` is the CC equivalent of
                // `ResponseGenerated` for surfacing the thread in
                // `get_recent_threads`.
                //
                // `preserving_verdict` keeps the turn's verdict. A CC turn that
                // ends in failure emits `ResponseFailed` (→ status='failed')
                // immediately followed by this `CodingAgentIdled` in the SAME
                // turn (`classify_result` returns `emit_idle=true` for every
                // non-shutdown Result, including the Failed kind), and a turn
                // an engine restart interrupted emits `ResponseAborted`
                // (→ status='paused') then this idle. The restart case is the
                // sharper one: recovery emits the idle ITSELF, one boot later,
                // so nothing in the turn's own lifetime protects the verdict.
                // Without the guard this bookkeeping idle would downgrade
                // 'failed' or 'paused' to 'idle' and erase the status dot in
                // the thread list. The terminal event owns the turn's status;
                // this idle only parks the thread when the turn didn't already
                // fail or pause. A later follow-up
                // (`CodingAgentUserMessageSent` → 'running') clears the verdict
                // before the next idle, so neither can wedge the thread.
                // Mirrors the `CASE WHEN status='running'` preservation the
                // `ChangeProposed` arm uses for the same class of reason.
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_agent_action = NOW(), \
                     has_response = TRUE, \
                     coding_agent_is_external_repo = $2, \
                     {}, \
                     status = {} \
                     WHERE thread_id = $1",
                    branch_work_sql("$3"),
                    status_after_session_bookkeeping(tx, thread_id).await?
                ))
                .bind(thread_id)
                .bind(is_external_repo)
                .bind(has_changes)
                .execute(&mut **tx)
                .await?;
                if let Some(pid) =
                    reconcile_parent_active_children_count(tx, thread_id).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::ChangeApplied {
                change_id,
                requires_restart,
                commits,
                pre_merge_sha,
                post_merge_sha,
                ..
            } => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_user_action = NOW(), \
                     {clear}, \
                     status = {IDLE} \
                     WHERE thread_id = $1",
                    clear = *CLEAR_CODING_AGENT_FLAGS,
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                crate::core::changes_projection::ChangesProjection::write_applied(
                    tx,
                    change_id,
                    *requires_restart,
                    commits,
                    pre_merge_sha.as_deref(),
                    post_merge_sha.as_deref(),
                )
                .await?;
                crate::engine::thread_events::permission::projection::end_until_plan_lands(tx, thread_id).await?;
                reconcile_proposal_lifecycle_end(
                    tx,
                    thread_id,
                    &mut extra_ancestors,
                    &mut descendant_recount_anchors,
                )
                .await?;
                skip_blocking_propagate = true;
                Vec::new()
            }
            ThreadEvent::ChangeDiscarded { change_id, .. } => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_user_action = NOW(), \
                     {clear}, \
                     status = {IDLE} \
                     WHERE thread_id = $1",
                    clear = *CLEAR_CODING_AGENT_FLAGS,
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                crate::core::changes_projection::ChangesProjection::write_status(
                    tx, change_id, crate::core::changes::ChangeStatus::Discarded,
                )
                .await?;
                reconcile_proposal_lifecycle_end(
                    tx,
                    thread_id,
                    &mut extra_ancestors,
                    &mut descendant_recount_anchors,
                )
                .await?;
                skip_blocking_propagate = true;
                Vec::new()
            }

            ThreadEvent::CodingAgentUserMessageSent { .. }
            | ThreadEvent::PromptInjected { .. } => {
                let (status, human) = match event {
                    ThreadEvent::PromptInjected { mode, .. } => (
                        format!("CASE WHEN status = {WAITING_FOR_USER_ANSWER} THEN status ELSE {RUNNING} END"),
                        *mode == ActorMode::Human,
                    ),
                    _ => (RUNNING.to_string(), true),
                };
                let (action_column, read_requested) = if human {
                    ("last_user_action", "FALSE")
                } else {
                    ("last_agent_action", "read_requested")
                };
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), {action_column} = NOW(), message_count = message_count + 1, status = {status}, last_revived_at = NOW(), coding_agent_unproposed_reason = NULL, read_requested = {read_requested} WHERE thread_id = $1",
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                // INVARIANT: Start event: the parent owes a card for the turn this begins.
                mark_parent_callback_pending(tx, thread_id).await?;
                if let Some(pid) =
                    reincrement_parent_active_count_if_revived(tx, thread_id, &prev_sample).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }

            ThreadEvent::ThreadTitleGenerated { title } | ThreadEvent::ThreadTitleRenamed { title } => {
                if !write_thread_title(tx, thread_id, title).await? {
                    crate::log_legacy!(
                        "[EventBus] Dropped the name {:?} for {}: no thread_summaries row. \
                         A title event has to follow the thread's first MessageReceived \
                         (ADR 0240), and a deleted thread has no row to name.",
                        title,
                        thread_id
                    );
                }
                Vec::new()
            }

            ThreadEvent::ThreadArchiveRequested => Vec::new(),

            ThreadEvent::ThreadReadRequested { parent_report } => {
                sqlx::query(
                    "UPDATE thread_summaries SET read_requested = TRUE, reply_seen_devices = '{}' \
                     WHERE thread_id = $1 AND NOT ($2 AND coding_agent_integrated)",
                )
                .bind(thread_id)
                .bind(parent_report)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadReplySeen { personal: false, .. }
            | ThreadEvent::ThreadReadNotRequested {
                supersedes_request: true,
            } => {
                sqlx::query("UPDATE thread_summaries SET read_requested = FALSE WHERE thread_id = $1")
                    .bind(thread_id)
                    .execute(&mut **tx)
                    .await?;
                Vec::new()
            }
            ThreadEvent::ThreadReplySeen { personal: true, .. } => {
                if let Some(MessageOrigin::Device { device_id }) = &meta.actor {
                    sqlx::query(
                        "UPDATE thread_summaries \
                         SET reply_seen_devices = array_append(reply_seen_devices, $2) \
                         WHERE thread_id = $1 AND NOT ($2 = ANY(reply_seen_devices))",
                    )
                    .bind(thread_id)
                    .bind(device_id)
                    .execute(&mut **tx)
                    .await?;
                }
                Vec::new()
            }

            ThreadEvent::ParentWakeRequested | ThreadEvent::ParentWakeNotRequested => {
                let requested = matches!(event, ThreadEvent::ParentWakeRequested);
                sqlx::query(
                    "UPDATE thread_summaries SET parent_wake_requested = $2 WHERE thread_id = $1",
                )
                .bind(thread_id)
                .bind(requested)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ApplyNowStarted | ThreadEvent::ApplyNowEnded { .. } => {
                let in_flight = matches!(event, ThreadEvent::ApplyNowStarted);
                sqlx::query("UPDATE thread_summaries SET apply_in_flight = $2 WHERE thread_id = $1")
                    .bind(thread_id)
                    .bind(in_flight)
                    .execute(&mut **tx)
                    .await?;
                Vec::new()
            }
            ThreadEvent::ThreadMcpServerEnabled { server_id, .. } => {
                sqlx::query(
                    "UPDATE thread_summaries \
                     SET enabled_mcp_servers = array_append(enabled_mcp_servers, $2) \
                     WHERE thread_id = $1 AND NOT ($2 = ANY(enabled_mcp_servers))",
                )
                .bind(thread_id)
                .bind(server_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadMcpServerDisabled { server_id, .. } => {
                sqlx::query(
                    "UPDATE thread_summaries \
                     SET enabled_mcp_servers = array_remove(enabled_mcp_servers, $2) \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .bind(server_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            ThreadEvent::ThreadUnarchived
            | ThreadEvent::ThreadTriageProposed { .. }
            | ThreadEvent::ThreadReadNotRequested {
                supersedes_request: false,
            } => Vec::new(),

            ThreadEvent::ThreadSaved => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET is_saved = TRUE, archive_state = '{}' \
                     WHERE thread_id = $1",
                    ArchiveState::Inbox.as_str()
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadUnsaved => {
                sqlx::query(
                    "UPDATE thread_summaries SET is_saved = FALSE, pin_folder = NULL \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadMovedToFolder { folder } => {
                sqlx::query(
                    "UPDATE thread_summaries SET pin_folder = $2 \
                     WHERE thread_id = $1 AND is_saved",
                )
                .bind(thread_id)
                .bind(folder.as_deref())
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            ThreadEvent::ThreadArchived => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET status = {IDLE}, \
                     is_saved = FALSE, pin_folder = NULL, \
                     is_stopped_child = FALSE, read_requested = FALSE, \
                     continue_key = NULL, \
                     {clear} \
                     WHERE thread_id = $1",
                    clear = *CLEAR_CODING_AGENT_FLAGS,
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                // INVARIANT: The status write above can take this thread out of the
                // in-flight set, so its parent recounts, as on any terminal. A
                // paused child kept its place on the count, expecting a resume
                // the archive ends. Without this the parent stayed in Current.
                if let Some(pid) = reconcile_parent_active_children_count(tx, thread_id).await? {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::ThreadStarted { mode, recovered_compose, agent_draft, .. } => {
                crate::core::preferences::lock_compose_agent_seed(tx, false).await?;
                let seed = recovered_compose
                    .as_ref()
                    .map(|r| (&r.text, &r.image_hashes, &r.selection))
                    .or(agent_draft.as_ref().map(|d| (&d.text, &d.image_hashes, &d.selection)));
                let text = seed.map_or("", |(text, _, _)| text.as_str());
                let images = serde_json::json!(seed.map(|(_, images, _)| images.clone()).unwrap_or_default());
                let selection = seed.and_then(|(_, _, selection)| selection.as_ref());
                let source = if mode == "claude_code" { "claude_code" } else { "chat" };
                let initiator = if agent_draft.is_some() { "system" } else { "user" };
                sqlx::query(&format!(
                    r#"INSERT INTO thread_summaries
                        (thread_id, initiator, source, created_at, last_activity, message_count,
                         state, compose_mode, status, compose_text, compose_images, compose_selection, compose_updated_at,
                         drafted_by_thread_id)
                       VALUES ($1, $8, $3, NOW(), NOW(), 0, 'composing', $2, {IDLE}, $4, $5, $6, CASE WHEN $7 THEN NOW() ELSE NULL END, $9)
                       ON CONFLICT (thread_id) DO NOTHING"#
                ))
                .bind(thread_id)
                .bind(mode)
                .bind(source)
                .bind(text)
                .bind(images)
                .bind(selection)
                .bind(seed.is_some())
                .bind(initiator)
                .bind(agent_draft.as_ref().map(|d| d.drafted_by_thread_id))
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::HomeThreadCreated => {
                // INVARIANT: No `ON CONFLICT`, on purpose. A second home thread hits the
                // partial unique index. The error rolls this event back with
                // the row, so the log never records two homes.
                sqlx::query(&format!(
                    r#"INSERT INTO thread_summaries
                        (thread_id, title, initiator, source, created_at, last_activity,
                         message_count, state, status, is_home)
                       VALUES ($1, $2, 'user', 'chat', NOW(), NOW(), 0, 'active', {IDLE}, TRUE)"#
                ))
                .bind(thread_id)
                .bind(crate::engine::thread_events::HOME_THREAD_TITLE)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadDiscarded { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET state = 'discarded', \
                     archive_state = 'archived', is_saved = FALSE, pin_folder = NULL, \
                     compose_text = '', compose_images = '[]'::jsonb, compose_mode = NULL, \
                     compose_selection = NULL \
                     WHERE thread_id = $1 AND state = 'composing'",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            ThreadEvent::ResponseCanceled { cause, .. } => {
                // INVARIANT: User canceled — go idle. Set has_response so the thread
                // appears in archive (a canceled response is still a response —
                // the user should see the thread).
                //
                // `preserving_verdict` covers the shutdown sweep's ordering:
                // `shutdown_active_threads` emits `ResponseAborted` (whose
                // verdict is 'paused' for a user-initiated switch and 'failed'
                // otherwise, per `AbortCause::promises_auto_resume`), then
                // cancels the loop, whose own `ResponseCanceled` follows. Both
                // are anchored on the in-flight turn, so
                // `emit_response_canceled`'s idempotency gate normally pairs
                // them, but a turn that never recorded an anchor leaves nothing
                // to match and the phantom cancel lands on top.
                // `ResponseAborted` "taking precedence" holds for
                // the exchange label (`exchangeStatus` checks aborted before
                // canceled, order-independently) but the status column is
                // last-write-wins, so without this guard the sweep walked an
                // interrupted chat thread's status dot back to 'idle'. A cancel
                // of a genuinely new turn is preceded by a start event that
                // already cleared the verdict, so a real Stop still settles the
                // thread.
                //
                // A redirect cancel (`CancelCause::is_redirect`) is the engine
                // interrupting the turn to carry the work on, so the thread
                // keeps its status. The redirected turn's start follows once
                // teardown ends, and `CodingAgentIdled` and `SessionEnded`
                // hold the status until then (`keeping_status_across_redirect`).
                let status = if cause.is_redirect() {
                    "status".to_string()
                } else {
                    preserving_verdict(STATUS_FROM_PROPOSED_CHANGE)
                };
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET has_response = TRUE, \
                     status = {status} WHERE thread_id = $1",
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                let by_agent = meta
                    .actor
                    .as_ref()
                    .is_some_and(|actor| actor.mode() != ActorMode::Human);
                if matches!(cause, CancelCause::UserStop) && !by_agent {
                    mark_stopped_child(tx, thread_id).await?;
                }
                if let Some(pid) =
                    reconcile_parent_active_children_count(tx, thread_id).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::QueuedMessageRemoved { removed_message_id, compose, .. } => {
                if let Some(report) = compose {
                    sqlx::query("UPDATE thread_summaries SET compose_text=$2, compose_images=$3, compose_mode=$4, compose_selection=$5, compose_epoch=$6, compose_restorations=$7 WHERE thread_id=$1 AND compose_epoch < $6")
                        .bind(thread_id).bind(&report.text).bind(serde_json::json!(report.image_hashes))
                        .bind(&report.mode).bind(&report.selection).bind(report.compose_epoch).bind(serde_json::json!(report.restorations))
                        .execute(&mut **tx).await?;
                }
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET status = {IDLE} \
                     WHERE thread_id = $1 AND status = {RUNNING} AND source <> $3 \
                       AND NOT EXISTS (SELECT 1 FROM events e WHERE e.thread_id = $1 \
                         AND e.event_type IN ('MessageReceived', 'ChatTurnSettingsResolved') \
                         AND e.sequence > COALESCE((SELECT t.sequence FROM events t \
                           WHERE t.thread_id = $1 AND t.event_type IN ('ResponseGenerated', \
                             'ResponseCanceled', 'ResponseAborted', 'ResponseFailed') \
                           ORDER BY t.sequence DESC LIMIT 1), 0) \
                         AND NOT (e.event_type = 'MessageReceived' AND (e.id = $2 OR EXISTS ( \
                           SELECT 1 FROM events r WHERE r.thread_id = $1 \
                             AND r.event_type = 'QueuedMessageRemoved' \
                             AND r.payload->>'removed_message_id' = e.id::text))))",
                ))
                .bind(thread_id)
                .bind(removed_message_id)
                .bind(crate::engine::thread_lifecycle::CODING_AGENT_SOURCE)
                .execute(&mut **tx)
                .await?;
                if let Some(pid) =
                    reconcile_parent_active_children_count(tx, thread_id).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::ResponseFailed { .. } => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_agent_action = NOW(), has_response = TRUE, status = {} WHERE thread_id = $1",
                    ThreadStatus::Failed.sql_literal(),
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                if let Some(pid) =
                    reconcile_parent_active_children_count(tx, thread_id).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::SessionEnded { reason } => {
                if !reason.is_transient() {
                    sqlx::query(&format!(
                        "UPDATE thread_summaries SET has_response = TRUE, \
                         status = {} WHERE thread_id = $1",
                        status_after_session_bookkeeping(tx, thread_id).await?
                    ))
                    .bind(thread_id)
                    .execute(&mut **tx)
                    .await?;
                    // INVARIANT: See ResponseGenerated for the IN-TX broadcast rationale.
                    // Gated on `!reason.is_transient()` to mirror the
                    // out-of-tx `notify_parent_if_child` gate — transient
                    // SessionEnded is mid-retry and must not drop the count.
                    if let Some(pid) =
                        reconcile_parent_active_children_count(tx, thread_id).await?
                    {
                        extra_ancestors.push(pid);
                    }
                }
                Vec::new()
            }
            ThreadEvent::TriggerCompleted { .. } => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_agent_action = NOW(), has_response = TRUE, status = {IDLE} WHERE thread_id = $1",
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeProposed {
                change_id,
                description,
                files,
                requires_restart,
                commit_sha,
                branch_name,
                repo_root,
                hardened,
                incomplete,
                set_aside,
                ..
            } => {
                use crate::core::changes_projection::ChangesProjection;
                debug_assert!(
                    !change_id.is_empty() || commit_sha.is_some(),
                    "ChangeProposed with neither change_id nor commit_sha is malformed"
                );
                if crate::core::git::is_lane_branch(branch_name) {
                    return Err(format!(
                        "ChangeProposed refused for thread {thread_id} on {branch_name}: {}",
                        crate::core::git::LANE_BRANCH_IS_NOT_A_CHANGE
                    )
                    .into());
                }
                if !change_id.is_empty() {
                    // INVARIANT: CASE preserves 'running' so a follow-up turn overlapping
                    // the emit doesn't yank the thread back to 'idle' while
                    // CC is still mid-stream. Else 'idle' — a proposed change
                    // is an artifact for review, not a parked loop. The
                    // pending-review state is carried by the change state.
                    //
                    // `preserving_verdict` because a proposal is also one of the
                    // dying turn's trailing events: an interrupted coding-agent
                    // session commits and proposes on its way out, seconds after
                    // the teardown abort. Without the wrap this arm walked that
                    // abort's verdict to 'idle'. That is the same erasure
                    // `CodingAgentIdled` and `SessionEnded` already guard
                    // against, and the second half of
                    // `docs/plans/2026-08-22-a-restart-verdict-survives-a-pending-change.md`.
                    //
                    // The branch work comes from the file list the event carries.
                    // `reconcile_emptied_pending_change` re-emits with an empty
                    // one when the diff cancelled out, and a set-aside proposal
                    // leaves its work unproposed. `write_proposed_aggregate`
                    // then moves a pending row to `proposed`.
                    sqlx::query(&format!(
                        "UPDATE thread_summaries SET {}, \
                         status = {} \
                         WHERE thread_id = $1",
                        branch_work_sql("$2"),
                        &*STATUS_AFTER_PROPOSAL_SQL
                    ))
                    .bind(thread_id)
                    .bind(!files.is_empty())
                    .execute(&mut **tx)
                    .await?;
                    if *set_aside {
                        sqlx::query(
                            "UPDATE thread_summaries SET coding_agent_unproposed_reason = NULL \
                             WHERE thread_id = $1",
                        )
                        .bind(thread_id)
                        .execute(&mut **tx)
                        .await?;
                    }
                    ChangesProjection::write_proposed_aggregate(
                        tx,
                        change_id,
                        thread_id,
                        branch_name,
                        repo_root,
                        description.as_deref(),
                        files,
                        *requires_restart,
                        *hardened,
                        *incomplete,
                        if *set_aside {
                            crate::core::changes::ChangeStatus::SetAside
                        } else {
                            crate::core::changes::ChangeStatus::Pending
                        },
                    )
                    .await?;
                } else if commit_sha.is_some() {
                    ChangesProjection::write_proposed_per_commit(
                        tx,
                        branch_name,
                        description.as_deref(),
                        *requires_restart,
                    )
                    .await?;
                }
                Vec::new()
            }
            ThreadEvent::CodingAgentPromptSent { text, .. } => {
                if !text.is_empty() {
                    sqlx::query(&format!(
                        "UPDATE thread_summaries SET status = {RUNNING}, last_revived_at = NOW() WHERE thread_id = $1",
                    ))
                    .bind(thread_id)
                    .execute(&mut **tx)
                    .await?;
                    mark_parent_callback_pending(tx, thread_id).await?;
                    if let Some(pid) = reincrement_parent_active_count_if_revived(
                        tx,
                        thread_id,
                        &prev_sample,
                    )
                    .await?
                    {
                        extra_ancestors.push(pid);
                    }
                }
                Vec::new()
            }
            ThreadEvent::ChatTurnSettingsResolved { .. } => {
                if let Some(message) = meta.request_event_id {
                    let resumed = sqlx::query(&format!(
                        "UPDATE thread_summaries SET status={RUNNING}, last_revived_at=NOW()
                         WHERE thread_id=$1 AND EXISTS (SELECT 1 FROM events WHERE id=$2
                           AND thread_id=$1 AND payload->>'queued_for_permission'='true')",
                    ))
                    .bind(thread_id)
                    .bind(message)
                    .execute(&mut **tx)
                    .await?
                    .rows_affected() > 0;
                    if resumed {
                        mark_parent_callback_pending(tx, thread_id).await?;
                        if let Some(pid) = reincrement_parent_active_count_if_revived(
                            tx, thread_id, &prev_sample,
                        ).await? {
                            extra_ancestors.push(pid);
                        }
                    }
                }
                Vec::new()
            }
            ThreadEvent::CodingAgentInputRead {
                started_turn: true, ..
            } => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET status = {RUNNING}, last_revived_at = NOW() WHERE thread_id = $1",
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                mark_parent_callback_pending(tx, thread_id).await?;
                if let Some(pid) =
                    reincrement_parent_active_count_if_revived(tx, thread_id, &prev_sample).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::ContinuationRequested { .. } => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     status = {RUNNING}, last_revived_at = NOW(), \
                     coding_agent_unproposed_reason = NULL WHERE thread_id = $1",
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;

                // INVARIANT: Continue is a start event, so the parent owes a card for the
                // turn it begins. Without this the resumed turn's
                // `CodingAgentIdled` hits the dedup guard in
                // `notify_parent_if_child` and the parent is never told: this
                // arm re-increments below but never touched the marker.
                mark_parent_callback_pending(tx, thread_id).await?;

                // INVARIANT: Re-increment parent's `active_children_count`: the synthetic
                // `CodingAgentIdled{reason=engine_restart_interrupt}` that
                // recovery emits for the prior turn already decremented the
                // parent (a parked child is not active). When the user clicks
                // Continue the child is alive again, so the parent's count
                // must bounce back — otherwise its `ThreadStatusIcon` stays
                // 'idle' (no pulsing dot) while the child is actively running.
                //
                // Through the SHARED helper, like every other revive arm, and
                // that is the whole point. This arm used to hand-roll the same
                // parent lookup and `+1` behind an `is_blocking()` gate, which
                // the helper's own doc names as the wrong predicate: an idle CC
                // child holding a pending change is `is_blocking = true` (clause
                // 3) while its status is `idle`, i.e. NOT counted in the parent.
                // The hand-rolled gate therefore skipped the re-increment on
                // exactly the shape it was written for, and the parent's dot
                // stayed idle for the whole resumed turn.
                // `active_thread_statuses()` asks the question that actually
                // matters (was the child already counted?) and still suppresses
                // the double-increment a rapid double click would cause, since a
                // re-clicked child is already `running`.
                if let Some(pid) =
                    reincrement_parent_active_count_if_revived(tx, thread_id, &prev_sample).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::ToolCalled { .. }
            | ThreadEvent::ToolResult { .. }
            | ThreadEvent::TextStreamed { .. }
            | ThreadEvent::ThoughtStreamed { .. }
            | ThreadEvent::MemoryRecalled { .. }
            | ThreadEvent::CodingAgentTextStreamed { .. }
            | ThreadEvent::CodingAgentThoughtStreamed { .. }
            | ThreadEvent::CodingAgentToolCalled { .. }
            | ThreadEvent::CodingAgentToolResult { .. } => {
                // INVARIANT: Update last_activity so the thread list timestamp stays current during
                // long-running agentic responses. Without this, the timestamp only
                // advances on discrete lifecycle events, not during streaming.
                //
                // Also bump status back to 'running' if the projection drifted to a
                // non-running state (e.g. CC emitted a mid-session `Result` that the
                // engine treated as idle, then continued working). `last_revived_at`
                // is gated by CASE rather than set unconditionally — sibling UPDATEs
                // for one-shot transitions (`ContinuationRequested`, etc.) refresh it every
                // time, but activity events fire many times per turn and would
                // constantly reshuffle IN PROGRESS sort order if treated the same
                // way. Mirrors `status_transitions()` for these event types — see
                // `thread_lifecycle.rs`. One exception the table cannot express: a
                // `CodingAgentToolResult` keeps a park it is not the question's
                // own result for (`tool_result_keeps_park_sql`).
                //
                // The bump is `preserving_verdict`, so it can revive a thread
                // that drifted to 'idle'/'waiting' but never one whose turn
                // already carries a verdict ('failed', or the 'paused' an
                // engine restart writes). That case is
                // routine for a coding agent and impossible for the Lucidos
                // Agent, which is why only the latter used to keep its error
                // dot after an interruption: the restart teardown emits
                // `ResponseAborted` while the subprocess is still alive, and
                // `external_terminal_emitted` suppresses only the duplicate
                // TERMINAL — the drain keeps forwarding
                // `CodingAgentTextStreamed` / `CodingAgentToolResult` for
                // milliseconds afterwards. Those landed here, wrote 'running'
                // over the verdict, and the `CodingAgentIdled` closing the turn
                // then read a non-'failed' row and settled it to 'idle'. The
                // chat loop emits nothing after its own terminator, so its
                // 'failed' survived. Reviving on trailing output is wrong on
                // its own terms too: the turn is over, nothing is running.
                //
                // Exception: `MessageOrigin::System` activity events are
                // backfill from the recovery sweeps (see
                // `recover_orphan_tool_calls` in `engine/chat/recovery.rs` —
                // emits a synthetic `ToolResult` to pair an orphan
                // `ToolCalled` so the next LLM call doesn't trip the
                // Anthropic API's "tool_use without tool_result" rule).
                // They arrive long after the turn's terminal event, on
                // threads the user has long since stopped touching, so
                // the bump-to-Running is wrong: it parks the thread in the
                // Active section forever with no way out except a manual
                // SQL UPDATE. Live activity events never carry an actor —
                // the LLM-loop emit sites pass `EventMeta::NONE` — so this
                // guard only catches the recovery path.
                let is_recovery_backfill =
                    matches!(meta.actor, Some(MessageOrigin::System));
                if is_recovery_backfill {
                    sqlx::query(
                        "UPDATE thread_summaries SET last_activity = NOW() WHERE thread_id = $1",
                    )
                    .bind(thread_id)
                    .execute(&mut **tx)
                    .await?;
                } else {
                    let (keeps_park, result_id) = match event {
                        ThreadEvent::CodingAgentToolResult { tool_use_id, .. } => {
                            (tool_result_keeps_park_sql(), Some(tool_use_id))
                        }
                        _ => ("FALSE".to_string(), None),
                    };
                    let sql = format!(
                        "UPDATE thread_summaries SET last_activity = NOW(), \
                         last_agent_action = NOW(), \
                         last_revived_at = CASE WHEN status NOT IN ({RUNNING}, {verdicts}) \
                                                 AND NOT {keeps_park} \
                                                THEN NOW() ELSE last_revived_at END, \
                         status = CASE WHEN {keeps_park} THEN status ELSE {status} END \
                         WHERE thread_id = $1",
                        verdicts = &*crate::engine::event_bus::PRESERVED_STATUS_VERDICTS_SQL,
                        status = preserving_verdict(RUNNING),
                    );
                    let mut query = sqlx::query(&sql).bind(thread_id);
                    if let Some(result_id) = result_id {
                        query = query.bind(result_id);
                    }
                    query.execute(&mut **tx).await?;
                }
                Vec::new()
            }
            ThreadEvent::UserQuestionAsked { .. }
            | ThreadEvent::CodingAgentPermissionRequest { .. }
            | ThreadEvent::CommandPermissionRequested { .. }
            | ThreadEvent::McpPermissionRequested { .. } => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_agent_action = NOW(), \
                     status = {WAITING_FOR_USER_ANSWER} WHERE thread_id = $1",
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::EventWaitStarted {
                wait_id,
                on,
                reason,
                expires_at,
                background_timing,
                ..
            } => {
                let entry = EventWaitSummary {
                    wait_id: *wait_id,
                    on: on.clone(),
                    reason: reason.clone(),
                    expires_at: *expires_at,
                    background_timing: *background_timing,
                };
                sqlx::query(EVENT_WAIT_UPSERT_SQL.as_str())
                    .bind(thread_id)
                    .bind(wait_id.to_string())
                    .bind(sqlx::types::Json(&entry))
                    .execute(&mut **tx)
                    .await?;
                Vec::new()
            }
            ThreadEvent::EventWaitTimingUpdated { wait_id, reason, expires_at, background_timing } => {
                sqlx::query(
                    "UPDATE thread_summaries SET live_event_waits = ( \
                        SELECT COALESCE(jsonb_agg(CASE WHEN w->>'wait_id' = $2 \
                            THEN w || $3::jsonb ELSE w END ORDER BY ordinal), '[]'::jsonb) \
                        FROM jsonb_array_elements(live_event_waits) WITH ORDINALITY AS waits(w, ordinal) \
                    ) WHERE thread_id = $1 AND live_event_waits @> jsonb_build_array(jsonb_build_object('wait_id', $2::text))",
                )
                .bind(thread_id)
                .bind(wait_id.to_string())
                .bind(serde_json::json!({
                    "reason": reason,
                    "expires_at": expires_at,
                    "background_timing": background_timing,
                }))
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            // INVARIANT: **No resolution moves the status.** A subscription never held the
            // turn, so its resolution says nothing about one. An idle thread
            // is woken by its own `PromptInjected`, a running one would be
            // misreported as revived, and a question keeps a parked one.
            //
            // A delivery or an expiry leaves `last_agent_action` to the
            // re-entry anchor written right after it, a non-human
            // `PromptInjected`. A cancel IS a user action, because every cancel
            // cause is a person pressing something.
            //
            // All three DROP the wait from `live_event_waits`, by `wait_id`,
            // and the count follows from the array's new length. The wait is
            // gone either way, so both the dot and the panel have to let go of
            // it. A resolution with no matching entry (a test fixture, a
            // hand-emitted row) leaves both columns exactly as they were,
            // which is what `event_wait_sql` explains.
            ThreadEvent::EventWaitDelivered { wait_id, .. }
            | ThreadEvent::EventWaitExpired { wait_id } => {
                sqlx::query(EVENT_WAIT_DROP_SQL.as_str())
                    .bind(thread_id)
                    .bind(wait_id.to_string())
                    .execute(&mut **tx)
                    .await?;
                Vec::new()
            }
            ThreadEvent::CacheKeepAliveStatusChanged { status } => {
                sqlx::query(
                    "UPDATE thread_summaries SET cache_keep_alive_status = $2 WHERE thread_id = $1",
                )
                .bind(thread_id)
                .bind(status.as_ref().map(sqlx::types::Json))
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::CacheKeepAliveTurnedOff => {
                sqlx::query(
                    "UPDATE thread_summaries SET cache_keep_alive_off_at = \
                            (SELECT created FROM events \
                              WHERE thread_id = $1 AND event_type = 'CacheKeepAliveTurnedOff' \
                              ORDER BY sequence DESC LIMIT 1) \
                      WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::CacheKeepAliveTurnedOn => {
                sqlx::query(
                    "UPDATE thread_summaries SET cache_keep_alive_off_at = NULL \
                      WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::WakeExpected { .. } => Vec::new(),
            ThreadEvent::EventWaitCanceled { wait_id, .. } => {
                sqlx::query(EVENT_WAIT_CANCEL_SQL.as_str())
                    .bind(thread_id)
                    .bind(wait_id.to_string())
                    .execute(&mut **tx)
                    .await?;
                Vec::new()
            }
            ThreadEvent::UserQuestionAnswered { tool_use_id, quiet, compose, .. } => {
                let retires_queued_card =
                    crate::engine::event_bus::card_slot::is_queued_unposted(tx, thread_id, tool_use_id)
                        .await?;
                if let Some(report) = compose {
                    sqlx::query("UPDATE thread_summaries SET compose_text=$2, compose_images=$3, compose_mode=$4, compose_selection=$5, compose_epoch=$6, compose_restorations=$7 WHERE thread_id=$1 AND compose_epoch < $6")
                        .bind(thread_id).bind(&report.text).bind(serde_json::json!(report.image_hashes))
                        .bind(&report.mode).bind(&report.selection).bind(report.compose_epoch).bind(serde_json::json!(report.restorations))
                        .execute(&mut **tx).await?;
                }
                let status = if *quiet {
                    format!(
                        "CASE WHEN EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
                         AND event_type = 'UserQuestionAsked' AND payload->>'tool_use_id' = $3 \
                         AND (payload #> '{{permission_request,batch,items}}' \
                              @> '[{{\"kind\":\"trigger_program_grant\"}}]'::jsonb \
                         OR payload #> '{{permission_request,batch,items}}' @> '[{{\"kind\":\"trigger_folder_grant\"}}]'::jsonb)) \
                         THEN CASE WHEN status = {WAITING_FOR_USER_ANSWER} THEN {} ELSE status END \
                         WHEN EXISTS (SELECT 1 FROM events q WHERE q.thread_id = $1 \
                           AND q.event_type = 'UserQuestionAsked' \
                           AND (q.payload #> '{{permission_request,batch,items}}' \
                               @> '[{{\"kind\":\"reviewed_trigger_program_grant\"}}]'::jsonb \
                           OR q.payload #> '{{permission_request,batch,items}}' \
                               @> '[{{\"kind\":\"reviewed_trigger_program_grants\"}}]'::jsonb) \
                           AND NOT EXISTS (SELECT 1 FROM events a WHERE a.thread_id = q.thread_id \
                               AND a.event_type = 'UserQuestionAnswered' \
                               AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id')) \
                         THEN {WAITING_FOR_USER_ANSWER} \
                         ELSE {IDLE} END",
                        ThreadStatus::Failed.sql_literal(),
                    )
                } else {
                    format!("{RUNNING}, last_revived_at = NOW()")
                };
                let human = MessageOrigin::mode_or(meta.actor.as_ref(), ActorMode::Engine)
                    == ActorMode::Human;
                let sql = format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_user_action = NOW(), \
                     status = {status}, \
                     read_requested = read_requested AND NOT $2 WHERE thread_id = $1",
                );
                let mut query = sqlx::query(&sql).bind(thread_id).bind(human);
                if *quiet {
                    query = query.bind(tool_use_id);
                }
                if !retires_queued_card {
                    query.execute(&mut **tx).await?;
                }
                Vec::new()
            }
            // INVARIANT: A permission resolution only ever unblocks an in-memory broadcast
            // waiter (cc_permission / command_permission / mcp_permission) — it
            // NEVER spawns a resume. So it may flip to 'running' ONLY when the
            // thread is still genuinely parked on the card
            // ('waiting_for_user_answer'). Resolving a card whose session already
            // ended (a stale click hours after idle, or the clear-at-idle sweep
            // in `emit_coding_agent_idled`) must NOT resurrect an idle/terminal
            // thread into a dead 'running' with no live session to consume it —
            // that was the zombie-`running` bug
            // (`docs/plans/2026-07-02-cc-permission-card-zombie-running.md`). Both
            // CASE arms read the pre-update `status`, so they agree.
            ThreadEvent::CodingAgentPermissionResolved { .. }
            | ThreadEvent::CommandPermissionResolved { .. }
            | ThreadEvent::McpPermissionResolved { .. } => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_user_action = NOW(), \
                     status = CASE WHEN status = {WAITING_FOR_USER_ANSWER} THEN {RUNNING} ELSE status END, \
                     last_revived_at = CASE WHEN status = {WAITING_FOR_USER_ANSWER} THEN NOW() ELSE last_revived_at END \
                     WHERE thread_id = $1",
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeSetAside { change_id, .. } => {
                crate::core::changes_projection::ChangesProjection::write_status(
                    tx,
                    change_id,
                    crate::core::changes::ChangeStatus::SetAside,
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeBroughtBack { change_id } => {
                crate::core::changes_projection::ChangesProjection::write_status(
                    tx,
                    change_id,
                    crate::core::changes::ChangeStatus::Pending,
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeWithdrawn { change_id } => {
                crate::core::changes_projection::ChangesProjection::write_status(
                    tx,
                    change_id,
                    crate::core::changes::ChangeStatus::Withdrawn,
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::SuccessorThreadSpawned { successor_thread_id, .. } => {
                let linked = sqlx::query(
                    "UPDATE thread_summaries SET successor_thread_id = $2, \
                     parent_callback_pending = FALSE \
                     WHERE thread_id = $1 AND successor_thread_id IS NULL",
                )
                .bind(thread_id)
                .bind(successor_thread_id)
                .execute(&mut **tx)
                .await?;
                if linked.rows_affected() == 0 {
                    return Err(format!(
                        "SuccessorThreadSpawned refused for thread {thread_id}: {}",
                        crate::engine::pending_handoff::ALREADY_HANDED_OFF
                    )
                    .into());
                }
                Vec::new()
            }
            ThreadEvent::ProposalWithheld { reason, agent_returns, .. } => {
                let proposed = ChangeStateKind::Proposed.as_str();
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET \
                     coding_agent_unproposed_reason = CASE \
                        WHEN coding_agent_change_state = '{proposed}' THEN NULL ELSE $2 END, \
                     coding_agent_change_state = CASE \
                        WHEN coding_agent_change_state = '{proposed}' THEN '{proposed}' \
                        ELSE '{unproposed}' END \
                     WHERE thread_id = $1",
                    unproposed = ChangeStateKind::Unproposed.as_str(),
                ))
                .bind(thread_id)
                .bind((!agent_returns).then_some(reason.as_str()))
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeQueued { .. }
            | ThreadEvent::ChangeHandoffStarted { .. }
            | ThreadEvent::ChangeUnqueued { .. } => Vec::new(),
            ThreadEvent::ChangeReverted { change_id, .. } => {
                crate::core::changes_projection::ChangesProjection::write_status(
                    tx, change_id, crate::core::changes::ChangeStatus::Reverted,
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeHardened { change_id, .. } => {
                crate::core::changes_projection::ChangesProjection::write_hardened(tx, change_id)
                    .await?;
                Vec::new()
            }
            ThreadEvent::ChangeSummarized {
                change_id,
                summary,
                description,
            } => {
                crate::core::changes_projection::ChangesProjection::write_summary(
                    tx,
                    change_id,
                    summary,
                    description,
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::MergeResolutionStarted {
                change_id,
                worktree_path,
                temp_branch,
            } => {
                crate::core::changes_projection::ChangesProjection::write_merge_started(
                    tx,
                    change_id,
                    worktree_path,
                    temp_branch,
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::MergeResolutionCleared { change_id } => {
                crate::core::changes_projection::ChangesProjection::write_merge_cleared(
                    tx, change_id,
                )
                .await?;
                Vec::new()
            }
            // INVARIANT: Child-completion fan-in landing on the parent. Clear the child's
            // `parent_callback_pending` HERE, in the same transaction as the
            // event INSERT, so the marker and the event cannot disagree. A
            // separate post-emit UPDATE (the old shape in
            // `notify_parent_if_child`) left a crash window where the typed
            // event committed but the marker didn't; the next terminal event
            // then re-fired the fan-in and handed the parent a duplicate
            // completion card. The parent's wake-up + agentic-loop replay
            // handle the actual side effect — no other projection state
            // changes for the parent row itself.
            //
            // The same write settles a stopped child. It lands on the child's
            // row, outside this event's own attention sample. So the child's
            // ancestors are reconciled after the climb, and its aggregate is
            // rebroadcast.
            //
            // A filed report settles the child the same way. It writes nothing
            // on the parent's own row: the parent does not wake.
            ThreadEvent::ChildThreadCompleted {
                child_thread_id, ..
            }
            | ThreadEvent::ChildReportFiled {
                child_thread_id, ..
            } => {
                if settle_parent_callback(tx, *child_thread_id).await? {
                    extra_ancestors.push(*child_thread_id);
                    descendant_recount_anchors.extend(parent_of(tx, *child_thread_id).await?);
                }
                Vec::new()
            }
            ThreadEvent::ChildThreadStopped { .. } => Vec::new(),
            ThreadEvent::ChildThreadDetached {
                child_thread_id, ..
            } => {
                extra_ancestors
                    .extend(detach_child_from_parent(tx, thread_id, *child_thread_id).await?);
                descendant_recount_anchors.push(thread_id);
                Vec::new()
            }
            ThreadEvent::VoiceSessionStarted { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries \
                     SET last_activity = NOW(), last_user_action = NOW() \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::SpokenMessageReceived { text, .. } => {
                sqlx::query(
                    "UPDATE thread_summaries \
                     SET last_activity = NOW(), last_user_action = NOW(), \
                         has_response = TRUE, \
                         first_message = COALESCE(first_message, $2) \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .bind(text)
                .execute(&mut **tx)
                .await?;
                a_call_makes_the_thread_real(tx, thread_id).await?
            }
            ThreadEvent::SpokenReplyGenerated { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries \
                     SET last_activity = NOW(), last_agent_action = NOW(), \
                         has_response = TRUE \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                a_call_makes_the_thread_real(tx, thread_id).await?
            }
            ThreadEvent::CodingAgentCommandReceived { text, agent, repo_id, app_folder, .. } => {
                sqlx::query(&format!(
                    r#"INSERT INTO thread_summaries (thread_id, first_message, source, initiator, created_at, last_activity, last_user_action, message_count, status, state, has_response, is_coding_agent, coding_agent, cc_repo_id, coding_agent_kind, coding_agent_folder)
                       VALUES ($1, $2, 'claude_code', 'user', NOW(), NOW(), NOW(), 0, {IDLE}, 'active', TRUE, TRUE, $3, $4,
                               CASE WHEN $5::text IS NOT NULL THEN 'app' END, $5)
                       ON CONFLICT (thread_id) DO UPDATE
                       SET last_activity = NOW(),
                           last_user_action = NOW(),
                           first_message = COALESCE(thread_summaries.first_message, EXCLUDED.first_message),
                           source = CASE WHEN thread_summaries.state = 'composing' THEN EXCLUDED.source ELSE thread_summaries.source END,
                           is_coding_agent = TRUE,
                           coding_agent = COALESCE(thread_summaries.coding_agent, EXCLUDED.coding_agent),
                           cc_repo_id = COALESCE(thread_summaries.cc_repo_id, EXCLUDED.cc_repo_id),
                           coding_agent_kind = COALESCE(thread_summaries.coding_agent_kind, EXCLUDED.coding_agent_kind),
                           coding_agent_folder = COALESCE(thread_summaries.coding_agent_folder, EXCLUDED.coding_agent_folder),
                           has_response = thread_summaries.has_response OR thread_summaries.state = 'composing',
                           state = 'active'
                       WHERE thread_summaries.state != 'discarded'"#
                ))
                .bind(thread_id)
                .bind(text)
                .bind(agent.as_str())
                .bind(repo_id)
                .bind(app_folder)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::UserOnlyMessagePosted { text, .. } => {
                sqlx::query(&format!(
                    r#"INSERT INTO thread_summaries (thread_id, first_message, source, initiator, created_at, last_activity, last_user_action, message_count, status, state, has_response)
                       VALUES ($1, $2, 'chat', 'user', NOW(), NOW(), NOW(), 0, {IDLE}, 'active', TRUE)
                       ON CONFLICT (thread_id) DO UPDATE
                       SET last_activity = NOW(),
                           last_user_action = NOW(),
                           has_response = TRUE,
                           first_message = COALESCE(thread_summaries.first_message, EXCLUDED.first_message),
                           source = CASE WHEN thread_summaries.state = 'composing' THEN 'chat' ELSE thread_summaries.source END,
                           state = 'active'
                       WHERE thread_summaries.state != 'discarded'"#
                ))
                .bind(thread_id)
                .bind(text)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::PermissionGranted {
                permission_id,
                action,
                breadth,
                lifetime,
                holder,
                request_tool_use_id,
                pattern,
                picked_by,
            } => {
                crate::engine::thread_events::permission::projection::write_granted(
                    tx,
                    thread_id,
                    crate::engine::thread_events::permission::projection::Granted {
                        permission_id: *permission_id,
                        action: *action,
                        breadth: breadth.as_deref(),
                        lifetime: *lifetime,
                        holder,
                        request_tool_use_id,
                        pattern: pattern.as_deref(),
                        picked_by,
                    },
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::PermissionSpent { permission_id } => {
                crate::engine::thread_events::permission::projection::write_spent(tx, *permission_id).await?;
                Vec::new()
            }
            ThreadEvent::PermissionRestored { spent, restored } => {
                crate::engine::thread_events::permission::projection::write_restored(tx, thread_id, *spent, *restored).await?;
                Vec::new()
            }
            ThreadEvent::PermissionRevoked { permission_id } => {
                crate::engine::thread_events::permission::projection::write_ended(tx, *permission_id, "revoked")
                    .await?;
                Vec::new()
            }
            ThreadEvent::CredentialRequested { .. }
            | ThreadEvent::BatchItemRan { .. }
            | ThreadEvent::PermissionRequestPrepared { .. }
            | ThreadEvent::PluginInstallRequested { .. }
            | ThreadEvent::PluginUninstallRequested { .. }
            | ThreadEvent::EmailConfirmRequested { .. }
            | ThreadEvent::CodingAgentCommandAnswered { .. }
            | ThreadEvent::CodingAgentMcpStatusesReported { .. }
            | ThreadEvent::AgentSignInNeeded { .. }
            | ThreadEvent::TerminalOpened { .. }
            | ThreadEvent::TerminalExited { .. }
            | ThreadEvent::TerminalCommandSuggested { .. }
            | ThreadEvent::OAuthAuthorizationRequested { .. }
            | ThreadEvent::AgentSignInRequested { .. }
            | ThreadEvent::FormRequestResolved { .. }
            | ThreadEvent::McpConsentRequested { .. }
            | ThreadEvent::UserQuestionQueued { .. }
            | ThreadEvent::RootThreadDriverTaken
            | ThreadEvent::RootThreadDriverReleased
            | ThreadEvent::RootThreadGateEscalated { .. }
            | ThreadEvent::RootThreadWatched { .. }
            | ThreadEvent::RootThreadUnwatched
            | ThreadEvent::AgentPaused { .. }
            | ThreadEvent::AgentResumed { .. }
            | ThreadEvent::CumulativeTextUpdated { .. }
            | ThreadEvent::LlmCallRetried { .. }
            | ThreadEvent::PreambleCompleted
            | ThreadEvent::PushNotificationRequested
            | ThreadEvent::AppUiRefreshRequested { .. }
            | ThreadEvent::AppUiCaptureRequested { .. }
            | ThreadEvent::WidgetFitRequested { .. }
            | ThreadEvent::NavigationRequested { .. }
            | ThreadEvent::CodingAgentThreadSpawned { .. }
            | ThreadEvent::CodingAgentDiffChanged { .. }
            | ThreadEvent::ChildrenCountChanged { .. }
            | ThreadEvent::MissingHardeningDetected { .. }
            | ThreadEvent::CodingAgentSettingsChanged { .. }
            | ThreadEvent::ChatSettingsChanged { .. }
            | ThreadEvent::WorktreeCleaned { .. }
            | ThreadEvent::ImageUploaded { .. }
            | ThreadEvent::ContextCaptured { .. }
            | ThreadEvent::ModelCallFailed { .. }
            | ThreadEvent::CodingAgentCompacted { .. }
            | ThreadEvent::CodingAgentModelSubstituted { .. }
            | ThreadEvent::PlanProposed { .. }
            | ThreadEvent::PlanApproved { .. }
            | ThreadEvent::ChangeApplyRequested { .. }
            | ThreadEvent::PlanStarted { .. }
            | ThreadEvent::HandoffCheckpointReached { .. }
            | ThreadEvent::SuccessorOfferDeclined { .. }
            | ThreadEvent::SuccessorRequested { .. }
            | ThreadEvent::ContextCeilingSet { .. }
            | ThreadEvent::ContextCeilingReached { .. }
            | ThreadEvent::SideQuestionAsked { .. }
            | ThreadEvent::SideQuestionAnswered { .. }
            | ThreadEvent::SideQuestionFailed { .. }
            | ThreadEvent::SideQuestionDismissed { .. }
            | ThreadEvent::MessageHeld { .. }
            | ThreadEvent::HeldMessageReleased { .. }
            | ThreadEvent::CodingAgentInputRead { started_turn: false, .. }
            | ThreadEvent::CodingAgentInputDiscarded { .. }
            | ThreadEvent::ChildReportTaken { .. }
            | ThreadEvent::ContextDismissed { .. }
            | ThreadEvent::ContextKeptOpen { .. }
            // INVARIANT: Background bash lifecycle events. The paired ToolCalled /
            // ToolResult for the spawn already bumped last_activity. The
            // completion event fires asynchronously from a tokio watcher,
            // possibly long after the LLM turn ended — bumping last_activity
            // there would surface a quiet thread as "active" with nothing
            // for the user to look at. Persisted for the audit trail and
            // the event-store fallback in `bash_output`.
            | ThreadEvent::BackgroundBashStarted { .. }
            | ThreadEvent::BackgroundBashCompleted { .. }
            | ThreadEvent::ConfinementRunStarted { .. }
            | ThreadEvent::OwnerApprovalRequested { .. }
            | ThreadEvent::OwnerApprovalSpent { .. }
            | ThreadEvent::MachineMadeWorkspaceChanged { .. }
            | ThreadEvent::ImageDescribed { .. }
            // INVARIANT: The cached older-turn summary. Persisted so the next turn reads
            // it instead of re-rolling. It says nothing about thread activity.
            // Some paths write it before the turn's own starter event, so it
            // must bump nothing.
            | ThreadEvent::ConversationSummarized { .. }
            | ThreadEvent::TodoListWritten { .. }
            | ThreadEvent::WorkingUnderstandingWritten { .. }
            | ThreadEvent::CommandCheckpointed { .. }
            | ThreadEvent::CommandCheckpointReverted { .. }
            | ThreadEvent::WidgetShown { .. }
            | ThreadEvent::WidgetPinned { .. }
            | ThreadEvent::WidgetUnpinned { .. }
            | ThreadEvent::InlineWidgetSized { .. }
            | ThreadEvent::HomeBoardArranged { .. }
            | ThreadEvent::VoiceSessionEnded { .. }
            | ThreadEvent::WorkDelegated { .. }
            | ThreadEvent::MergeConflictDetected { .. }
            | ThreadEvent::ChangeApplyFailed { .. }
            | ThreadEvent::TriggerRunInterrupted { .. }
            | ThreadEvent::CodingAgentBillingObserved { .. } => Vec::new(),
        };

        let thread_type = Self::get_thread_type(&mut **tx, &thread_id).await?;
        let current = Self::get_current_section(tx, &thread_id).await?;
        let (source, trigger_go_to_review, is_saved, read_requested): (
            Option<String>,
            bool,
            bool,
            bool,
        ) = sqlx::query_as(
            "SELECT source, COALESCE(trigger_go_to_review, FALSE), is_saved, read_requested \
                 FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or((None, false, false, false));
        let is_unattended = if source.as_deref() != Some("trigger")
            || trigger_go_to_review
            || is_saved
            || read_requested
        {
            false
        } else {
            let human = ActorMode::Human.as_str();
            let latest_start_sql = format!(
                "SELECT event_type FROM events WHERE aggregate_id = $1::text \
                 AND (event_type = 'TriggerStarted' OR ({})) \
                 ORDER BY sequence DESC LIMIT 1",
                crate::engine::thread_events::HUMAN_MESSAGE_SQL,
            );
            let latest_start: Option<String> = sqlx::query_scalar(&latest_start_sql)
                .bind(thread_id)
                .bind(human)
                .fetch_optional(&mut **tx)
                .await?;
            latest_start.as_deref() != Some("MessageReceived")
        };
        match resolve_transition(event.event_type(), thread_type, current, is_unattended) {
            Ok(mut transition) => {
                // INVARIANT: CodingAgentIdled(has_changes=false) after apply/discard is a housekeeping
                // event — the section is already 'inbox' so setting it again is redundant.
                // When section is Default (first idle with no changes), let the transition
                // through so the thread surfaces in REVIEW — the user needs to know the
                // Claude Code session completed.
                if matches!(
                    event,
                    ThreadEvent::CodingAgentIdled {
                        has_changes: false,
                        ..
                    }
                ) && current == ArchiveState::Inbox
                {
                    transition.new_section = None;
                }
                if matches!(
                    event,
                    ThreadEvent::ChangeProposed {
                        set_aside: true,
                        ..
                    }
                ) {
                    transition.new_section = None;
                }
                Self::apply_transition(tx, thread_id, &transition).await?;
            }
            Err(v) => {
                crate::log_legacy!("[EventBus] {}", v);
                return Err(Box::new(v));
            }
        }

        if !skip_blocking_sample {
            extra_ancestors.extend(
                reconcile_awaited_from(tx, thread_id)
                    .await?
                    .into_iter()
                    .filter(|id| *id != thread_id),
            );
        }

        // INVARIANT: Every walk that writes ancestor rows without their recount lock runs
        // after the climb. The climb takes each lock before its row, as a
        // sibling's recount does. An earlier walk would leave the climb waiting
        // on a lock while holding the row that sibling waits on: a deadlock.
        for anchor in descendant_recount_anchors {
            extra_ancestors.extend(reconcile_descendant_counts_from(tx, anchor).await?);
        }

        // INVARIANT: Sample is_blocking-relevant state AFTER both the metadata match and
        // the section transition, then walk ancestors and apply the delta to
        // `blocking_descendant_count`. A missing prev_sample is treated as
        // "was not blocking" so newly-inserted child rows propagate +1
        // cleanly when their first event lands in a blocking state. Skipped
        // for per-token streaming events (see `skip_blocking_sample` above).
        let mut affected_ancestors = if skip_blocking_sample || skip_blocking_propagate {
            Vec::new()
        } else {
            let curr_sample = load_blocking_sample(tx, thread_id).await?;
            let (was_blocking, was_attention) = prev_sample
                .as_ref()
                .map(|s| (s.is_blocking(), s.is_attention_needing()))
                .unwrap_or((false, false));
            if let Some(c) = curr_sample {
                let blocking_delta = (c.is_blocking() as i32) - (was_blocking as i32);
                let attention_delta = (c.is_attention_needing() as i32) - (was_attention as i32);
                propagate_blocking_change(tx, thread_id, blocking_delta, attention_delta).await?
            } else {
                Vec::new()
            }
        };

        if !extra_ancestors.is_empty() {
            affected_ancestors.extend(extra_ancestors);
            affected_ancestors.sort();
            affected_ancestors.dedup();
        }

        Ok((match_side_effects, affected_ancestors))
    }
}

#[cfg(test)]
#[path = "event_bus_projection_thread_tests.rs"]
mod tests;
