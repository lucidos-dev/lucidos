//! The `thread_summaries` projection: `EventBus::update_thread_projection`.
//!
//! One cohesive `match` over every `ThreadEvent` variant — each arm runs the
//! metadata update for that event inside the caller's transaction, then a
//! shared step validates the section transition via the lifecycle contract and
//! propagates the blocking/attention deltas to ancestors. Kept as a single
//! unit: the arms share the `extra_ancestors` / `skip_blocking_propagate` /
//! `prev_sample` locals, so splitting them would mean threading that mutable
//! state through extracted helpers (a logic change this refactor avoids).

use uuid::Uuid;

use super::super::{BusEvent, EventBus, CLEAR_CODING_AGENT_FLAGS, STATUS_FROM_PROPOSED_CHANGE};
use super::propagation::{
    load_blocking_sample, propagate_blocking_change, reconcile_parent_active_children_count,
    reconcile_proposal_lifecycle_end, reincrement_parent_active_count_if_revived,
};
use crate::core::store::LegacyInitiator;
use crate::engine::thread_events::{ActorMode, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_lifecycle::{resolve_transition, ArchiveState};

impl EventBus {
    /// Returns side-effect events to emit after the main transaction commits,
    /// alongside the list of ancestor thread IDs whose
    /// `blocking_descendant_count` changed (empty when this event didn't flip
    /// the emitting thread's `is_blocking` predicate). The caller rebroadcasts
    /// each ancestor's aggregate over SSE — without that, the frontend's
    /// `meta.blockingDescendantCount` stays stale until a full
    /// `/api/v1/threads` reload, breaking live UI gates that depend on it
    /// (e.g. the "Archive" button hides while a CC sub-thread is running).
    ///
    /// Structure: Step 1 runs metadata updates (the match statement).
    /// Step 2 validates and applies section transitions via the lifecycle contract.
    /// This ensures upsert events create the row before the contract checks it.
    pub(crate) async fn update_thread_projection(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: Uuid,
        event: &ThreadEvent,
        meta: &EventMeta,
    ) -> Result<(Vec<BusEvent>, Vec<Uuid>), Box<dyn std::error::Error + Send + Sync>> {
        // Per-token streaming events (TextStreamed, ThoughtStreamed,
        // CodingAgentTextStreamed) fire many times per turn — a 30s CC turn
        // streams ~200 tokens. Their projection arms only set status='running'
        // (idempotent on already-Running threads), so the blocking predicate
        // cannot flip. Short-circuit the before/after sample + ancestor walk
        // to drop ~400 SELECTs per turn from the hot path. The scheduler uses
        // the same predicate to skip its trigger matcher (see
        // `crates/lucidos-engine/src/scheduler/mod.rs`).
        let skip_blocking_sample = event.is_per_token_streaming();

        // Sample is_blocking-relevant state BEFORE the projection mutates it,
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

        // Extra ancestors to rebroadcast over SSE — populated by arms that
        // perform their own defense-in-depth reconcile. Merged into
        // `affected_ancestors` at the bottom and deduplicated.
        let mut extra_ancestors: Vec<Uuid> = Vec::new();
        // Arms that fully recompute `blocking_descendant_count` from ground
        // truth (Apply/Discard) flip this so the function-boundary
        // `propagate_blocking_change` doesn't double-subtract on top of
        // the recompute.
        let mut skip_blocking_propagate = false;

        // Step 1: Run metadata updates
        let match_side_effects: Vec<BusEvent> = match event {
            // Thread start events — upsert the summary row
            ThreadEvent::MessageReceived { text, parent_thread_id, spawning_event_id, mode, .. } => {
                let source = meta.channel.as_ref().map(|c| c.as_str()).unwrap_or("chat");
                // Map ActorMode to the legacy two-state `initiator` column:
                // Human → "user", Agent | Engine → "system". The column was
                // never tri-state; promoting it would require a migration and
                // a frontend type change. See `LegacyInitiator` in
                // core/store/threads.rs for the matching read path.
                let msg_initiator = match mode {
                    ActorMode::Human => LegacyInitiator::User.as_str(),
                    ActorMode::Agent | ActorMode::Engine => LegacyInitiator::System.as_str(),
                };
                // A human-typed message is a user action; an agent/engine-driven
                // one is agent activity. Drives which attributed-recency column
                // the follow-up UPDATE bumps below.
                let human = matches!(mode, ActorMode::Human);
                // Compute child depth and inherit initiator from parent —
                // a non-Human parent forces "system" on its descendants.
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
                sqlx::query(
                    r#"INSERT INTO thread_summaries (thread_id, first_message, source, initiator, created_at, last_activity, message_count, parent_thread_id, spawning_event_id, depth, status, last_revived_at, state)
                       VALUES ($1, $2, $3, $6, NOW(), NOW(), 1, $4, $7, $5, 'running', NOW(), 'active')
                       ON CONFLICT (thread_id) DO UPDATE
                       SET last_activity = NOW(),
                           -- Attributed recency: a human follow-up bumps
                           -- last_user_action (the drawer sort key); an
                           -- agent/engine follow-up bumps last_agent_action.
                           last_user_action = CASE WHEN $8 THEN NOW() ELSE thread_summaries.last_user_action END,
                           last_agent_action = CASE WHEN $8 THEN thread_summaries.last_agent_action ELSE NOW() END,
                           message_count = thread_summaries.message_count + 1,
                           status = 'running',
                           last_revived_at = NOW(),
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
                           compose_text = '',
                           compose_images = '[]'::jsonb,
                           compose_mode = NULL
                       -- Defense in depth: refuse to resurrect a discarded thread if a
                       -- stale MessageReceived slips past the API-layer guard.
                       WHERE thread_summaries.state != 'discarded'"#,
                )
                .bind(thread_id)
                .bind(text)
                .bind(source)
                .bind(parent_thread_id)
                .bind(child_depth)
                .bind(initiator)
                .bind(spawning_event_id)
                .bind(human)
                .execute(&mut **tx)
                .await?;

                // If this message has a parent, increment the parent's active_children_count.
                // Parents are always Chat threads — CC threads are always children.
                if let Some(pid) = parent_thread_id {
                    sqlx::query(
                        "UPDATE thread_summaries SET active_children_count = active_children_count + 1, \
                         total_children_count = total_children_count + 1 WHERE thread_id = $1"
                    )
                    .bind(pid)
                    .execute(&mut **tx)
                    .await?;
                } else {
                    // Follow-up MessageReceived: payload's parent_thread_id
                    // is set only on the spawning message, so a follow-up
                    // bypasses the inline +1 above and must re-increment
                    // via the helper if the prior terminal event already
                    // decremented the parent.
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
            // Claude Code session lifecycle — session start/continuation don't update last_activity
            // (the first real activity event will set it).
            ThreadEvent::SessionStarted { .. } | ThreadEvent::ContinuationStarted { .. } => {
                let source = meta.channel.as_ref().map(|c| c.as_str()).unwrap_or("claude_code");
                // Extract repo_id + coding-agent-kind fields from SessionStarted;
                // ContinuationStarted carries none of these (it's a marker for
                // a fresh CC subprocess on an existing thread — the kind/folder
                // were already locked in by the first SessionStarted).
                // `app_id` from SessionStarted intentionally not read here —
                // the app id is the last segment of `coding_agent_folder`
                // (`<ws>/data/apps/<id>/`), so consumers reconstruct it
                // rather than storing it as its own column. See
                // `change_ops::load_apply_kind_context`.
                let (session_repo_id, session_kind, session_folder, session_agent) = match &event {
                    ThreadEvent::SessionStarted {
                        repo_id,
                        coding_agent_kind,
                        coding_agent_folder,
                        coding_agent,
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
                    ),
                    _ => (None, None, None, None),
                };
                sqlx::query(
                    r#"INSERT INTO thread_summaries (thread_id, source, is_coding_agent, created_at, last_activity, message_count, status, last_revived_at, cc_repo_id, coding_agent_kind, coding_agent_folder, coding_agent, state)
                       VALUES ($1, $2, TRUE, NOW(), NOW(), 0, 'running', NOW(), $3, $4, $5, $6, 'active')
                       ON CONFLICT (thread_id) DO UPDATE
                       SET is_coding_agent = TRUE, source = $2,
                           initiator = COALESCE(thread_summaries.initiator, 'unknown'),
                           -- Existing value wins: a thread's repo is locked at first SessionStarted.
                           -- The chat handler enforces that follow-ups can't pick a different repo,
                           -- but defend the projection so any drift (legacy data, replay) doesn't
                           -- silently flip the thread to a different repo's skill set.
                           cc_repo_id = COALESCE(thread_summaries.cc_repo_id, $3),
                           coding_agent_kind = COALESCE(thread_summaries.coding_agent_kind, $4),
                           coding_agent_folder = COALESCE(thread_summaries.coding_agent_folder, $5),
                           -- Same lock for the backend: a thread can never flip
                           -- between Claude Code and Codex mid-conversation.
                           coding_agent = COALESCE(thread_summaries.coding_agent, $6),
                           state = 'active',
                           compose_text = '',
                           compose_images = '[]'::jsonb,
                           compose_mode = NULL
                       -- Defense in depth (see MessageReceived above for rationale).
                       WHERE thread_summaries.state != 'discarded'"#,
                )
                .bind(thread_id)
                .bind(source)
                .bind(session_repo_id)
                .bind(session_kind)
                .bind(session_folder)
                .bind(session_agent)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::TriggerStarted { trigger_id, trigger_name, go_to_review, .. } => {
                let source = meta.channel.as_ref().map(|c| c.as_str()).unwrap_or("trigger");
                sqlx::query(
                    r#"INSERT INTO thread_summaries (thread_id, first_message, source, initiator, created_at, last_activity, message_count, status, last_revived_at, trigger_id, trigger_name, trigger_go_to_review, state)
                       VALUES ($1, $2, $3, 'system', NOW(), NOW(), 1, 'running', NOW(), $4, $5, $6, 'active')
                       ON CONFLICT (thread_id) DO UPDATE
                       SET last_activity = NOW(),
                           -- A trigger firing is autonomous agent activity.
                           last_agent_action = NOW(),
                           message_count = thread_summaries.message_count + 1,
                           status = 'running',
                           last_revived_at = NOW(),
                           state = 'active',
                           trigger_id = COALESCE(thread_summaries.trigger_id, EXCLUDED.trigger_id),
                           trigger_name = COALESCE(thread_summaries.trigger_name, EXCLUDED.trigger_name)
                       -- Defense in depth (see MessageReceived above for rationale).
                       WHERE thread_summaries.state != 'discarded'"#,
                )
                .bind(thread_id)
                .bind(trigger_name.as_deref())
                .bind(source)
                .bind(trigger_id.as_str())
                .bind(trigger_name.as_deref())
                .bind(*go_to_review)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            // Activity events — update last_activity + status
            ThreadEvent::ResponseGenerated { .. } => {
                // Normal completion — go idle (or waiting if CC has a pending proposal).
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_agent_action = NOW(), \
                     has_response = TRUE, \
                     status = {STATUS_FROM_PROPOSED_CHANGE} WHERE thread_id = $1"
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                // In-tx parent decrement: mirrors the out-of-tx
                // `notify_parent_if_child` path so the captured aggregate
                // (and the affected-ancestor broadcast that piggy-backs it)
                // already reflects the post-decrement count. Without this
                // the IN-TX broadcast carries the stale pre-decrement value
                // and a lost out-of-tx broadcast pins the parent in
                // ACTIVE / "waiting on children" until the next event
                // refreshes its ancestor aggregate. See
                // `test_cc_idle_in_tx_broadcast_carries_decremented_parent_active`.
                if let Some(pid) =
                    reconcile_parent_active_children_count(tx, thread_id).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::ResponseAborted { cause, .. } => {
                // Status mapping lives on `AbortCause::status_sql()` so the
                // cause-classification stays next to `is_transient()` on the
                // enum. Most aborts surface a red `failed` indicator;
                // stale-settle is engine cleanup of an already-gone process
                // and uses the cancel-style `idle`/`waiting` mapping.
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_agent_action = NOW(), \
                     has_response = TRUE, \
                     status = {} WHERE thread_id = $1",
                    cause.status_sql()
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                // Mirror the out-of-tx `notify_parent_if_child` gate: only
                // non-transient causes are terminal (transient aborts are
                // mid-retry — the resumed child's eventual SessionStarted
                // would land before the parent's counter recovers if we
                // reconciled here too). See ResponseGenerated for the
                // IN-TX vs out-of-tx broadcast rationale.
                if !cause.is_transient() {
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
                // Authoritative for `is_external_repo` and `coding_agent_has_diff`
                // (git-truth signal — reflects whether the branch has any
                // committed diff against main, not the proposal lifecycle).
                // `requires_restart` stays the aggregate `ChangeProposed`'s job
                // — same reason `coding_agent_proposed` does. `has_response =
                // TRUE` is the CC equivalent of `ResponseGenerated` for
                // surfacing the thread in `get_recent_threads`.
                //
                // `coding_agent_has_diff = has_changes` is what fixes the
                // "Diff button says 'No changes on this branch' when the
                // branch has commits" bug — `has_changes` is `wt_has_changes`
                // computed from `branch_changed_files`, which is the same
                // git-truth the seed/refresh paths consult. Setting it here
                // means the Diff button is enabled the moment CC idles with
                // a diff, regardless of whether `ChangeProposed` has fired.
                //
                // The `status` CASE preserves a `failed` status: a CC turn that
                // ends in failure emits `ResponseFailed` (→ status='failed')
                // immediately followed by this `CodingAgentIdled` in the SAME
                // turn (`classify_result` returns `emit_idle=true` for every
                // non-shutdown Result, including the Failed kind). Without the
                // CASE this bookkeeping idle would downgrade 'failed' → 'idle'
                // and erase the red error dot in the thread list. The terminal
                // event owns the turn's status; this idle only parks the thread
                // when the turn didn't already fail. A later follow-up
                // (`CodingAgentUserMessageSent` → 'running') clears 'failed'
                // before the next idle, so the failure can't wedge the thread.
                // Mirrors the `CASE WHEN status='running'` preservation the
                // `ChangeProposed` arm uses for the same class of reason.
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_agent_action = NOW(), \
                     has_response = TRUE, \
                     coding_agent_is_external_repo = $2, \
                     coding_agent_has_diff = $3, \
                     status = CASE WHEN status = 'failed' THEN 'failed' \
                                   ELSE {STATUS_FROM_PROPOSED_CHANGE} END \
                     WHERE thread_id = $1",
                ))
                .bind(thread_id)
                .bind(is_external_repo)
                .bind(has_changes)
                .execute(&mut **tx)
                .await?;
                // See ResponseGenerated for the IN-TX broadcast rationale.
                // `reconcile_parent_active_children_count` is idempotent —
                // safe even on a second CC idle whose out-of-tx decrement
                // was already deduped via `parent_callback_sent`.
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
                     {CLEAR_CODING_AGENT_FLAGS}, \
                     status = 'idle' \
                     WHERE thread_id = $1",
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
                // Defense in depth: the runtime `CodingAgentIdled`
                // decrement of `active_children_count` runs out-of-tx in
                // `notify_parent_if_child`; a transient failure leaves the
                // counter stuck non-zero and the parent pinned in ACTIVE
                // forever. Recompute both counters from ground truth here.
                reconcile_proposal_lifecycle_end(tx, thread_id, &mut extra_ancestors).await?;
                skip_blocking_propagate = true;
                Vec::new()
            }
            ThreadEvent::ChangeDiscarded { change_id, .. } => {
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_user_action = NOW(), \
                     {CLEAR_CODING_AGENT_FLAGS}, \
                     status = 'idle' \
                     WHERE thread_id = $1",
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                crate::core::changes_projection::ChangesProjection::write_status(
                    tx, change_id, "discarded",
                )
                .await?;
                // See ChangeApplied for the defense-in-depth rationale.
                reconcile_proposal_lifecycle_end(tx, thread_id, &mut extra_ancestors).await?;
                skip_blocking_propagate = true;
                Vec::new()
            }

            // Message count increment + activity (CC user messages and mid-flight injections)
            ThreadEvent::CodingAgentUserMessageSent { .. }
            | ThreadEvent::UserPromptInjected { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_user_action = NOW(), message_count = message_count + 1, status = 'running', last_revived_at = NOW() WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                // Revive: if the child was previously not 'running' (typically
                // 'idle' after a CodingAgentIdled that already decremented the
                // parent via notify_parent_if_child), the user follow-up flips
                // it back. Bounce the parent's active_children_count up so the
                // parent's status dot + collapsed sub-thread count track live state.
                if let Some(pid) =
                    reincrement_parent_active_count_if_revived(tx, thread_id, &prev_sample).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }

            // Title events
            ThreadEvent::ThreadTitleGenerated { title } | ThreadEvent::ThreadTitleRenamed { title } => {
                sqlx::query(
                    "UPDATE thread_summaries SET title = $2 WHERE thread_id = $1",
                )
                .bind(thread_id)
                .bind(title)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            // Save/unsave
            ThreadEvent::ThreadSaved => {
                sqlx::query(
                    "UPDATE thread_summaries SET is_saved = TRUE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadUnsaved => {
                sqlx::query(
                    "UPDATE thread_summaries SET is_saved = FALSE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            ThreadEvent::ThreadArchived => {
                // Clear is_saved so display priority doesn't keep the row in
                // Saved (is_saved=true wins over archive_state='archived').
                // The `archive_state` flip itself is written by the contract
                // layer (`event_bus::apply_transition` →
                // `thread_lifecycle::resolve_transition` → `to_archived`) —
                // this projection only touches the orthogonal compose-state
                // (`state`), notifications gating (`is_saved`), and CC-flag
                // teardown columns. Children counters are NOT cleared —
                // they're a cache of real child events, and family-lift
                // routing surfaces this parent again whenever any descendant
                // is still active; zeroing them would hide the disclosure
                // chevron (gated on `totalChildrenCount > 0`) and leave the
                // user unable to collapse the lifted family.
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET status = 'idle', \
                     is_saved = FALSE, \
                     {CLEAR_CODING_AGENT_FLAGS} \
                     WHERE thread_id = $1",
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadStarted { mode, .. } => {
                // Compose-time thread creation — the row appears in
                // `thread_summaries` with `state='composing'` so the frontend
                // can render it as a draft via cross-device SSE. Default
                // initiator is `user` since only humans open compose. Source
                // mirrors the user's chosen mode so a draft that auto-archives
                // before being sent still surfaces with the correct channel
                // pill. Send events later re-assert source via the
                // `source = 'chat'`-keyed CASE in MessageReceived.
                let source = if mode == "claude_code" { "claude_code" } else { "chat" };
                sqlx::query(
                    r#"INSERT INTO thread_summaries
                        (thread_id, initiator, source, created_at, last_activity, message_count,
                         state, compose_mode, status)
                       VALUES ($1, 'user', $3, NOW(), NOW(), 0, 'composing', $2, 'idle')
                       ON CONFLICT (thread_id) DO NOTHING"#,
                )
                .bind(thread_id)
                .bind(mode)
                .bind(source)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ThreadDiscarded { .. } => {
                // Terminal transition. Only valid from `composing` — the
                // state-machine guard at the API boundary already rejected
                // discard from the post-send states (active with archive_state
                // either 'inbox' or 'archived'), so this UPDATE is safe to run
                // without re-checking. Compose fields are wiped so a stale
                // SSE replay can't show ghost text. archive_state moves to
                // 'archived' to keep the invariant that no discarded row
                // lingers in inbox-scoped queries.
                sqlx::query(
                    "UPDATE thread_summaries SET state = 'discarded', \
                     archive_state = 'archived', \
                     compose_text = '', compose_images = '[]'::jsonb, compose_mode = NULL \
                     WHERE thread_id = $1 AND state = 'composing'",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }

            // Events that update status but no other metadata.
            ThreadEvent::ResponseCanceled { .. } => {
                // User canceled — go idle (or waiting if there's a pending proposal).
                // Set has_response so the thread appears in archive (a canceled
                // response is still a response — the user should see the thread).
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET has_response = TRUE, \
                     status = {STATUS_FROM_PROPOSED_CHANGE} WHERE thread_id = $1"
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                // See ResponseGenerated for the IN-TX broadcast rationale.
                if let Some(pid) =
                    reconcile_parent_active_children_count(tx, thread_id).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::ResponseFailed { .. } => {
                // Failed response — distinct from 'waiting' (which means CC has
                // changes to review) so the UI can render an error indicator.
                // Set has_response so the thread stays visible.
                sqlx::query(
                    "UPDATE thread_summaries SET last_agent_action = NOW(), has_response = TRUE, status = 'failed' WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                // See ResponseGenerated for the IN-TX broadcast rationale.
                if let Some(pid) =
                    reconcile_parent_active_children_count(tx, thread_id).await?
                {
                    extra_ancestors.push(pid);
                }
                Vec::new()
            }
            ThreadEvent::SessionEnded { reason } => {
                // Transient reasons (StaleResume) are mid-retry — the chat
                // handler is about to spawn a fresh session within the same
                // request. Flipping to terminal here would render the exchange
                // as "Aborted" until the retry's SessionStarted lands.
                if !reason.is_transient() {
                    sqlx::query(&format!(
                        "UPDATE thread_summaries SET has_response = TRUE, \
                         status = {STATUS_FROM_PROPOSED_CHANGE} WHERE thread_id = $1"
                    ))
                    .bind(thread_id)
                    .execute(&mut **tx)
                    .await?;
                    // See ResponseGenerated for the IN-TX broadcast rationale.
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
                // Trigger run done — go idle. Set has_response so the thread
                // appears in get_recent_threads (which filters has_response=TRUE).
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_agent_action = NOW(), has_response = TRUE, status = 'idle' WHERE thread_id = $1",
                )
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
                ..
            } => {
                // Aggregate emit (non-empty change_id) is the only writer of
                // coding_agent_proposed. Legacy per-commit events (empty
                // change_id, commit_sha set) survive only in historical rows
                // and stay inert in thread_summaries — UPDATE-only into the
                // existing changes row, so replay can't resurrect orphan chips.
                use crate::core::changes_projection::ChangesProjection;
                debug_assert!(
                    !change_id.is_empty() || commit_sha.is_some(),
                    "ChangeProposed with neither change_id nor commit_sha is malformed"
                );
                if !change_id.is_empty() {
                    // CASE preserves 'running' so a follow-up turn overlapping
                    // the emit doesn't yank the thread back to 'idle' while
                    // CC is still mid-stream. Else 'idle' — a proposed change
                    // is an artifact for review, not a parked loop. The
                    // pending-review state is carried by coding_agent_proposed,
                    // not by `status = 'waiting'`.
                    sqlx::query(
                        "UPDATE thread_summaries SET coding_agent_proposed = TRUE, \
                         coding_agent_requires_restart = $2, coding_agent_has_diff = TRUE, \
                         status = CASE WHEN status = 'running' THEN status ELSE 'idle' END \
                         WHERE thread_id = $1",
                    )
                    .bind(thread_id)
                    .bind(*requires_restart)
                    .execute(&mut **tx)
                    .await?;
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
            ThreadEvent::MergeConflictDetected { .. } => {
                // Merge conflict — mark as applying.
                sqlx::query(
                    "UPDATE thread_summaries SET coding_agent_applying = TRUE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeApplyFailed { .. } => {
                // Apply failed — clear applying flag, stay waiting.
                sqlx::query(
                    "UPDATE thread_summaries SET coding_agent_applying = FALSE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::CodingAgentPromptSent { text, .. } => {
                // Empty prompt = no agent intent → no status change. Real prompts
                // always carry text (user follow-up audit trail, automated CC
                // sessions). The contract in `status_transitions()` reflects this
                // exception.
                if !text.is_empty() {
                    sqlx::query(
                        "UPDATE thread_summaries SET status = 'running', last_revived_at = NOW() WHERE thread_id = $1",
                    )
                    .bind(thread_id)
                    .execute(&mut **tx)
                    .await?;
                    // Engine-synthesized prompts (hardening, conflict recovery)
                    // can land on an already-idled CC child whose terminal
                    // event already decremented the parent. Same revive
                    // semantics as CodingAgentUserMessageSent above.
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
            ThreadEvent::ContinuationRequested { .. } => {
                // Continuation start event — bump last_activity and flip status
                // back to running so the thread surfaces in the recents list as
                // soon as the dispatcher emits the spawn. The contract's
                // status_transitions table sets Running too; we emit the SQL
                // here so the timestamp moves alongside it.
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     status = 'running', last_revived_at = NOW() WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;

                // Re-increment parent's `active_children_count`: the synthetic
                // `CodingAgentIdled{reason=engine_restart_interrupt}` that
                // recovery emits for the prior turn already decremented the
                // parent (a parked child is not active). When the user clicks
                // Continue the child is alive again, so the parent's count
                // must bounce back — otherwise its `ThreadStatusIcon` stays
                // 'idle' (no pulsing dot) while the child is actively running.
                // Gated on the pre-sample's `is_blocking`: a duplicate
                // ContinuationRequested (rapid double click) arrives with the child
                // already 'running' and must not double-increment.
                let was_blocking = prev_sample
                    .as_ref()
                    .map(|s| s.is_blocking())
                    .unwrap_or(false);
                if !was_blocking {
                    let parent_id: Option<Uuid> = sqlx::query_scalar(
                        "SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1",
                    )
                    .bind(thread_id)
                    .fetch_optional(&mut **tx)
                    .await?
                    .flatten();
                    if let Some(pid) = parent_id {
                        sqlx::query(
                            "UPDATE thread_summaries SET active_children_count = \
                             active_children_count + 1 WHERE thread_id = $1",
                        )
                        .bind(pid)
                        .execute(&mut **tx)
                        .await?;
                    }
                }
                Vec::new()
            }
            ThreadEvent::ToolCalled { .. }
            | ThreadEvent::ToolResult { .. }
            | ThreadEvent::TextStreamed { .. }
            | ThreadEvent::ThoughtStreamed { .. }
            | ThreadEvent::MemorySearched { .. }
            | ThreadEvent::CodingAgentTextStreamed { .. }
            | ThreadEvent::CodingAgentToolCalled { .. }
            | ThreadEvent::CodingAgentToolResult { .. } => {
                // Update last_activity so the thread list timestamp stays current during
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
                // `thread_lifecycle.rs`.
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
                    sqlx::query(
                        "UPDATE thread_summaries SET last_activity = NOW(), \
                         last_agent_action = NOW(), \
                         last_revived_at = CASE WHEN status != 'running' THEN NOW() \
                                                ELSE last_revived_at END, \
                         status = 'running' \
                         WHERE thread_id = $1",
                    )
                    .bind(thread_id)
                    .execute(&mut **tx)
                    .await?;
                }
                Vec::new()
            }
            // Both CC AskUserQuestion and CC permission prompts pause the
            // exchange on user input — surface in REVIEW. AskUserQuestion kills
            // the Claude Code subprocess; the permission prompt keeps it alive while
            // its MCP stdio server blocks on the engine's HTTP response. The
            // projection treats them identically: status flips to
            // 'waiting_for_user_answer' on the request and back to 'running'
            // on the resolution.
            ThreadEvent::UserQuestionAsked { .. }
            | ThreadEvent::CodingAgentPermissionRequest { .. }
            | ThreadEvent::CommandPermissionRequested { .. } => {
                // The agent asking for input is agent activity (bumps
                // last_agent_action); the user's resolution below is the user
                // action.
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_agent_action = NOW(), \
                     status = 'waiting_for_user_answer' WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::UserQuestionAnswered { .. }
            | ThreadEvent::CodingAgentPermissionResolved { .. }
            | ThreadEvent::CommandPermissionResolved { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), last_user_action = NOW(), \
                     status = 'running', last_revived_at = NOW() WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeReverted { change_id, .. } => {
                crate::core::changes_projection::ChangesProjection::write_status(
                    tx, change_id, "reverted",
                )
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeHardened { change_id, .. } => {
                crate::core::changes_projection::ChangesProjection::write_hardened(tx, change_id)
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
            // Child-completion fan-in landing on the parent. Mark the child's
            // `parent_callback_sent` HERE — in the same transaction as the
            // event INSERT — so the marker and the event cannot disagree. A
            // separate post-emit UPDATE (the old shape in
            // `notify_parent_if_child`) left a crash window where the typed
            // event committed but the marker didn't; the next terminal event
            // then re-fired the fan-in and handed the parent a duplicate
            // completion card. The parent's wake-up + agentic-loop replay
            // handle the actual side effect — no other projection state
            // changes for the parent row itself.
            ThreadEvent::ChildThreadCompleted {
                child_thread_id, ..
            } => {
                sqlx::query(
                    "UPDATE thread_summaries SET parent_callback_sent = TRUE \
                     WHERE thread_id = $1",
                )
                .bind(child_thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            // Events that don't affect thread_summaries metadata or status.
            // Exhaustive match — adding a new ThreadEvent variant forces you to decide
            // whether it needs a projection update. Never use `_ =>` here.
            ThreadEvent::CredentialRequested { .. }
            | ThreadEvent::McpConsentRequested { .. }
            // Transient events (never persisted, never reach this function)
            | ThreadEvent::CumulativeTextUpdated { .. }
            | ThreadEvent::LlmCallRetried { .. }
            | ThreadEvent::PreambleCompleted
            | ThreadEvent::CredentialPromptRequested { .. }
            | ThreadEvent::PluginInstallRequested { .. }
            | ThreadEvent::PluginUninstallRequested { .. }
            | ThreadEvent::EmailConfirmRequested { .. }
            | ThreadEvent::PushNotificationRequested
            | ThreadEvent::McpConsentPromptRequested { .. }
            | ThreadEvent::FileRefreshRequested { .. }
            | ThreadEvent::AppUiRefreshRequested { .. }
            | ThreadEvent::AppUiCaptureRequested { .. }
            | ThreadEvent::NavigationRequested { .. }
            | ThreadEvent::CodingAgentThreadSpawned { .. }
            | ThreadEvent::ChildrenCountChanged { .. }
            | ThreadEvent::MissingHardeningDetected { .. }
            | ThreadEvent::CodingAgentSettingsChanged { .. }
            // Passive bookkeeping for the background cleanup worker
            // (Phase 10.2). Persisted to the events stream for audit /
            // debugging but produces no projection side effects.
            | ThreadEvent::WorktreeCleaned { .. }
            // ImageUploaded is a per-thread audit fact for content-addressed
            // blob uploads. Persisted for audit + cross-device prefetch hint
            // via SSE; no projection side effects (no status change, no
            // section transition, no last_activity bump).
            | ThreadEvent::ImageUploaded { .. }
            | ThreadEvent::ContextCaptured { .. }
            // Agent-driven dismissal of a prior tool result / child completion
            // from future resume context. Pure resume-helper input; no
            // projection state change.
            | ThreadEvent::ContextDismissed { .. }
            // Background bash lifecycle events. The paired ToolCalled /
            // ToolResult for the spawn already bumped last_activity. The
            // completion event fires asynchronously from a tokio watcher,
            // possibly long after the LLM turn ended — bumping last_activity
            // there would surface a quiet thread as "active" with nothing
            // for the user to look at. Persisted for the audit trail and
            // the event-store fallback in `bash_output`.
            | ThreadEvent::BackgroundBashStarted { .. }
            | ThreadEvent::BackgroundBashCompleted { .. }
            // Engine-internal Flash enrichment of a prior MessageReceived's
            // attached images. Persisted for history projection / title
            // generation lookup by source_event_id; doesn't bump activity or
            // trigger a section transition (the MessageReceived already did
            // that one or more iterations earlier).
            | ThreadEvent::ImageDescribed { .. }
            // Todo list snapshot from a `todo_write` tool call. The paired
            // `ToolCalled` / `ToolResult` already bumped last_activity; this
            // event exists for the sticky panel projection, not for routing.
            | ThreadEvent::TodoListWritten { .. }
            // Command-guard checkpoint lifecycle (ADR 0002, Phase 4). The
            // checkpoint is taken mid-turn, right before the command runs — the
            // paired ToolCalled already bumped last_activity and the thread is
            // still running, so there's no status change or section transition.
            // The revert is a small one-click undo on a past card; persisted for
            // the reverted-state render and broadcast over SSE, but it doesn't
            // resurface the thread. Both are pure card/audit projection.
            | ThreadEvent::CommandCheckpointed { .. }
            | ThreadEvent::CommandCheckpointReverted { .. } => Vec::new(),
        };

        // Step 2: Validate and apply section transition via the lifecycle contract.
        // This runs after metadata updates so upsert events have created the row.
        let thread_type = Self::get_thread_type(tx, &thread_id).await;
        let current = Self::get_current_section(tx, &thread_id).await;
        let (depth, source, trigger_go_to_review): (i32, Option<String>, bool) = sqlx::query_as(
            "SELECT COALESCE(depth, 0), source, COALESCE(trigger_go_to_review, FALSE) \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&mut **tx)
        .await
        .unwrap_or(None)
        .unwrap_or((0, None, false));
        // Trigger executions run unattended — don't surface in REVIEW. But
        // user followups on trigger threads ARE attended (latest start =
        // user-driven MessageReceived), and triggers with `go_to_review=true`
        // opt back in for reports/alerts the user is meant to read.
        //
        // Engine- and agent-driven `MessageReceived` events MUST NOT count as
        // a user follow-up — they're automated. The mode filter defaults to
        // Human to mirror `default_mode_human` on `ThreadEvent::MessageReceived`,
        // so legacy rows persisted before the field existed still read as
        // user messages.
        let is_top_level = if depth > 0 {
            false
        } else if source.as_deref() != Some("trigger") || trigger_go_to_review {
            true
        } else {
            let human = ActorMode::Human.as_str();
            let latest_start: Option<String> = sqlx::query_scalar(
                "SELECT event_type FROM events WHERE aggregate_id = $1::text \
                 AND (event_type = 'TriggerStarted' \
                      OR (event_type = 'MessageReceived' \
                          AND COALESCE(payload->>'mode', $2) = $2)) \
                 ORDER BY sequence DESC LIMIT 1",
            )
            .bind(thread_id)
            .bind(human)
            .fetch_optional(&mut **tx)
            .await?;
            latest_start.as_deref() == Some("MessageReceived")
        };
        match resolve_transition(event.event_type(), thread_type, current, is_top_level) {
            Ok(mut transition) => {
                // CodingAgentIdled(has_changes=false) after apply/discard is a housekeeping
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
                Self::apply_transition(tx, thread_id, &transition).await?;
            }
            Err(v) => {
                crate::log!("[EventBus] {}", v);
                return Err(Box::new(v));
            }
        }

        // Sample is_blocking-relevant state AFTER both the metadata match and
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

        // Merge any extra ancestors collected by defense-in-depth reconciles
        // (Apply/Discard arms) so the SSE rebroadcast covers them too.
        if !extra_ancestors.is_empty() {
            affected_ancestors.extend(extra_ancestors);
            affected_ancestors.sort();
            affected_ancestors.dedup();
        }

        Ok((match_side_effects, affected_ancestors))
    }
}
