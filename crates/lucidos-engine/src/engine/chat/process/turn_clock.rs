//! The turn's time context, split across two prompt tiers (ADR 0084).
//!
//! Anthropic caches by prefix. A value changing every turn, anywhere in the
//! system block, rewrites that whole tier at every turn boundary. A wire probe
//! measured 45,824 tokens rewritten against 27,145 read. It also found two
//! unrelated threads at the same 58,854 system bytes, hashing differently. The
//! clock alone kept a workspace-level block from being shared.
//!
//! Two rules come out of that, one per half:
//!
//! - [`timezone_section`] takes NO timestamp. It renders only from the IANA
//!   name, so the cached block cannot notice the time or the thread.
//! - [`current_time_block`] takes [`LucidosEngine::turn_started_at`], never a
//!   wall clock. The message array is rebuilt from events per request, so the
//!   block must come from persisted state. Otherwise the miss just moves tiers.

use crate::engine::LucidosEngine;
use chrono::{DateTime, Utc};
use uuid::Uuid;

impl LucidosEngine {
    /// When this turn started: the newest event already on the thread.
    ///
    /// Persisted state rather than `Utc::now()`, so it is fixed for the whole
    /// turn and every round of it sends the same bytes.
    ///
    /// NOT the turn anchor's own `created`. The anchor answers which exchange
    /// the turn's events group under, and the two part company on the
    /// answer-driven resume: `ChatResumeAnchor::ExistingTurn` deliberately
    /// re-uses the interrupted turn's `request_event_id`, which can be from
    /// yesterday. The newest event is the answer that woke the thread.
    ///
    /// An empty or unreadable thread falls back to the wall clock. A turn with
    /// no clock at all is worse than one turn's cache miss, and the log line
    /// names which happened.
    pub(super) async fn turn_started_at(&self, thread_id: Uuid) -> DateTime<Utc> {
        match self
            .event_store
            .thread_latest_event_created(thread_id)
            .await
        {
            Ok(Some(created)) => created,
            Ok(None) => {
                crate::log!(
                    "[Chat] thread {} has no events to read a clock from; using the wall clock for CURRENT TIME",
                    thread_id
                );
                Utc::now()
            }
            Err(e) => {
                crate::log!(
                    "[Chat] turn clock read failed ({}); using the wall clock for CURRENT TIME",
                    e
                );
                Utc::now()
            }
        }
    }
}

/// The invariant half, spliced into the cached system block.
///
/// Names the timezone and the rules for handling it, and points at the
/// reading. No timestamp reaches this text.
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

/// The volatile half, appended after the request line so it rides in the
/// message tier rather than the cached system block.
///
/// `at` is the turn start instant. Each line carries its own date: the local
/// and UTC dates disagree either side of midnight, and one date against two
/// clock readings is how the agent gets "yesterday" wrong.
///
/// The DST offset is here rather than in the prose because it is clock-derived,
/// and the system tier holds nothing clock-derived.
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
        // `%:z`, not a whole-hour division. India is +05:30 and Nepal +05:45.
        // Truncating either leaves the agent converting off by half an hour
        // from a figure the prompt stated as fact.
        offset = local.format("%:z"),
    )
}

/// "Sunday, August 17, 2026", in whichever zone the value carries.
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

    /// The measured bug, as an assertion. Two turns differing only in when they
    /// happened must produce the same system bytes.
    ///
    /// The anchors sit either side of a DST transition, the hardest case: it
    /// moves offset, date, time and weekday at once. The old code spliced all
    /// four into the system block.
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

        // Nothing clock-shaped survives in the cached tier: no reading, no
        // weekday, no offset (that last one moves twice a year on its own).
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

    /// A zone whose offset differs between the anchors proves the offset really
    /// moved out of the system block, rather than being absent from one sample.
    #[test]
    fn the_offset_rides_with_the_reading_across_a_dst_transition() {
        let winter = current_time_block(utc(2026, 1, 14, 13, 2), "Europe/Oslo");
        let summer = current_time_block(utc(2026, 8, 17, 13, 2), "Europe/Oslo");

        assert!(winter.contains("14:02 Europe/Oslo (UTC+01:00)"), "{winter}");
        assert!(summer.contains("15:02 Europe/Oslo (UTC+02:00)"), "{summer}");
    }

    /// The agent answers "what time is it" from this block alone, so every part
    /// it needs is in it: local date, local time, IANA name, offset, and UTC.
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

    /// The old text dated the UTC instant, then paired it with a local
    /// time-of-day. A late-evening turn therefore named the wrong day, and
    /// every "yesterday" after it was off by one.
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

    /// Timezone is a mandatory preference, so an empty one means setup mode.
    /// The agent still needs a clock to run that interview.
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

    /// A garbage IANA name must not lose the reading. It falls back to UTC,
    /// which the offset then reports honestly as `+0`.
    #[test]
    fn an_unparseable_timezone_falls_back_to_utc_rather_than_dropping_the_time() {
        let block = current_time_block(utc(2026, 8, 17, 13, 2), "Not/AZone");
        assert!(block.contains("at 13:02 Not/AZone (UTC+00:00)."), "{block}");
    }

    /// A negative offset renders with its sign, not as a bare number.
    #[test]
    fn a_western_timezone_renders_a_signed_offset() {
        let block = current_time_block(utc(2026, 8, 17, 13, 2), "America/New_York");
        assert!(
            block.contains("at 09:02 America/New_York (UTC-04:00)."),
            "{block}"
        );
    }

    /// Not every zone is a whole hour from UTC. India is +05:30 and Nepal
    /// +05:45, and a truncated offset is a figure the prompt states as fact and
    /// the agent then converts from.
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

    // ===== The wire, where the cache actually looks =====
    //
    // The tests above pin the clock's two halves. These assemble a real
    // Anthropic request and assert on its serialized bytes, which is what
    // Anthropic hashes and what `llm::cache_probe` reports.
    //
    // Scope is the whole cached-tier contract rather than the clock alone,
    // because the property is one property: `super::turn_tail`'s two values
    // split the same way and share this harness.

    use super::super::turn_tail::{
        client_url_block, engine_build_block, version_status, CLIENT_URL_POINTER,
        ENGINE_BUILD_POINTER,
    };
    use crate::llm::anthropic_wire::{build_claude_request, ClaudeRequest, WireTarget};
    use crate::llm::provider::{ContentBlock, Message, MessageContent, ToolDefinition};

    const TEST_URL: &str = "https://example.invalid/v1/messages";
    const TZ: &str = "Europe/Oslo";

    /// The system block in miniature: the identity framing, the timezone
    /// section, the language line, and the two pointers at the values that
    /// left this tier. The rest of the real prompt is workspace-level static
    /// text that would only pad the bytes.
    fn system_prompt() -> String {
        format!(
            "You are managing Lucidos, a personal assistant running in the \"myws\" workspace.\
             \n\n{}\n\nUSER LANGUAGE: English{ENGINE_BUILD_POINTER}{CLIENT_URL_POINTER}",
            timezone_section(TZ)
        )
    }

    /// The per-turn readings, in the order `run.rs` appends them.
    struct Tail {
        /// `(update_available, source_behind_head, rebuild_wedged)`.
        build: (bool, bool, bool),
        origin: &'static str,
    }

    impl Default for Tail {
        fn default() -> Self {
            Self {
                build: (false, false, false),
                origin: "https://localhost:5173",
            }
        }
    }

    /// One turn's messages, in the order `run.rs` builds them: a resume tool
    /// pair, then the user message whose parts end with the request line and
    /// then the three tail blocks.
    fn turn_messages(history: &str, request: &str, anchor: DateTime<Utc>) -> Vec<Message> {
        turn_messages_with_tail(history, request, anchor, Tail::default())
    }

    fn turn_messages_with_tail(
        history: &str,
        request: &str,
        anchor: DateTime<Utc>,
        tail: Tail,
    ) -> Vec<Message> {
        let (update, behind, wedged) = tail.build;
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
                    engine_build_block(&version_status(update, behind, wedged)),
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

    /// The message prefix Anthropic looks up, through `last`, canonicalized to
    /// the content it keys on rather than the envelope we happened to send.
    ///
    /// Two encodings are undone, both from the wire's breakpoint placement on
    /// the last two messages. It adds `cache_control`, and it rewrites bare
    /// string content into a one-block array (the only shape that accepts the
    /// marker). Neither changes a token, which is what the ~99% mid-turn read
    /// rate in `data/artifacts/prompt-cache-clock-invalidation.md` shows. Left
    /// in, both would read as prefix churn every time a breakpoint advances.
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

    /// Rebuilding one turn must reproduce every cached byte, and a LATER turn
    /// must still find the same system block waiting for it.
    ///
    /// The second half is the measured bug: `system_bytes` held constant while
    /// `system_hash` moved, so a boundary two minutes old rewrote 45,824 tokens.
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

        // Three hours later, same conversation state. Only the reading moves,
        // and it moves in the message tier where it is uncached anyway.
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

        // tools[-1], system, messages[-1] and messages[-2]. All four are spent.
        assert_eq!(marker_count(&first), 4);
    }

    /// Round 2 pushes the breakpoint onto a newer last message, so round 1's
    /// last message is now INSIDE the cached prefix. It has to be byte-identical
    /// to what round 1 sent, or the fix moved the miss rather than removing it.
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

    /// The system block is workspace-level, so two unrelated threads should
    /// share one cache entry. A probe saw both at 58,854 bytes with differing
    /// hashes, which the clock alone explained.
    ///
    /// Equal byte counts are not proof of equal content, which is why this
    /// asserts the hash. It also catches a future thread-scoped value leaking
    /// into the block, which would break sharing without moving the count.
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
        // Guards against passing on an empty request: the threads really do
        // differ everywhere the system block is not.
        assert_ne!(message_prefix_hash(&one, 2), message_prefix_hash(&two, 2));
    }

    /// The guard above compares two threads at ONE moment, so a value that is
    /// per-TURN rather than per-thread reads the same on both and passes. That
    /// is exactly how the engine build state and the client URL sat in the
    /// cached tier undetected. This is the missing axis: one thread, two turns,
    /// with both of those values moved.
    ///
    /// A build flip alone was measured costing a full system-tier rewrite, at
    /// 21,668 tokens times the write-minus-read rate. The origin flip is the
    /// same shape, for a user reaching one workspace from two clients.
    #[test]
    fn one_thread_across_turns_keeps_the_system_block() {
        let first = request_for(turn_messages_with_tail(
            "User: hi",
            "is the change live?",
            utc(2026, 8, 17, 13, 2),
            Tail {
                build: (false, false, false),
                origin: "https://localhost:5173",
            },
        ));
        // Later: a newer engine is built and unswitched, and they moved to the
        // desktop app on another origin.
        let later = request_for(turn_messages_with_tail(
            "User: hi",
            "is the change live?",
            utc(2026, 8, 17, 16, 2),
            Tail {
                build: (true, false, false),
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

    /// The same claim from the other side, and the one a hash cannot make
    /// readable: no build state and no URL appears anywhere in the system
    /// block's bytes, whichever turn built it.
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
        // It still POINTS at both, or the agent cannot find them.
        assert!(
            system.contains("[ENGINE BUILD] block at the END"),
            "{system}"
        );
        assert!(system.contains("[CLIENT URL] block at the END"), "{system}");
    }
}
