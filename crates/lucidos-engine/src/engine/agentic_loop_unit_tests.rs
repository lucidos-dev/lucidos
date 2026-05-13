mod should_flush_tests {
    use super::super::{is_bad_image_description, should_flush};

    // --- Paragraph breaks ---

    #[test]
    fn flushes_on_double_newline() {
        assert!(should_flush("Hello world\n\n"));
    }

    #[test]
    fn no_flush_on_single_newline() {
        assert!(!should_flush("Hello world\n"));
    }

    #[test]
    fn no_flush_mid_paragraph() {
        assert!(!should_flush("Hello world"));
    }

    #[test]
    fn flushes_on_multiple_paragraphs() {
        assert!(should_flush("First paragraph.\n\nSecond paragraph.\n\n"));
    }

    // --- Code fence close ---

    #[test]
    fn flushes_on_code_fence_close() {
        assert!(should_flush("```rust\nfn main() {}\n```\n"));
    }

    #[test]
    fn no_flush_on_code_fence_open() {
        assert!(!should_flush("```rust\n"));
    }

    #[test]
    fn no_flush_on_code_fence_without_trailing_newline() {
        assert!(!should_flush("```rust\ncode\n```"));
    }

    // --- Heading after newline ---

    #[test]
    fn flushes_on_heading_followed_by_newline() {
        assert!(should_flush("Some text\n## Heading\n"));
    }

    #[test]
    fn flushes_on_h1_followed_by_newline() {
        assert!(should_flush("Intro\n# Title\n"));
    }

    #[test]
    fn no_flush_on_heading_without_trailing_newline() {
        assert!(!should_flush("Some text\n## Heading"));
    }

    #[test]
    fn flushes_on_first_line_heading() {
        // A complete heading line should always flush
        assert!(should_flush("# Title\n"));
    }

    // --- Horizontal rules ---

    #[test]
    fn flushes_on_dash_horizontal_rule() {
        assert!(should_flush("Some text\n---\n"));
    }

    #[test]
    fn flushes_on_asterisk_horizontal_rule() {
        assert!(should_flush("Some text\n***\n"));
    }

    #[test]
    fn no_flush_on_partial_horizontal_rule() {
        assert!(!should_flush("Some text\n---"));
    }

    // --- Edge cases ---

    #[test]
    fn no_flush_on_empty_string() {
        assert!(!should_flush(""));
    }

    #[test]
    fn no_flush_on_whitespace_only() {
        assert!(!should_flush("   "));
    }

    #[test]
    fn no_flush_on_single_char() {
        assert!(!should_flush("a"));
    }

    #[test]
    fn flushes_long_text_ending_with_paragraph_break() {
        let mut text = "A".repeat(5000);
        text.push_str("\n\n");
        assert!(should_flush(&text));
    }

    #[test]
    fn no_flush_on_long_text_without_boundary() {
        let text = "A".repeat(5000);
        assert!(!should_flush(&text));
    }

    // --- Combinations ---

    #[test]
    fn flushes_code_block_then_paragraph() {
        assert!(should_flush("```\ncode\n```\n\nNext paragraph\n\n"));
    }

    #[test]
    fn flushes_heading_in_middle_of_text() {
        assert!(should_flush("First part\n## Section\n"));
    }

    // --- List items (should NOT flush) ---

    #[test]
    fn no_flush_on_list_item() {
        assert!(!should_flush("- item 1\n"));
    }

    #[test]
    fn no_flush_on_numbered_list() {
        assert!(!should_flush("1. item\n"));
    }

    // --- is_bad_image_description ---

    #[test]
    fn rejects_gemini_no_image_response() {
        assert!(is_bad_image_description(
            "Please provide the images you would like me to describe. I do not see any images attached to your message."
        ));
    }

    #[test]
    fn rejects_contraction_variant() {
        assert!(is_bad_image_description(
            "I don't see any images in the message."
        ));
    }

    #[test]
    fn rejects_no_image_provided() {
        assert!(is_bad_image_description(
            "No image was provided for analysis."
        ));
    }

    #[test]
    fn accepts_valid_description() {
        assert!(!is_bad_image_description(
            "A screenshot of a calendar invitation showing a meeting titled 'Standup' on March 17, 2026 at 09:00-09:15."
        ));
    }

    #[test]
    fn accepts_ocr_description() {
        assert!(!is_bad_image_description(
            "The image shows a document with the text: 'Møte med Alex, 14. mars kl 10:00-11:00'"
        ));
    }
}

mod intent_loop_tools_tests {
    use crate::llm::tool_names as tn;

    /// Intent sub-loops must include notification tools so intents can send
    /// notifications. Regression test for: send_notification silently fails
    /// when called from execute_intent because the tool wasn't in the tool list.
    #[test]
    fn intent_loop_tools_include_send_notification() {
        let tools = super::super::build_intent_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&tn::SEND_NOTIFICATION),
            "Intent loop tools must include send_notification, got: {:?}",
            names
        );
    }

    #[test]
    fn intent_loop_tools_include_read_notifications() {
        let tools = super::super::build_intent_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&tn::READ_NOTIFICATIONS),
            "Intent loop tools must include read_notifications, got: {:?}",
            names
        );
    }

    #[test]
    fn intent_loop_tools_exclude_execute_intent() {
        let tools = super::super::build_intent_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            !names.contains(&tn::EXECUTE_INTENT),
            "Intent loop tools must NOT include execute_intent (no recursion)"
        );
    }
}

mod derive_call_key_tests {
    use super::super::derive_call_key;
    use crate::llm::tool_names as tn;
    use serde_json::json;

    #[test]
    fn run_bash_buckets_by_first_token() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "git status" }));
        assert_eq!(key, "git");
    }

    #[test]
    fn run_bash_trims_leading_whitespace() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "  git  add ." }));
        assert_eq!(key, "git");
    }

    #[test]
    fn run_bash_empty_command_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "" }));
        assert_eq!(key, tn::RUN_BASH);
    }

    #[test]
    fn run_bash_whitespace_only_command_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "   " }));
        assert_eq!(key, tn::RUN_BASH);
    }

    #[test]
    fn run_bash_missing_command_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_BASH, &json!({}));
        assert_eq!(key, tn::RUN_BASH);
    }

    #[test]
    fn run_bash_distinct_commands_bucket_separately() {
        let git = derive_call_key(tn::RUN_BASH, &json!({ "command": "git status" }));
        let cargo = derive_call_key(tn::RUN_BASH, &json!({ "command": "cargo test" }));
        let ls = derive_call_key(tn::RUN_BASH, &json!({ "command": "ls -la" }));
        assert_eq!(git, "git");
        assert_eq!(cargo, "cargo");
        assert_eq!(ls, "ls");
        assert_ne!(git, cargo);
        assert_ne!(cargo, ls);
    }

    #[test]
    fn run_bash_same_prefix_buckets_together() {
        let a = derive_call_key(tn::RUN_BASH, &json!({ "command": "git status" }));
        let b = derive_call_key(tn::RUN_BASH, &json!({ "command": "git add ." }));
        let c = derive_call_key(tn::RUN_BASH, &json!({ "command": "git commit -m x" }));
        assert_eq!(a, "git");
        assert_eq!(b, "git");
        assert_eq!(c, "git");
    }

    #[test]
    fn read_file_keys_by_path_unchanged() {
        let key = derive_call_key(tn::READ_FILE, &json!({ "path": "src/main.rs" }));
        assert_eq!(key, "src/main.rs");
    }

    #[test]
    fn web_search_keys_by_query_unchanged() {
        let key = derive_call_key(tn::WEB_SEARCH, &json!({ "query": "rust async" }));
        assert_eq!(key, "rust async");
    }

    #[test]
    fn non_run_bash_with_command_arg_does_not_bucket_by_command() {
        // Sanity: only run_bash is special-cased. A different tool that happens
        // to carry a `command` arg falls through to the path/url/query lookup.
        let key = derive_call_key(tn::READ_FILE, &json!({ "command": "git status" }));
        assert_eq!(key, "");
    }

    #[test]
    fn non_run_bash_without_known_arg_returns_empty() {
        let key = derive_call_key(tn::LIST_FILES, &json!({}));
        assert_eq!(key, "");
    }
}

mod serialize_messages_for_capture_tests {
    use super::super::serialize_messages_for_capture;
    use crate::llm::{ContentBlock, Message, MessageContent};
    use serde_json::json;

    #[test]
    fn serializes_text_message_with_role_prefix() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text("hello".to_string()),
        }];
        let out = serialize_messages_for_capture(&messages);
        assert!(out.contains("[user]"));
        assert!(out.contains("hello"));
    }

    #[test]
    fn serializes_tool_use_block_inline() {
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "tu_1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "/tmp/x"}),
            }]),
        }];
        let out = serialize_messages_for_capture(&messages);
        assert!(out.contains("[assistant]"));
        assert!(out.contains("[tool_use read_file id=tu_1"));
        assert!(out.contains("/tmp/x"));
    }

    #[test]
    fn serializes_tool_result_block_inline() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "tu_1".to_string(),
                content: "file contents".to_string(),
            }]),
        }];
        let out = serialize_messages_for_capture(&messages);
        assert!(out.contains("[tool_result id=tu_1"));
        assert!(out.contains("file contents"));
    }

    #[test]
    fn separates_messages_with_blank_line() {
        // Boundaries between messages must be visible — without the blank
        // separator the dump reads as one continuous blob.
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: MessageContent::Text("hi".to_string()),
            },
            Message {
                role: "assistant".to_string(),
                content: MessageContent::Text("hello".to_string()),
            },
        ];
        let out = serialize_messages_for_capture(&messages);
        assert!(out.contains("hi\n\n[assistant]"));
    }
}

mod relation_tests {
    use super::super::special_tool::{parse_relation, Relation};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn parse_relation_defaults_to_sub_when_omitted() {
        let args = json!({"prompt": "do the thing"});
        assert!(matches!(parse_relation(&args), Ok(Relation::Sub)));
    }

    #[test]
    fn parse_relation_accepts_sub() {
        let args = json!({"prompt": "x", "relation": "sub"});
        assert!(matches!(parse_relation(&args), Ok(Relation::Sub)));
    }

    #[test]
    fn parse_relation_accepts_top() {
        let args = json!({"prompt": "x", "relation": "top"});
        assert!(matches!(parse_relation(&args), Ok(Relation::Top)));
    }

    #[test]
    fn parse_relation_rejects_unknown_value() {
        let args = json!({"prompt": "x", "relation": "sibling"});
        let err = parse_relation(&args).expect_err("unknown value must error");
        assert!(
            err.contains("relation must be") && err.contains("sibling"),
            "error must name the bad value: {}",
            err
        );
    }

    #[test]
    fn parse_relation_rejects_non_string() {
        // {"relation": 42} — wrong type. The parser must fail loudly so the
        // LLM sees the contract violation instead of silently getting "sub".
        let args = json!({"prompt": "x", "relation": 42});
        assert!(parse_relation(&args).is_err());
    }

    #[test]
    fn sub_linkage_passes_parent_and_event_through() {
        let spawning_thread = Uuid::new_v4();
        let tool_event = Uuid::new_v4();
        let (parent, event) = Relation::Sub.spawn_linkage(spawning_thread, Some(tool_event));
        assert_eq!(parent, Some(spawning_thread));
        assert_eq!(event, Some(tool_event));
    }

    #[test]
    fn top_linkage_drops_parent_and_event() {
        let spawning_thread = Uuid::new_v4();
        let tool_event = Uuid::new_v4();
        let (parent, event) = Relation::Top.spawn_linkage(spawning_thread, Some(tool_event));
        assert_eq!(
            parent, None,
            "top relation must clear parent_thread_id so notify_parent_if_child does not fire"
        );
        assert_eq!(
            event, None,
            "top relation must clear spawning_event_id (no callback target to trace to)"
        );
    }

    #[test]
    fn sub_run_thread_text_promises_auto_resume() {
        let child = Uuid::new_v4();
        let text = Relation::Sub.run_thread_success_text(child);
        assert!(
            text.contains("automatically resume"),
            "sub spawn must tell the LLM the parent will auto-resume: {}",
            text
        );
        assert!(text.contains(&child.to_string()));
    }

    #[test]
    fn top_run_thread_text_states_no_callback() {
        let child = Uuid::new_v4();
        let text = Relation::Top.run_thread_success_text(child);
        assert!(
            text.contains("NOT report back") || text.contains("not report back"),
            "top spawn must explicitly say there is no callback: {}",
            text
        );
        assert!(text.contains(&child.to_string()));
    }

    #[test]
    fn sub_run_claude_text_describes_new_thread() {
        let child = Uuid::new_v4();
        let text = Relation::Sub.run_claude_success_text(child);
        assert!(text.contains("Claude Code"));
        assert!(text.contains(&child.to_string()));
    }

    #[test]
    fn top_run_claude_text_states_no_callback() {
        let child = Uuid::new_v4();
        let text = Relation::Top.run_claude_success_text(child);
        assert!(
            text.contains("NOT report back") || text.contains("not report back"),
            "top CC spawn must explicitly say there is no callback: {}",
            text
        );
        assert!(text.contains(&child.to_string()));
    }
}

mod sentinel_redaction_tests {
    use super::super::match_sentinel;
    use crate::engine::thread_events::ThreadEvent;
    use crate::engine::tools::credentials::CREDENTIAL_REQUEST_PREFIX;
    use crate::engine::tools::plugins::{
        PLUGIN_INSTALL_REQUEST_PREFIX, PLUGIN_UNINSTALL_REQUEST_PREFIX,
    };

    #[test]
    fn no_sentinel_returns_none() {
        assert!(match_sentinel("plain tool result").is_none());
        assert!(match_sentinel("Error: file not found").is_none());
        assert!(match_sentinel("").is_none());
    }

    #[test]
    fn install_sentinel_extracts_payload_and_redacts() {
        let raw = format!(
            "{PLUGIN_INSTALL_REQUEST_PREFIX}{}",
            r#"{"install_id":"abc","files":["apps/x/index.html"]}"#
        );
        let m = match_sentinel(&raw).expect("install sentinel must match");
        match m.event {
            ThreadEvent::PluginInstallRequest { payload } => {
                assert!(payload.starts_with('{'), "payload must be the JSON object");
                assert!(payload.contains("install_id"));
                assert!(payload.contains("apps/x/index.html"));
            }
            other => panic!("expected PluginInstallRequest, got {:?}", other),
        }
        let redacted = m.redacted_text.expect("install must redact for the LLM");
        // Redacted text MUST NOT leak the JSON to the LLM — that's the bug
        // we're fixing (LLM was parsing `overwrites` and chat-asking the user).
        assert!(
            !redacted.contains('{'),
            "redacted text must not contain the JSON payload: {}",
            redacted
        );
        assert!(
            !redacted.contains(PLUGIN_INSTALL_REQUEST_PREFIX),
            "redacted text must not contain the sentinel prefix: {}",
            redacted
        );
        // Must instruct the LLM to wait, not respond, not chat-ask.
        let lower = redacted.to_lowercase();
        assert!(
            lower.contains("panel") && (lower.contains("confirm") || lower.contains("cancel")),
            "redacted text must explain the panel resolves it: {}",
            redacted
        );
    }

    #[test]
    fn uninstall_sentinel_extracts_payload_and_redacts() {
        let raw = format!(
            "{PLUGIN_UNINSTALL_REQUEST_PREFIX}{}",
            r#"{"uninstall_id":"xyz","plugin_id":"foo","files":["apps/foo/index.html"]}"#
        );
        let m = match_sentinel(&raw).expect("uninstall sentinel must match");
        match m.event {
            ThreadEvent::PluginUninstallRequest { payload } => {
                assert!(payload.contains("uninstall_id"));
                assert!(payload.contains("foo"));
            }
            other => panic!("expected PluginUninstallRequest, got {:?}", other),
        }
        let redacted = m.redacted_text.expect("uninstall must redact for the LLM");
        assert!(!redacted.contains('{'));
        assert!(!redacted.contains(PLUGIN_UNINSTALL_REQUEST_PREFIX));
    }

    #[test]
    fn credential_sentinel_extracts_payload_and_redacts() {
        let raw = format!(
            "{CREDENTIAL_REQUEST_PREFIX}{}",
            r#"{"service":"openai","prompt":"Enter API key","base_url":"https://api.openai.com","auth_type":"api_key"}"#
        );
        let m = match_sentinel(&raw).expect("credential sentinel must match");
        match m.event {
            ThreadEvent::CredentialRequest { payload } => {
                assert!(payload.contains("openai"));
            }
            other => panic!("expected CredentialRequest, got {:?}", other),
        }
        let redacted = m.redacted_text.expect("credential must redact for the LLM");
        assert!(!redacted.contains('{'));
    }

    #[test]
    fn email_confirm_sentinel_emits_event_but_does_not_redact() {
        let raw = "[EMAIL_CONFIRM]{\"to\":[\"a@b\"],\"subject\":\"hi\"}".to_string();
        let m = match_sentinel(&raw).expect("email confirm sentinel must match");
        match m.event {
            ThreadEvent::EmailConfirmRequest { payload } => {
                assert!(payload.contains("a@b"));
            }
            other => panic!("expected EmailConfirmRequest, got {:?}", other),
        }
        assert!(
            m.redacted_text.is_none(),
            "email-confirm must pass through unredacted (its tool description already explains the modal)"
        );
    }

    #[test]
    fn sentinel_without_json_returns_none() {
        // Defensive: sentinel prefix without `{` afterwards should not match
        // (the agentic loop pre-redaction code skipped these too).
        let raw = format!("{PLUGIN_INSTALL_REQUEST_PREFIX}no json here");
        assert!(match_sentinel(&raw).is_none());
    }
}

mod run_tool_with_cancel_tests {
    use super::super::run_tool_with_cancel;
    use crate::engine::tools::ToolOutcome;
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    /// A tool future that completes normally must produce its own result —
    /// the wrapper passes through when no cancel is signaled.
    #[tokio::test]
    async fn returns_tool_result_when_not_cancelled() {
        let cancel_token = CancellationToken::new();
        let fut = async { Ok::<String, String>("ok".to_string()) };
        assert_eq!(
            run_tool_with_cancel(fut, &cancel_token).await,
            Ok("ok".to_string())
        );
    }

    /// Pre-cancelled token MUST short-circuit even when the inner future is
    /// instantly ready. The `biased` poll order in the wrapper guarantees the
    /// cancel arm wins the race — without `biased`, an already-cancelled token
    /// plus a synchronously-ready tool would still let the tool result win
    /// (tokio::select picks fairly otherwise), letting the loop run one more
    /// iteration past the cancel signal.
    #[tokio::test]
    async fn pre_cancelled_token_short_circuits_even_when_tool_ready() {
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let fut = async { Ok::<String, String>("ok".to_string()) };
        assert_eq!(
            run_tool_with_cancel(fut, &cancel_token).await,
            Err("Error: canceled by user".to_string())
        );
    }

    /// Mid-flight cancel: the tool future is parked on a long sleep when
    /// the cancel signal arrives. The wrapper must observe the cancel within
    /// the polling latency — NOT wait for the tool to finish. This is the
    /// regression contract: a hung `urlopen()` (no timeout) must not block
    /// cancel from completing.
    #[tokio::test]
    async fn cancel_aborts_long_running_tool_quickly() {
        let cancel_token = CancellationToken::new();
        let cancel_clone = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let slow_fut = async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok::<String, String>("should never see this".to_string())
        };

        let start = Instant::now();
        let result = run_tool_with_cancel(slow_fut, &cancel_token).await;
        let elapsed = start.elapsed();

        assert_eq!(result, Err("Error: canceled by user".to_string()));
        assert!(
            elapsed < Duration::from_millis(500),
            "cancel must abort within polling latency (got {:?}); the wrapper is \
             not racing the tool against cancel_token.cancelled() if this trips",
            elapsed
        );
    }

    /// Cancel must surface as `Err` so the agent loop classifies it as a
    /// tool failure and stamps `success: false` on the persisted ToolResult.
    /// This is the typed-Result successor to the legacy "starts_with(\"Error:\")"
    /// classifier — `ToolOutcome::Err(_)` is the contracted failure shape.
    #[tokio::test]
    async fn canceled_result_is_typed_err_for_caller() {
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let fut = async { Ok::<String, String>("ok".to_string()) };
        let result: ToolOutcome = run_tool_with_cancel(fut, &cancel_token).await;
        assert!(
            result.is_err(),
            "cancel must produce a typed Err so the agent loop stamps \
             ToolResult.success=false; got {:?}",
            result
        );
    }
}
