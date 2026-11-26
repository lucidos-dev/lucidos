use crate::engine::LucidosEngine;
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[easy_ext::ext(TurnClockExt)]
pub(crate) impl LucidosEngine {
    async fn turn_started_at(&self, thread_id: Uuid) -> DateTime<Utc> {
        match self
            .event_store
            .thread_latest_event_created(thread_id)
            .await
        {
            Ok(Some(created)) => created,
            Ok(None) => {
                crate::log_legacy!(
                    "[Chat] thread {} has no events to read a clock from; using the wall clock for CURRENT TIME",
                    thread_id
                );
                Utc::now()
            }
            Err(e) => {
                crate::log_legacy!(
                    "[Chat] turn clock read failed ({}); using the wall clock for CURRENT TIME",
                    e
                );
                Utc::now()
            }
        }
    }
}

pub(super) fn timezone_section(user_timezone: &str) -> String {
    if user_timezone.is_empty() {
        return "USER TIMEZONE: not set yet, so times are given in UTC.\n\
                \"Now\" is the [CURRENT TIME] block at the END of the request."
            .to_string();
    }
    format!(
        "USER TIMEZONE: {tz}\n\
         \n\
         TIMEZONE HANDLING:\n\
         - \"Now\" is the [CURRENT TIME] block at the END of the request, in local and UTC.\n\
         - The user speaks in their LOCAL time ({tz}).\n\
         - All timestamps are stored as UTC in the database.\n\
         - ALWAYS display times to the user in their local timezone (not UTC).\n\
         - Cron uses 6 fields: second minute hour day-of-month month day-of-week\n\
         - When user says \"at 8am\", use \"0 0 8 * * *\" (second=0, minute=0, hour=8).\n\
         - Example: \"daily at 8am\" -> cron \"0 0 8 * * *\", \"at 9:30\" -> \"0 30 9 * * *\"\n\
         - The system automatically handles daylight saving time adjustments.",
        tz = user_timezone
    )
}

pub(super) fn current_time_block(at: DateTime<Utc>, user_timezone: &str) -> String {
    let utc_reading = format!("{} at {}", date_of(at), at.format("%H:%M"));

    if user_timezone.is_empty() {
        return format!("[CURRENT TIME]\nNow: {utc_reading} UTC.\n[END CURRENT TIME]");
    }

    let tz: chrono_tz::Tz = user_timezone.parse().unwrap_or(chrono_tz::UTC);
    let local = at.with_timezone(&tz);
    format!(
        "[CURRENT TIME]\n\
         Now: {local_date} at {local_time} {tz} (UTC{offset}).\n\
         The same instant in UTC: {utc_reading}.\n\
         [END CURRENT TIME]",
        local_date = date_of(local),
        local_time = local.format("%H:%M"),
        tz = user_timezone,
        offset = local.format("%:z"),
    )
}

fn date_of<Tz: chrono::TimeZone>(at: DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    at.format("%A, %B %d, %Y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[tokio::test]
    async fn the_turn_started_at_is_the_newest_event_on_the_thread() {
        use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
        let (pool, db) = setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = LucidosEngine::new(
            workspace.path().to_path_buf(),
            &test_db_url(&db),
            crate::llm::ProviderBuildContext {
                default_model: "mock".into(),
                model_is_mock: true,
                vertex_project_id: String::new(),
                vertex_location: crate::llm::vertex::location_handle(String::new()),
                vertex_token_cache: None,
                model_registry: Default::default(),
                not_served: Default::default(),
                claude_gateway_access: None,
                chatgpt_plan_access: None,
            },
            std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
        )
        .await
        .unwrap();

        let thread_id = Uuid::new_v4();
        let newest = utc(2026, 1, 16, 9, 30);
        for created in [newest, utc(2026, 1, 14, 13, 2)] {
            sqlx::query(
                "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
                 VALUES ($1, 'thread', $2, 'MessageReceived', $3, $4, $2::uuid)",
            )
            .bind(Uuid::new_v4())
            .bind(thread_id.to_string())
            .bind(serde_json::json!({}))
            .bind(created)
            .execute(&pool)
            .await
            .unwrap();
        }

        assert_eq!(engine.turn_started_at(thread_id).await, newest);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[test]
    fn the_system_half_ignores_the_clock_and_the_message_half_does_not() {
        let winter = utc(2026, 1, 14, 13, 2);
        let summer = utc(2026, 8, 17, 13, 2);

        assert_eq!(
            timezone_section("Europe/Oslo"),
            timezone_section("Europe/Oslo"),
            "the system half must not vary at all"
        );
        assert_ne!(
            current_time_block(winter, "Europe/Oslo"),
            current_time_block(summer, "Europe/Oslo"),
            "the message half carries the reading, so it must move"
        );

        let section = timezone_section("Europe/Oslo");
        for leaked in [
            "CURRENT TIME:",
            "13:02",
            "14:02",
            "15:02",
            "UTC+01:00",
            "UTC+02:00",
        ] {
            assert!(
                !section.contains(leaked),
                "the cached system half leaks {leaked}:\n{section}"
            );
        }
    }

    #[test]
    fn the_offset_rides_with_the_reading_across_a_dst_transition() {
        let winter = current_time_block(utc(2026, 1, 14, 13, 2), "Europe/Oslo");
        let summer = current_time_block(utc(2026, 8, 17, 13, 2), "Europe/Oslo");

        assert!(winter.contains("14:02 Europe/Oslo (UTC+01:00)"), "{winter}");
        assert!(summer.contains("15:02 Europe/Oslo (UTC+02:00)"), "{summer}");
    }

    #[test]
    fn the_block_carries_local_and_utc_unambiguously() {
        let block = current_time_block(utc(2026, 8, 17, 13, 2), "Europe/Oslo");

        assert!(block.starts_with("[CURRENT TIME]"), "{block}");
        assert!(block.ends_with("[END CURRENT TIME]"), "{block}");
        assert!(
            block.contains("Now: Monday, August 17, 2026 at 15:02 Europe/Oslo (UTC+02:00)."),
            "{block}"
        );
        assert!(
            block.contains("The same instant in UTC: Monday, August 17, 2026 at 13:02."),
            "{block}"
        );
    }

    #[test]
    fn the_local_date_is_the_local_date_past_midnight() {
        let block = current_time_block(utc(2026, 8, 17, 23, 30), "Europe/Oslo");

        assert!(
            block.contains("Now: Tuesday, August 18, 2026 at 01:30 Europe/Oslo (UTC+02:00)."),
            "{block}"
        );
        assert!(
            block.contains("The same instant in UTC: Monday, August 17, 2026 at 23:30."),
            "{block}"
        );
    }

    #[test]
    fn an_unset_timezone_still_gets_a_clock_and_a_pointer_to_it() {
        let block = current_time_block(utc(2026, 8, 17, 13, 2), "");
        assert!(
            block.contains("Now: Monday, August 17, 2026 at 13:02 UTC."),
            "{block}"
        );

        let section = timezone_section("");
        assert!(section.contains("[CURRENT TIME]"), "{section}");
        assert!(section.contains("not set yet"), "{section}");
    }

    #[test]
    fn an_unparseable_timezone_falls_back_to_utc_rather_than_dropping_the_time() {
        let block = current_time_block(utc(2026, 8, 17, 13, 2), "Not/AZone");
        assert!(block.contains("at 13:02 Not/AZone (UTC+00:00)."), "{block}");
    }

    #[test]
    fn a_western_timezone_renders_a_signed_offset() {
        let block = current_time_block(utc(2026, 8, 17, 13, 2), "America/New_York");
        assert!(
            block.contains("at 09:02 America/New_York (UTC-04:00)."),
            "{block}"
        );
    }

    #[test]
    fn a_sub_hour_offset_keeps_its_minutes() {
        let india = current_time_block(utc(2026, 8, 17, 13, 2), "Asia/Kolkata");
        assert!(
            india.contains("at 18:32 Asia/Kolkata (UTC+05:30)."),
            "{india}"
        );

        let nepal = current_time_block(utc(2026, 8, 17, 13, 2), "Asia/Kathmandu");
        assert!(
            nepal.contains("at 18:47 Asia/Kathmandu (UTC+05:45)."),
            "{nepal}"
        );
    }

    use super::super::turn_tail::{
        client_url_block, engine_build_block, version_status, CLIENT_URL_POINTER,
        ENGINE_BUILD_POINTER,
    };
    use crate::llm::anthropic_wire::{build_claude_request, ClaudeRequest, WireTarget};
    use crate::llm::provider::{ContentBlock, Message, MessageContent, ToolDefinition};

    const TEST_URL: &str = "https://example.invalid/v1/messages";
    const TZ: &str = "Europe/Oslo";

    fn system_prompt() -> String {
        format!(
            "You are managing Lucidos, a personal assistant running in the \"myws\" workspace.\
             \n\n{}\n\nUSER LANGUAGE: English{ENGINE_BUILD_POINTER}{CLIENT_URL_POINTER}",
            timezone_section(TZ)
        )
    }

    struct Tail {
        build: (bool, bool),
        origin: &'static str,
    }

    impl Default for Tail {
        fn default() -> Self {
            Self {
                build: (false, false),
                origin: "https://localhost:5173",
            }
        }
    }

    fn turn_messages(history: &str, request: &str, anchor: DateTime<Utc>) -> Vec<Message> {
        turn_messages_with_tail(history, request, anchor, Tail::default())
    }

    fn turn_messages_with_tail(
        history: &str,
        request: &str,
        anchor: DateTime<Utc>,
        tail: Tail,
    ) -> Vec<Message> {
        let (update, behind) = tail.build;
        vec![
            Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "tu_1".to_string(),
                    name: "list_files".to_string(),
                    input: serde_json::json!({"path": "artifacts"}),
                    thought_signature: None,
                }]),
            },
            Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "tu_1".to_string(),
                    content: "notes.md".to_string(),
                }]),
            },
            Message {
                role: "user".to_string(),
                content: MessageContent::Text(format!(
                    "[CONVERSATION HISTORY (recent)]\n{history}\n[END HISTORY]\
                     \n\nRequest: {request}\n\n{}\n\n{}\n\n{}",
                    engine_build_block(&version_status(update, behind)),
                    client_url_block(tail.origin),
                    current_time_block(anchor, TZ)
                )),
            },
        ]
    }

    fn one_tool() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "list_files".to_string(),
            description: "list workspace files".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }]
    }

    fn request_for(messages: Vec<Message>) -> ClaudeRequest {
        build_claude_request(
            messages,
            one_tool(),
            "claude-opus-5",
            Some(&system_prompt()),
            Some("high"),
            WireTarget::Direct { url: TEST_URL },
            "Test",
        )
        .0
    }

    fn sha(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn system_hash(request: &ClaudeRequest) -> String {
        sha(&serde_json::to_vec(&request.system).expect("system serializes"))
    }

    fn message_prefix_hash(request: &ClaudeRequest, last: usize) -> String {
        let canonical: Vec<serde_json::Value> = request.messages[..=last]
            .iter()
            .map(|m| {
                let mut content = m.content.clone();
                if let Some(blocks) = content.as_array_mut() {
                    for block in blocks.iter_mut() {
                        if let Some(obj) = block.as_object_mut() {
                            obj.remove("cache_control");
                        }
                    }
                    if let [serde_json::Value::Object(only)] = blocks.as_slice() {
                        if only.get("type").and_then(|t| t.as_str()) == Some("text") {
                            content = only.get("text").cloned().unwrap_or(content.clone());
                        }
                    }
                }
                serde_json::json!({"role": m.role, "content": content})
            })
            .collect();
        sha(&serde_json::to_vec(&canonical).expect("messages serialize"))
    }

    fn marker_count(request: &ClaudeRequest) -> usize {
        serde_json::to_string(request)
            .expect("request serializes")
            .matches("\"cache_control\"")
            .count()
    }

    #[test]
    fn the_cached_prefix_survives_a_rebuild_and_a_later_turn() {
        let anchor = utc(2026, 8, 17, 13, 2);
        let first = request_for(turn_messages("User: hi", "what is on today?", anchor));
        let rebuilt = request_for(turn_messages("User: hi", "what is on today?", anchor));

        assert_eq!(
            system_hash(&first),
            system_hash(&rebuilt),
            "a rebuild of one turn changed the system block"
        );
        assert_eq!(
            message_prefix_hash(&first, 2),
            message_prefix_hash(&rebuilt, 2),
            "a rebuild of one turn changed the message prefix"
        );

        let later = request_for(turn_messages(
            "User: hi",
            "what is on today?",
            utc(2026, 8, 17, 16, 2),
        ));
        assert_eq!(
            system_hash(&first),
            system_hash(&later),
            "the clock is back in the cached system tier"
        );
        assert_ne!(
            message_prefix_hash(&first, 2),
            message_prefix_hash(&later, 2),
            "the reading must actually be present in the message tier"
        );

        assert_eq!(marker_count(&first), 4);
    }

    #[test]
    fn round_two_finds_round_ones_last_message_unchanged() {
        let anchor = utc(2026, 8, 17, 13, 2);
        let round_one = request_for(turn_messages("User: hi", "what is on today?", anchor));

        let mut grown = turn_messages("User: hi", "what is on today?", anchor);
        grown.push(Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "tu_2".to_string(),
                name: "list_files".to_string(),
                input: serde_json::json!({"path": "."}),
                thought_signature: None,
            }]),
        });
        grown.push(Message {
            role: "user".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "tu_2".to_string(),
                content: "apps".to_string(),
            }]),
        });
        let round_two = request_for(grown);

        assert_eq!(
            message_prefix_hash(&round_one, 2),
            message_prefix_hash(&round_two, 2),
            "round 2 sees a different round-1 prefix, so the miss only moved tiers"
        );
        assert_eq!(system_hash(&round_one), system_hash(&round_two));
        assert_eq!(marker_count(&round_two), 4);
    }

    #[test]
    fn two_threads_in_one_workspace_share_the_system_block() {
        let one = request_for(turn_messages(
            "User: draft the release notes",
            "where did we land?",
            utc(2026, 8, 17, 13, 2),
        ));
        let two = request_for(turn_messages(
            "User: what is the weather",
            "and tomorrow?",
            utc(2026, 8, 17, 9, 47),
        ));

        assert_eq!(
            system_hash(&one),
            system_hash(&two),
            "two threads in one workspace must present the same system block"
        );
        assert_ne!(message_prefix_hash(&one, 2), message_prefix_hash(&two, 2));
    }

    #[test]
    fn one_thread_across_turns_keeps_the_system_block() {
        let first = request_for(turn_messages_with_tail(
            "User: hi",
            "is the change live?",
            utc(2026, 8, 17, 13, 2),
            Tail {
                build: (false, false),
                origin: "https://localhost:5173",
            },
        ));
        let later = request_for(turn_messages_with_tail(
            "User: hi",
            "is the change live?",
            utc(2026, 8, 17, 16, 2),
            Tail {
                build: (true, false),
                origin: "http://localhost:3000",
            },
        ));

        assert_eq!(
            system_hash(&first),
            system_hash(&later),
            "a per-turn value is back in the cached system tier"
        );
        assert_ne!(
            message_prefix_hash(&first, 2),
            message_prefix_hash(&later, 2),
            "the readings must actually be present in the message tier"
        );
        assert_eq!(marker_count(&later), 4);
    }

    #[test]
    fn no_build_state_and_no_origin_reaches_the_system_block() {
        let system = system_prompt();
        for leaked in [
            "RUNNING ENGINE IS CURRENT",
            "HAS NOT SWITCHED ONTO IT YET",
            "NO BUILD BEHIND IT YET",
            "NO REBUILD CAN DELIVER",
            "localhost:5173",
            "localhost:3000",
        ] {
            assert!(
                !system.contains(leaked),
                "the cached system block leaks {leaked}:\n{system}"
            );
        }
        assert!(
            system.contains("[ENGINE BUILD] block at the END"),
            "{system}"
        );
        assert!(system.contains("[CLIENT URL] block at the END"), "{system}");
    }
}
