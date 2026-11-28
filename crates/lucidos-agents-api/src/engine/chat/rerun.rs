use crate::engine::chat::process::ChatProcessExt;
use std::sync::Arc;

use uuid::Uuid;

use super::PreEmittedOrigin;
use crate::engine::event_bus::EventBus;
use crate::engine::resume_anchor::{
    emit_resume_anchor, ChatResumeAnchor, ChatResumeFuture, ContinueChatOutcome, ResumeAnchorClaim,
};
use crate::engine::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin,
};
use crate::engine::LucidosEngine;
use crate::scheduler::user_tasks::{
    answered_park, in_trigger_run, run_awaits_owner, trigger_config_of_thread, AnsweredPark,
    TriggerRunGrants,
};

const MAX_BULLET_ENTRIES: usize = 50;
const MAX_FIELD_BYTES: usize = 200;

const ALLOWED_RUN_NOTE: &str = "[Engine note] The owner allowed what the blocked command \
    needed. Run it again and carry on.";

async fn abort_resumed_by_unclaimed_boundary(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    abort_event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
            SELECT 1 FROM events \
            WHERE aggregate_id = $1 \
              AND event_type = 'ContinuationStarted' \
              AND payload->>'resumed_abort_id' IS NULL \
              AND sequence > (SELECT sequence FROM events WHERE id = $2) \
        )",
    )
    .bind(thread_id.to_string())
    .bind(abort_event_id)
    .fetch_one(pool)
    .await
}

#[easy_ext::ext(RerunExt)]
pub(crate) impl LucidosEngine {
    async fn continue_chat(
        self: &Arc<Self>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
        grants: TriggerRunGrants,
    ) -> Result<ContinueChatOutcome, Box<dyn std::error::Error + Send + Sync>> {
        if !matches!(actor, Some(MessageOrigin::Device { .. }))
            && !crate::core::store::thread_hold::may_start_turn(self.pool(), thread_id).await
        {
            return Ok(ContinueChatOutcome::NothingToContinue);
        }
        if let Some(AnsweredPark {
            turn,
            granted: true,
        }) = answered_park(self.pool(), thread_id).await?
        {
            return Ok(self.resume_allowed_run(thread_id, turn, grants).await);
        }
        let abort_row: Option<(Uuid, Option<Uuid>, Option<String>)> = sqlx::query_as(
            "SELECT id, \
                    NULLIF(payload->>'request_event_id','')::uuid AS request_event_id, \
                    payload->>'channel' AS channel \
             FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseAborted' \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id.to_string())
        .fetch_optional(self.pool())
        .await?;

        let (abort_event_id, abort_request_event_id, abort_channel) = match abort_row {
            Some(row) => row,
            None => {
                log_legacy!(
                    "[Continue] No ResponseAborted for thread {} — nothing to continue",
                    thread_id
                );
                return Ok(ContinueChatOutcome::NothingToContinue);
            }
        };

        if abort_resumed_by_unclaimed_boundary(self.pool(), thread_id, abort_event_id).await? {
            log_legacy!(
                "[Continue] Abort {} on thread {} was resumed by an unclaimed boundary: no-op",
                abort_event_id,
                thread_id
            );
            return Ok(ContinueChatOutcome::AlreadyResumed);
        }

        let originating_event_id = match abort_request_event_id {
            Some(id) => Some(id),
            None => {
                crate::engine::session_queries::latest_originating_event_id(
                    self.pool(),
                    thread_id,
                    crate::engine::thread_events::CHAT_ORIGINATING_EVENT_TYPES,
                )
                .await
            }
        };

        let originating_event_id = match originating_event_id {
            Some(id) => id,
            None => {
                log_legacy!(
                    "[Continue] No originating event found for abort {} in thread {} — nothing to continue",
                    abort_event_id,
                    thread_id
                );
                return Ok(ContinueChatOutcome::NothingToContinue);
            }
        };

        let between_events =
            events_between(self.pool(), thread_id, originating_event_id, abort_event_id).await?;

        let summary = build_side_effect_summary(&between_events);
        let engine_note = build_engine_note(&summary);

        let thread_source: Option<String> = sqlx::query_scalar::<_, String>(
            "SELECT source FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool())
        .await?;
        let resume_channel =
            resolve_resume_channel(abort_channel.as_deref(), thread_source.as_deref());

        let claim = self
            .spawn_chat_resume(
                thread_id,
                engine_note,
                resume_channel,
                actor,
                ChatResumeAnchor::NewBoundary {
                    interrupted_turn: Some(originating_event_id),
                    resumed_abort: Some(abort_event_id),
                },
            )
            .await?;

        Ok(match claim {
            ResumeAnchorClaim::Anchored(_) => ContinueChatOutcome::Resumed,
            ResumeAnchorClaim::AlreadyResumed => {
                log_legacy!(
                    "[Continue] Abort {} on thread {} was already resumed: no-op",
                    abort_event_id,
                    thread_id
                );
                ContinueChatOutcome::AlreadyResumed
            }
        })
    }

    async fn resume_allowed_run(
        self: &Arc<Self>,
        thread_id: Uuid,
        turn: Uuid,
        grants: TriggerRunGrants,
    ) -> ContinueChatOutcome {
        let Some(config) = trigger_config_of_thread(self, thread_id).await else {
            log_legacy!("[Continue] Thread {thread_id} has no trigger to resume: nothing to do");
            return ContinueChatOutcome::NothingToContinue;
        };
        let engine = self.clone_arc();
        tokio::spawn(async move {
            let run = async {
                let resumed = engine
                    .resume_trigger_turn(&config, thread_id, ALLOWED_RUN_NOTE, turn)
                    .await;
                if let Err(e) = resumed.as_ref() {
                    if run_awaits_owner(e.as_ref()) {
                        return;
                    }
                    log_legacy!("[Continue] Allowed run on thread {thread_id} failed: {e}");
                }
                let status = crate::triggers::TriggerRunStatus::from_success(resumed.is_ok());
                engine.record_trigger_executed(&config.id, status).await;
            };
            in_trigger_run(&config, grants, run).await;
        });
        ContinueChatOutcome::Resumed
    }

    fn spawn_chat_resume(
        self: &Arc<Self>,
        thread_id: Uuid,
        engine_note: String,
        channel: EventChannel,
        actor: Option<MessageOrigin>,
        anchor: ChatResumeAnchor,
    ) -> ChatResumeFuture {
        let engine = self.clone_arc();
        Box::pin(async move {
            // INVARIANT: Sampled BEFORE the anchor emit, which is this reader's fence: a
            // message landing after it finds no live handle and gets a turn of
            // its own from the chat API. See `queued_recovery`.
            let window_end = super::queued_recovery::window_end_sequence(engine.pool(), thread_id)
                .await
                .unwrap_or_else(|e| {
                    log_legacy!(
                        "[Continue] window-end sample failed for thread {}: {}. \
                         Any queued follow-up keeps its remove button",
                        thread_id,
                        e
                    );
                    0
                });

            let anchor_event_id = match emit_resume_anchor(
                &engine.event_bus,
                thread_id,
                anchor,
                &engine_note,
                channel,
                actor,
            )
            .await?
            {
                ResumeAnchorClaim::Anchored(id) => id,
                ResumeAnchorClaim::AlreadyResumed => return Ok(ResumeAnchorClaim::AlreadyResumed),
            };

            let recovered = ingest_queued_messages_for_resume(
                ResumeStores {
                    bus: &engine.event_bus,
                    pool: engine.pool(),
                    workspace: engine.workspace_path(),
                },
                thread_id,
                anchor,
                anchor_event_id,
                channel,
                window_end,
            )
            .await;

            let loop_engine = engine.clone();
            let prompt_for_llm =
                prompt_with_recovered_messages(engine_note, recovered.as_ref().map(|r| &*r.text));
            let recovered_images = recovered.and_then(|r| r.images);
            tokio::spawn(async move {
                if let Err(e) = loop_engine
                    .process_message_with_steps(
                        &prompt_for_llm,
                        None,
                        None,
                        None,
                        None,
                        None,
                        recovered_images.as_deref(),
                        None,
                        None,
                        None,
                        Some(thread_id),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        ActorMode::Engine,
                        None,
                        None,
                        Some(PreEmittedOrigin::EngineReentry(anchor_event_id)),
                        None,
                        Some(MessageOrigin::engine(EngineReason::ContinuationStarted)),
                        None,
                        crate::engine::FollowUpUrgency::Normal,
                        None,
                        None,
                    )
                    .await
                {
                    log_legacy!("[Continue] Rerun for thread {} failed: {}", thread_id, e);
                }
            });

            Ok(ResumeAnchorClaim::Anchored(anchor_event_id))
        })
    }
}

pub(crate) struct ResumeStores<'a> {
    pub(crate) bus: &'a EventBus,
    pub(crate) pool: &'a sqlx::PgPool,
    pub(crate) workspace: &'a std::path::Path,
}

pub(crate) struct RecoveredMessages {
    pub(crate) text: String,
    pub(crate) images: Option<Vec<crate::engine::types::ChatImage>>,
}

/// INVARIANT: Announce every queued follow-up this resume still owes an answer for, and
/// return what the resumed turn should carry.
///
/// The announcement is a `PromptInjected` per message, awaited before the
/// loop is spawned. That write is what a second resume reads, so a later
/// resume, or one racing the boot sweep, finds the marker and recovers
/// nothing. A double Continue of one abort never gets here: the anchor's
/// claim refuses it first.
///
/// Anchored on the RESUME, not on the first recovered message, because the
/// turn's own `PreEmittedOrigin` is the resume anchor. The live orphan path
/// (`api::chat::announce_orphan_batch`) anchors on its first message for the
/// same reason, with a different origin.
///
/// **The retraction check is per message, immediately before its own emit.**
/// Filtering the batch once leaves a window as wide as the whole announce
/// loop, and the trash icon is live throughout it. Only what was actually
/// announced reaches the prompt, so the two can never disagree.
pub(crate) async fn ingest_queued_messages_for_resume(
    stores: ResumeStores<'_>,
    thread_id: Uuid,
    anchor: ChatResumeAnchor,
    anchor_event_id: Uuid,
    channel: EventChannel,
    window_end: i64,
) -> Option<RecoveredMessages> {
    let ResumeStores {
        bus,
        pool,
        workspace,
    } = stores;
    let interrupted_turn = anchor.interrupted_turn()?;
    let prompts = match super::queued_recovery::undrained_user_messages(
        pool,
        workspace,
        thread_id,
        interrupted_turn,
        window_end,
    )
    .await
    {
        Ok(prompts) => prompts,
        Err(e) => {
            log_legacy!(
                "[Continue] queued-message read failed for thread {}: {}. \
                 Any queued follow-up keeps its remove button",
                thread_id,
                e
            );
            return None;
        }
    };

    let base_meta = EventMeta {
        request_event_id: Some(anchor_event_id),
        channel: Some(channel),
        ..EventMeta::NONE
    };
    let mut announced: Vec<crate::engine::InjectedPrompt> = Vec::new();
    for prompt in prompts {
        let still_owed = crate::engine::agentic_loop::filter_removed_queued_prompts(
            pool,
            thread_id,
            vec![prompt],
        )
        .await;
        let Some(prompt) = still_owed.into_iter().next() else {
            continue;
        };
        if crate::engine::agentic_loop::emit_prompt_injected_event(
            bus, thread_id, &base_meta, &prompt,
        )
        .await
        {
            announced.push(prompt);
        }
    }
    if announced.is_empty() {
        return None;
    }
    log_legacy!(
        "[Continue] Recovered {} queued follow-up(s) stranded on thread {}",
        announced.len(),
        thread_id
    );

    Some(RecoveredMessages {
        text: crate::engine::agentic_loop::coalesced_user_text_for_reprocess(&announced),
        images: crate::engine::agentic_loop::coalesced_images_for_reprocess(&announced),
    })
}

fn prompt_with_recovered_messages(engine_note: String, recovered: Option<&str>) -> String {
    match recovered {
        Some(text) => format!("{}\n\n---\n\n{}", engine_note, text),
        None => engine_note,
    }
}

fn resolve_resume_channel(
    abort_channel: Option<&str>,
    thread_source: Option<&str>,
) -> EventChannel {
    abort_channel
        .and_then(EventChannel::from_wire)
        .or_else(|| thread_source.and_then(EventChannel::from_wire))
        .unwrap_or(EventChannel::Chat)
}

async fn events_between(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    from_event_id: Uuid,
    to_event_id: Uuid,
) -> Result<Vec<BetweenEvent>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, event_type, payload FROM events \
         WHERE aggregate_id = $1 \
           AND sequence > (SELECT sequence FROM events WHERE id = $2) \
           AND sequence < (SELECT sequence FROM events WHERE id = $3) \
         ORDER BY sequence ASC",
    )
    .bind(thread_id.to_string())
    .bind(from_event_id)
    .bind(to_event_id)
    .fetch_all(pool)
    .await
}

type BetweenEvent = (Uuid, String, serde_json::Value);

fn build_side_effect_summary(events: &[BetweenEvent]) -> String {
    let mut summary_lines: Vec<String> = Vec::new();
    let mut pending_calls: Vec<(Uuid, String, String)> = Vec::new();

    for (event_id, event_type, payload) in events {
        match event_type.as_str() {
            "ToolCalled" => {
                let name = payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)")
                    .to_string();
                let args_str = payload
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        payload
                            .get("args")
                            .map(|a| a.to_string())
                            .unwrap_or_default()
                    });
                pending_calls.push((*event_id, name, truncate_for_note(&args_str)));
            }
            "ToolResult" => {
                let result_text = payload
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no result)")
                    .to_string();
                let result_summary = truncate_for_note(&result_text);
                let answered = match crate::core::store::tool_called_event_id_of(payload) {
                    Some(call_id) => pending_calls
                        .iter()
                        .position(|(id, _, _)| *id == call_id)
                        .map(|i| pending_calls.remove(i)),
                    None => pending_calls.pop(),
                };
                if let Some((_, name, args)) = answered {
                    summary_lines.push(format!("- {}({}) → {}", name, args, result_summary));
                    if summary_lines.len() >= MAX_BULLET_ENTRIES {
                        summary_lines.push(format!(
                            "- ...(truncated at {} entries)",
                            MAX_BULLET_ENTRIES
                        ));
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    if summary_lines.is_empty() {
        "No actions completed before the abort.".to_string()
    } else {
        summary_lines.join("\n")
    }
}

fn truncate_for_note(s: &str) -> String {
    let trimmed = s.trim().replace('\n', " ");
    if trimmed.len() > MAX_FIELD_BYTES {
        let cut = trimmed.floor_char_boundary(MAX_FIELD_BYTES);
        format!("{}...", &trimmed[..cut])
    } else {
        trimmed
    }
}

fn build_engine_note(summary: &str) -> String {
    format!(
        "[Engine note — this is a rerun]\n\
         Your previous attempt at this turn was interrupted by an engine restart.\n\
         The interrupted run performed the following actions before the abort:\n\
         {}\n\
         Decide whether to:\n\
         - Skip actions that already completed successfully (don't re-run \"send_notification\" if the user has already been pinged).\n\
         - Re-verify state where it might have changed since the abort (file edits, git state).\n\
         - Continue with anything that didn't get to run.\n\
         Engine performed no automatic skipping — your judgment owns this.",
        summary
    )
}

#[cfg(test)]
#[path = "rerun_queued_tests.rs"]
mod queued_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resume_channel_prefers_the_abort_then_the_thread_source() {
        assert_eq!(
            resolve_resume_channel(Some("claude_code"), Some("chat")),
            EventChannel::ClaudeCode
        );

        assert_eq!(
            resolve_resume_channel(None, Some("trigger")),
            EventChannel::Trigger
        );
        assert_eq!(
            resolve_resume_channel(None, Some("chat")),
            EventChannel::Chat
        );
        assert_eq!(
            resolve_resume_channel(None, Some("scheduled_trigger")),
            EventChannel::Trigger
        );

        assert_eq!(resolve_resume_channel(None, None), EventChannel::Chat);
        assert_eq!(
            resolve_resume_channel(Some("from_the_future"), Some("also_unknown")),
            EventChannel::Chat
        );
    }

    #[test]
    fn summary_pairs_tool_call_with_result() {
        let events = vec![
            (
                Uuid::nil(),
                "ToolCalled".to_string(),
                json!({"name": "send_notification", "description": "Notify: Ping"}),
            ),
            (
                Uuid::nil(),
                "ToolResult".to_string(),
                json!({"name": "send_notification", "result": "ok"}),
            ),
        ];
        let s = build_side_effect_summary(&events);
        assert!(
            s.contains("- send_notification(Notify: Ping) → ok"),
            "got {}",
            s
        );
    }

    #[test]
    fn summary_pairs_results_by_call_id_in_any_order() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let call = |id, path: &str| {
            (
                id,
                "ToolCalled".to_string(),
                json!({"name": "read_file", "description": format!("Read {path}")}),
            )
        };
        let result = |id: Uuid, text: &str| {
            (
                Uuid::new_v4(),
                "ToolResult".to_string(),
                json!({"name": "read_file", "result": text, "tool_called_event_id": id}),
            )
        };
        let events = vec![
            call(a, "a.txt"),
            call(b, "b.txt"),
            call(c, "c.txt"),
            result(a, "contents of a"),
            result(c, "contents of c"),
            result(b, "contents of b"),
        ];
        let s = build_side_effect_summary(&events);
        for name in ["a", "b", "c"] {
            let line = format!("- read_file(Read {name}.txt) → contents of {name}");
            assert!(s.contains(&line), "missing {line:?} in {s}");
        }
    }

    #[test]
    fn summary_handles_zero_pairs() {
        let events: Vec<BetweenEvent> = vec![(
            Uuid::nil(),
            "Thinking".to_string(),
            json!({"text": "musing"}),
        )];
        let s = build_side_effect_summary(&events);
        assert_eq!(s, "No actions completed before the abort.");
    }

    #[test]
    fn summary_skips_thinking() {
        let events = vec![
            (
                Uuid::nil(),
                "Thinking".to_string(),
                json!({"text": "thinking..."}),
            ),
            (
                Uuid::nil(),
                "ToolCalled".to_string(),
                json!({"name": "read_file", "description": "Read foo.txt"}),
            ),
            (
                Uuid::nil(),
                "ToolResult".to_string(),
                json!({"name": "read_file", "result": "contents"}),
            ),
        ];
        let s = build_side_effect_summary(&events);
        assert!(
            !s.contains("thinking"),
            "summary must not include thinking text: {}",
            s
        );
        assert!(s.contains("read_file"));
    }

    #[test]
    fn summary_truncates_long_args_and_results() {
        let big = "x".repeat(500);
        let events = vec![
            (
                Uuid::nil(),
                "ToolCalled".to_string(),
                json!({"name": "run_bash", "args": {"command": big.clone()}}),
            ),
            (
                Uuid::nil(),
                "ToolResult".to_string(),
                json!({"name": "run_bash", "result": big}),
            ),
        ];
        let s = build_side_effect_summary(&events);
        assert!(s.contains("..."), "expected ellipsis on truncation: {}", s);
        for line in s.lines() {
            for field in line.split("→") {
                assert!(
                    field.len() < MAX_FIELD_BYTES + 50,
                    "field too long: {}",
                    field
                );
            }
        }
    }

    #[test]
    fn summary_caps_at_max_entries() {
        let mut events = Vec::new();
        for i in 0..(MAX_BULLET_ENTRIES + 5) {
            events.push((
                Uuid::nil(),
                "ToolCalled".to_string(),
                json!({"name": "tool", "description": format!("call {}", i)}),
            ));
            events.push((
                Uuid::nil(),
                "ToolResult".to_string(),
                json!({"name": "tool", "result": format!("ok {}", i)}),
            ));
        }
        let s = build_side_effect_summary(&events);
        let line_count = s.lines().count();
        assert!(
            line_count <= MAX_BULLET_ENTRIES + 1,
            "expected ≤{} bullet lines, got {}",
            MAX_BULLET_ENTRIES + 1,
            line_count
        );
        assert!(
            s.contains("(truncated"),
            "expected truncation marker: {}",
            s
        );
    }

    #[test]
    fn engine_note_mentions_rerun_and_summary() {
        let note = build_engine_note("- send_notification(Hi) → ok");
        assert!(note.contains("[Engine note — this is a rerun]"));
        assert!(note.contains("send_notification(Hi)"));
        assert!(note.contains("Engine performed no automatic skipping"));
    }

    #[tokio::test]
    async fn continue_note_window_covers_the_interrupted_turn() {
        use crate::engine::event_bus::{BusEvent, EventBus};
        use crate::engine::thread_events::{ActorMode, ThreadEvent};
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
            let id = Uuid::new_v4();
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: EventMeta {
                    event_id: Some(id),
                    ..EventMeta::NONE
                },
            })
            .await
            .expect("event must persist");
            id
        }
        fn message(text: &str) -> ThreadEvent {
            ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: text.to_string(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            }
        }

        let turn = emit(&bus, thread_id, message("summarize the tickets")).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::ToolCalled {
                name: "send_notification".into(),
                args: json!({}),
                description: "Notify: Ping".into(),
            },
        )
        .await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::ToolResult {
                name: "send_notification".into(),
                result: "ok".into(),
                images: vec![],
                success: true,
                tool_called_event_id: None,
            },
        )
        .await;
        let queued = emit(&bus, thread_id, message("actually, only the open ones")).await;
        let abort = emit(
            &bus,
            thread_id,
            ThreadEvent::ResponseAborted {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
                cause: crate::engine::thread_events::AbortCause::EngineShutdown,
            },
        )
        .await;

        let anchored = build_side_effect_summary(
            &events_between(&pool, thread_id, turn, abort)
                .await
                .expect("window query"),
        );
        assert!(
            anchored.contains("send_notification"),
            "anchored on the turn, the note must name the tool call that ran: {}",
            anchored
        );

        let mis_anchored = build_side_effect_summary(
            &events_between(&pool, thread_id, queued, abort)
                .await
                .expect("window query"),
        );
        assert_eq!(
            mis_anchored, "No actions completed before the abort.",
            "the queued follow-up opens an empty window, which is exactly what \
             the mis-anchored abort used to hand the resumed agent"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
