//! Projection updaters for `EventBus`. Both methods take an open
//! `sqlx::Transaction` and run inside the same DB tx as the event insert,
//! so a projection failure rolls back the event itself — never persist an
//! event whose projection didn't apply.

use uuid::Uuid;

use super::{BusEvent, EventBus, SystemEvent, STATUS_FROM_CC_HAS_CHANGES};
use crate::core::store::LegacyInitiator;
use crate::engine::thread_events::{ActorMode, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_lifecycle::{resolve_transition, ArchiveState};

impl EventBus {
    /// Returns side-effect events to emit after the main transaction commits.
    ///
    /// Structure: Step 1 runs metadata updates (the match statement).
    /// Step 2 validates and applies section transitions via the lifecycle contract.
    /// This ensures upsert events create the row before the contract checks it.
    pub(super) async fn update_thread_projection(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: Uuid,
        event: &ThreadEvent,
        meta: &EventMeta,
    ) -> Result<Vec<BusEvent>, Box<dyn std::error::Error + Send + Sync>> {
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
                }

                Vec::new()
            }
            // CC session lifecycle — session start/continuation don't update last_activity
            // (the first real activity event will set it).
            ThreadEvent::SessionStarted { .. } | ThreadEvent::ContinuationStarted { .. } => {
                let source = meta.channel.as_ref().map(|c| c.as_str()).unwrap_or("claude_code");
                // Extract repo_id from SessionStarted; ContinuationStarted has no repo_id.
                let session_repo_id = match &event {
                    ThreadEvent::SessionStarted { repo_id, .. } => repo_id.as_deref(),
                    _ => None,
                };
                sqlx::query(
                    r#"INSERT INTO thread_summaries (thread_id, source, is_cc, created_at, last_activity, message_count, status, last_revived_at, cc_repo_id, state)
                       VALUES ($1, $2, TRUE, NOW(), NOW(), 0, 'running', NOW(), $3, 'active')
                       ON CONFLICT (thread_id) DO UPDATE
                       SET is_cc = TRUE, source = $2,
                           initiator = COALESCE(thread_summaries.initiator, 'unknown'),
                           -- Existing value wins: a thread's repo is locked at first SessionStarted.
                           -- The chat handler enforces that follow-ups can't pick a different repo,
                           -- but defend the projection so any drift (legacy data, replay) doesn't
                           -- silently flip the thread to a different repo's skill set.
                           cc_repo_id = COALESCE(thread_summaries.cc_repo_id, $3),
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
                // Normal completion — go idle (or waiting if CC has pending changes).
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), has_response = TRUE, \
                     status = {STATUS_FROM_CC_HAS_CHANGES} WHERE thread_id = $1"
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ResponseAborted { cause, .. } => {
                // Status mapping lives on `AbortCause::status_sql()` so the
                // cause-classification stays next to `is_transient()` on the
                // enum. Most aborts surface a red `failed` indicator;
                // stale-settle is engine cleanup of an already-gone process
                // and uses the cancel-style `idle`/`waiting` mapping.
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET last_activity = NOW(), has_response = TRUE, \
                     status = {} WHERE thread_id = $1",
                    cause.status_sql()
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::CodingAgentIdled { has_changes, requires_restart, is_external_repo, .. } => {
                // CC session idled — SET (not OR) cc_has_changes from payload.
                // CodingAgentIdled is the authoritative snapshot of the session's state.
                // After apply/discard, the session emits has_changes=false to clear the flag.
                // Set has_response = TRUE so the thread appears in get_recent_threads
                // (CC threads don't go through ResponseGenerated, so this is the CC equivalent).
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     has_response = TRUE, \
                     cc_has_changes = $2, \
                     cc_requires_restart = $3, \
                     cc_is_external_repo = $4, \
                     status = CASE WHEN $2 THEN 'waiting' ELSE 'idle' END \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .bind(has_changes)
                .bind(requires_restart)
                .bind(is_external_repo)
                .execute(&mut **tx)
                .await?;
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
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     cc_has_changes = FALSE, cc_requires_restart = FALSE, \
                     cc_is_external_repo = FALSE, cc_applying = FALSE, \
                     status = 'idle' \
                     WHERE thread_id = $1",
                )
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
                Vec::new()
            }
            ThreadEvent::ChangeDiscarded { change_id, .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     cc_has_changes = FALSE, cc_requires_restart = FALSE, \
                     cc_is_external_repo = FALSE, cc_applying = FALSE, \
                     status = 'idle' \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                crate::core::changes_projection::ChangesProjection::write_status(
                    tx, change_id, "discarded",
                )
                .await?;
                Vec::new()
            }

            // Message count increment + activity (CC user messages and mid-flight injections)
            ThreadEvent::CodingAgentUserMessageSent { .. }
            | ThreadEvent::UserPromptInjected { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), message_count = message_count + 1, status = 'running', last_revived_at = NOW() WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
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
                // Saved (is_saved=true wins over state='archived').
                sqlx::query(
                    "UPDATE thread_summaries SET status = 'idle', \
                     state = 'archived', \
                     is_saved = FALSE, \
                     cc_has_changes = FALSE, cc_requires_restart = FALSE, \
                     cc_is_external_repo = FALSE, cc_applying = FALSE, \
                     active_children_count = 0, total_children_count = 0 \
                     WHERE thread_id = $1",
                )
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
                // discard from active/archived, so this UPDATE is safe to run
                // without re-checking. Compose fields are wiped so a stale
                // SSE replay can't show ghost text.
                sqlx::query(
                    "UPDATE thread_summaries SET state = 'discarded', \
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
                // User canceled — go idle (or waiting if pending changes).
                // Set has_response so the thread appears in history (a canceled
                // response is still a response — the user should see the thread).
                sqlx::query(&format!(
                    "UPDATE thread_summaries SET has_response = TRUE, \
                     status = {STATUS_FROM_CC_HAS_CHANGES} WHERE thread_id = $1"
                ))
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ResponseFailed { .. } => {
                // Failed response — distinct from 'waiting' (which means CC has
                // changes to review) so the UI can render an error indicator.
                // Set has_response so the thread stays visible.
                sqlx::query(
                    "UPDATE thread_summaries SET has_response = TRUE, status = 'failed' WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
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
                         status = {STATUS_FROM_CC_HAS_CHANGES} WHERE thread_id = $1"
                    ))
                    .bind(thread_id)
                    .execute(&mut **tx)
                    .await?;
                }
                Vec::new()
            }
            ThreadEvent::TriggerCompleted { .. } => {
                // Trigger run done — go idle. Set has_response so the thread
                // appears in get_recent_threads (which filters has_response=TRUE).
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), has_response = TRUE, status = 'idle' WHERE thread_id = $1",
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
                // CodingAgentIdled already set status='waiting' if the session idled;
                // mid-session commits keep status='running'. Only the flag changes.
                sqlx::query(
                    "UPDATE thread_summaries SET cc_has_changes = TRUE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                use crate::core::changes_projection::ChangesProjection;
                if change_id.is_empty() && commit_sha.is_some() {
                    ChangesProjection::write_proposed_per_commit(
                        tx,
                        branch_name,
                        description.as_deref(),
                        *requires_restart,
                    )
                    .await?;
                } else if !change_id.is_empty() {
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
                }
                Vec::new()
            }
            ThreadEvent::MergeConflictDetected { .. } => {
                // Merge conflict — mark as applying.
                sqlx::query(
                    "UPDATE thread_summaries SET cc_applying = TRUE WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::ChangeApplyFailed { .. } => {
                // Apply failed — clear applying flag, stay waiting.
                sqlx::query(
                    "UPDATE thread_summaries SET cc_applying = FALSE WHERE thread_id = $1",
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
                }
                Vec::new()
            }
            ThreadEvent::ContinueSignal { .. } => {
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
                Vec::new()
            }
            ThreadEvent::ToolCalled { .. }
            | ThreadEvent::ToolResult { .. }
            | ThreadEvent::TextStreamed { .. }
            | ThreadEvent::Thinking { .. }
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
                // for one-shot transitions (`ContinueSignal`, etc.) refresh it every
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
            // the CC subprocess; the permission prompt keeps it alive while
            // its MCP stdio server blocks on the engine's HTTP response. The
            // projection treats them identically: status flips to
            // 'waiting_for_user_answer' on the request and back to 'running'
            // on the resolution.
            ThreadEvent::UserQuestionAsked { .. }
            | ThreadEvent::CodingAgentPermissionRequest { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
                     status = 'waiting_for_user_answer' WHERE thread_id = $1",
                )
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
                Vec::new()
            }
            ThreadEvent::UserQuestionAnswered { .. }
            | ThreadEvent::CodingAgentPermissionResolved { .. } => {
                sqlx::query(
                    "UPDATE thread_summaries SET last_activity = NOW(), \
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
            // Events that don't affect thread_summaries metadata or status.
            // Exhaustive match — adding a new ThreadEvent variant forces you to decide
            // whether it needs a projection update. Never use `_ =>` here.
            ThreadEvent::MemorySearched { .. }
            | ThreadEvent::CredentialRequested { .. }
            | ThreadEvent::McpConsentRequested { .. }
            // Transient events (never persisted, never reach this function)
            | ThreadEvent::TextStreaming { .. }
            | ThreadEvent::Retrying { .. }
            | ThreadEvent::PreambleCompleting
            | ThreadEvent::CredentialRequest { .. }
            | ThreadEvent::PluginInstallRequest { .. }
            | ThreadEvent::PluginUninstallRequest { .. }
            | ThreadEvent::EmailConfirmRequest { .. }
            | ThreadEvent::PushNotificationRequest
            | ThreadEvent::McpConsentRequest { .. }
            | ThreadEvent::RefreshFile { .. }
            | ThreadEvent::RefreshAppUI { .. }
            | ThreadEvent::CaptureAppUI { .. }
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
            // Child-completion fan-in landing on the parent. Persisted as
            // history; the parent's wake-up + agentic loop replay handle the
            // actual side effect — no projection update needed here.
            | ThreadEvent::ChildThreadCompleted { .. }
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
            | ThreadEvent::BackgroundBashCompleted { .. } => Vec::new(),
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
                // CC session completed.
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
        Ok(match_side_effects)
    }

    // ---- System projection ----

    pub(super) async fn update_system_projection(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event_id: Uuid,
        event: &SystemEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            SystemEvent::NotificationCreated {
                id,
                title,
                message,
                task_id,
                app_id,
                thread_id,
            } => {
                let notification_id = Uuid::parse_str(id).unwrap_or(event_id);
                let task_uuid = task_id.as_ref().and_then(|s| Uuid::parse_str(s).ok());
                let thread_uuid = thread_id.as_ref().and_then(|s| Uuid::parse_str(s).ok());
                sqlx::query(
                    "INSERT INTO notifications (id, task_id, app_id, thread_id, title, message, read, created_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, false, NOW())"
                )
                .bind(notification_id)
                .bind(task_uuid)
                .bind(app_id.as_deref())
                .bind(thread_uuid)
                .bind(title)
                .bind(message)
                .execute(&mut **tx)
                .await?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
