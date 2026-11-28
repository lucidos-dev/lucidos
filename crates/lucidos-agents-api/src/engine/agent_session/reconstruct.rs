use sqlx::PgPool;
use uuid::Uuid;

pub(crate) const RECONSTRUCT_MAX_CHARS: usize = 50_000;

const HEADER: &str = "[Reconstructed conversation summary — your prior session was lost]";

const FOOTER: &str = "[End of summary. Continue assisting the user.]";

const PER_EVENT_MAX: usize = 1500;

pub(crate) async fn reconstruct_summary(
    pool: &PgPool,
    thread_id: Uuid,
    max_chars: usize,
) -> String {
    let rows = match fetch_relevant_events(pool, thread_id).await {
        Ok(rows) => rows,
        Err(e) => {
            log_legacy!(
                "[Reconstruct] Failed to fetch events for thread {}: {}",
                thread_id,
                e
            );
            return String::new();
        }
    };

    if rows.is_empty() {
        return String::new();
    }

    let lines: Vec<String> = rows
        .iter()
        .filter_map(|(event_type, payload)| project_event(event_type, payload))
        .collect();

    if lines.is_empty() {
        return String::new();
    }

    assemble_with_budget(lines, max_chars)
}

async fn fetch_relevant_events(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Vec<(String, serde_json::Value)>, sqlx::Error> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, payload FROM events \
         WHERE aggregate_id = $1 \
           AND event_type IN ( \
                'MessageReceived', \
                'CodingAgentTextStreamed', \
                'CodingAgentToolCalled', \
                'CodingAgentToolResult', \
                'CodingAgentUserMessageSent', \
                'ResponseGenerated', \
                'ResponseFailed' \
           ) \
         ORDER BY sequence ASC",
    )
    .bind(thread_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

fn project_event(event_type: &str, payload: &serde_json::Value) -> Option<String> {
    let text_field = |key: &str| -> Option<String> {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| truncate(s, PER_EVENT_MAX))
    };

    match event_type {
        "MessageReceived" | "CodingAgentUserMessageSent" => {
            let text = text_field("text")?;
            Some(format!("User: {}", text))
        }
        "CodingAgentTextStreamed" => {
            let text = text_field("text")?;
            Some(format!("You (assistant): {}", text))
        }
        "ResponseGenerated" => match text_field("text") {
            Some(text) => Some(format!("You (assistant): {}", text)),
            None => Some("You (assistant): (no text response)".to_string()),
        },
        "ResponseFailed" => {
            let reason = text_field("error").unwrap_or_else(|| "no response".to_string());
            Some(format!("You (assistant): (failed: {})", reason))
        }
        "CodingAgentToolCalled" => {
            let name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("<unknown tool>");
            let desc = payload
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let line = match desc {
                Some(d) => format!("You called tool: {} — {}", name, truncate(d, PER_EVENT_MAX)),
                None => format!("You called tool: {}", name),
            };
            Some(line)
        }
        "CodingAgentToolResult" => {
            let result = text_field("result")?;
            Some(format!("Tool result: {}", result))
        }
        _ => None,
    }
}

fn assemble_with_budget(lines: Vec<String>, max_chars: usize) -> String {
    let header_len = HEADER.len() + 2;
    let footer_len = FOOTER.len() + 2;
    let elision_template = "… [{} earlier turn(s) elided]\n";
    let elision_reserve = elision_template.len() + 16;

    if max_chars <= header_len + footer_len + elision_reserve {
        return format!("{}\n\n{}", HEADER, FOOTER);
    }

    let body_budget = max_chars - header_len - footer_len;

    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    let mut running = 0usize;
    let mut elided = 0usize;
    for line in lines.iter().rev() {
        let cost = line.len() + 1;
        if running + cost + elision_reserve > body_budget {
            elided = lines.len() - kept.len();
            break;
        }
        kept.push(line.as_str());
        running += cost;
    }
    kept.reverse();

    let mut out = String::with_capacity(max_chars);
    out.push_str(HEADER);
    out.push_str("\n\n");
    if elided > 0 {
        out.push_str(&format!("… [{} earlier turn(s) elided]\n", elided));
    }
    for line in kept {
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(FOOTER);
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

pub(crate) async fn prepend_reconstruction(
    pool: &PgPool,
    thread_id: Uuid,
    message: &str,
) -> String {
    let reconstruction = reconstruct_summary(pool, thread_id, RECONSTRUCT_MAX_CHARS).await;
    if reconstruction.is_empty() {
        message.to_string()
    } else {
        format!("{}\n\n{}", reconstruction, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use serde_json::json;

    fn cc_meta() -> EventMeta {
        EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        }
    }

    async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: cc_meta(),
        })
        .await
        .unwrap();
    }

    fn user_msg(text: &str) -> ThreadEvent {
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: text.into(),
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

    fn cc_text(text: &str) -> ThreadEvent {
        ThreadEvent::CodingAgentTextStreamed {
            text: text.into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            parent_tool_use_id: None,
            progress_note: false,
        }
    }

    fn cc_tool_called(name: &str, description: &str) -> ThreadEvent {
        ThreadEvent::CodingAgentToolCalled {
            name: name.into(),
            args: json!({}),
            description: description.into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            tool_use_id: String::new(),
            parent_tool_use_id: None,
            api_call_id: None,
        }
    }

    fn cc_tool_result(result: &str) -> ThreadEvent {
        ThreadEvent::CodingAgentToolResult {
            name: String::new(),
            result: result.into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            tool_use_id: String::new(),
            parent_tool_use_id: None,
        }
    }

    #[tokio::test]
    async fn empty_thread_yields_empty_summary() {
        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        let summary = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(summary.is_empty());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn projects_a_simple_two_turn_conversation() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        emit(&bus, thread_id, user_msg("add a feature flag for X")).await;
        emit(
            &bus,
            thread_id,
            cc_text("Sure, let me look at the flag system."),
        )
        .await;
        emit(&bus, thread_id, cc_tool_called("Read", "Read src/flags.rs")).await;
        emit(
            &bus,
            thread_id,
            cc_tool_result("pub fn enabled(name) -> bool"),
        )
        .await;
        emit(&bus, thread_id, user_msg("now write the migration")).await;

        let summary = reconstruct_summary(&pool, thread_id, 50_000).await;

        assert!(summary.contains(HEADER));
        assert!(summary.contains(FOOTER));
        assert!(summary.contains("User: add a feature flag for X"));
        assert!(summary.contains("You (assistant): Sure, let me look at the flag system."));
        assert!(summary.contains("You called tool: Read — Read src/flags.rs"));
        assert!(summary.contains("Tool result: pub fn enabled(name) -> bool"));
        assert!(summary.contains("User: now write the migration"));

        let positions: Vec<usize> = [
            "User: add a feature flag for X",
            "You (assistant): Sure",
            "You called tool: Read",
            "Tool result: pub fn enabled",
            "User: now write the migration",
        ]
        .iter()
        .map(|needle| summary.find(needle).expect(needle))
        .collect();
        for w in positions.windows(2) {
            assert!(w[0] < w[1], "events must appear in chronological order");
        }

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn skips_irrelevant_events() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        emit(&bus, thread_id, user_msg("hi")).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionStarted {
                sandbox: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: "sess-1".into(),
                branch: "claude-code/foo".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
        )
        .await;
        emit(&bus, thread_id, cc_text("hello back")).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("sess-1".into()),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
                turn_end: None,
            },
        )
        .await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::ThreadTitleGenerated {
                title: "Greeting".into(),
            },
        )
        .await;

        let summary = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(summary.contains("User: hi"));
        assert!(summary.contains("You (assistant): hello back"));
        assert!(!summary.contains("Greeting"));
        assert!(!summary.contains("SessionStarted"));
        assert!(!summary.contains("CodingAgentIdled"));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn truncates_per_event_to_per_event_max() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionStarted {
                sandbox: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: "sess-1".into(),
                branch: "claude-code/trunc".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
        )
        .await;
        let huge = "X".repeat(PER_EVENT_MAX * 4);
        emit(&bus, thread_id, cc_tool_result(&huge)).await;

        let summary = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(summary.contains("Tool result:"));
        assert!(summary.contains('…'));
        assert!(summary.len() < PER_EVENT_MAX * 2);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn budget_drops_oldest_lines_first() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        for i in 0..30 {
            emit(&bus, thread_id, user_msg(&format!("ask number {}", i))).await;
            emit(&bus, thread_id, cc_text(&format!("answer number {}", i))).await;
        }

        let summary = reconstruct_summary(&pool, thread_id, 1_000).await;
        assert!(summary.contains(HEADER));
        assert!(summary.contains(FOOTER));
        assert!(
            summary.contains("ask number 29"),
            "most recent user turn missing; summary was:\n{}",
            summary
        );
        assert!(
            summary.contains("answer number 29"),
            "most recent assistant turn missing; summary was:\n{}",
            summary
        );
        assert!(
            !summary.contains("ask number 0\n"),
            "oldest user turn should have been elided"
        );
        assert!(
            summary.contains("earlier turn(s) elided"),
            "elision notice missing"
        );
        assert!(
            summary.len() <= 1_000,
            "summary len {} exceeds 1000 budget",
            summary.len()
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn stale_resume_retry_input_combines_reconstruction_then_user_message() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        emit(&bus, thread_id, user_msg("set up the project")).await;
        emit(&bus, thread_id, cc_text("Done — created scaffolding.")).await;

        let actual_user_message = "now add a CHANGELOG file";

        let reconstruction = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(
            !reconstruction.is_empty(),
            "thread had prior turns — reconstruction must be non-empty"
        );
        let retry_text = format!("{}\n\n{}", reconstruction, actual_user_message);

        let recon_pos = retry_text.find(HEADER).expect("header present");
        let user_pos = retry_text
            .find(actual_user_message)
            .expect("actual user message present");
        assert!(
            recon_pos < user_pos,
            "reconstruction must precede the user's actual message"
        );
        assert!(
            retry_text.contains("User: set up the project"),
            "prior user turn missing from retry input"
        );
        assert!(
            retry_text.contains("You (assistant): Done — created scaffolding."),
            "prior assistant turn missing from retry input"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn stale_resume_retry_with_no_prior_events_uses_raw_user_message() {
        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();

        let reconstruction = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(
            reconstruction.is_empty(),
            "thread with no events must yield empty reconstruction"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn skips_empty_text_events() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        emit(&bus, thread_id, user_msg("hi")).await;
        emit(&bus, thread_id, cc_text("   ")).await;
        emit(&bus, thread_id, cc_text("real reply")).await;

        let summary = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(summary.contains("You (assistant): real reply"));
        assert!(
            !summary.contains("You (assistant): \n"),
            "empty assistant deltas leaked into summary"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
