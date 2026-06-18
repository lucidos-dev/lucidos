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
                thought_signature: None,
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
    fn parse_relation_defaults_to_child_when_omitted() {
        let args = json!({"prompt": "do the thing"});
        assert!(matches!(parse_relation(&args), Ok(Relation::Child)));
    }

    #[test]
    fn parse_relation_accepts_child() {
        let args = json!({"prompt": "x", "relation": "child"});
        assert!(matches!(parse_relation(&args), Ok(Relation::Child)));
    }

    #[test]
    fn parse_relation_accepts_sub_as_back_compat_alias() {
        // The wire string was `"sub"` before the glossary settled on
        // *child thread* (direct descendant) vs *sub-thread* (transitive).
        // Older LLM tool calls and persisted prompts may still send `"sub"`,
        // so it must keep deserializing to Child.
        let args = json!({"prompt": "x", "relation": "sub"});
        assert!(matches!(parse_relation(&args), Ok(Relation::Child)));
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
        // LLM sees the contract violation instead of silently getting "child".
        let args = json!({"prompt": "x", "relation": 42});
        assert!(parse_relation(&args).is_err());
    }

    #[test]
    fn child_linkage_passes_parent_and_event_through() {
        let spawning_thread = Uuid::new_v4();
        let tool_event = Uuid::new_v4();
        let (parent, event) = Relation::Child.spawn_linkage(spawning_thread, Some(tool_event));
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
    fn child_run_thread_text_promises_auto_resume() {
        let child = Uuid::new_v4();
        let text = Relation::Child.run_thread_success_text(child, "dev");
        assert!(
            text.contains("automatically resume"),
            "child spawn must tell the LLM the parent will auto-resume: {}",
            text
        );
        assert!(text.contains(&child.to_string()));
    }

    #[test]
    fn top_run_thread_text_states_no_callback() {
        let child = Uuid::new_v4();
        let text = Relation::Top.run_thread_success_text(child, "dev");
        assert!(
            text.contains("NOT report back") || text.contains("not report back"),
            "top spawn must explicitly say there is no callback: {}",
            text
        );
        assert!(text.contains(&child.to_string()));
    }

    #[test]
    fn child_run_coding_agent_text_describes_new_thread() {
        let child = Uuid::new_v4();
        let text = Relation::Child.run_coding_agent_success_text(
            child,
            "dev",
            crate::runtime::CodingAgent::ClaudeCode,
        );
        assert!(text.contains("Claude Code"));
        assert!(text.contains(&child.to_string()));
    }

    #[test]
    fn top_run_coding_agent_text_states_no_callback() {
        let child = Uuid::new_v4();
        let text = Relation::Top.run_coding_agent_success_text(
            child,
            "dev",
            crate::runtime::CodingAgent::ClaudeCode,
        );
        assert!(
            text.contains("NOT report back") || text.contains("not report back"),
            "top CC spawn must explicitly say there is no callback: {}",
            text
        );
        assert!(text.contains(&child.to_string()));
    }

    #[test]
    fn run_coding_agent_text_stamps_workspace_into_body_and_link() {
        let child = Uuid::new_v4();
        for relation in [Relation::Child, Relation::Top] {
            let text = relation.run_coding_agent_success_text(
                child,
                "personal",
                crate::runtime::CodingAgent::ClaudeCode,
            );
            assert!(
                text.contains("workspace 'personal'"),
                "{:?} CC ack must name the workspace in the body: {}",
                relation,
                text
            );
            assert!(
                text.contains(&format!("thread:personal/{}", child)),
                "{:?} CC ack link must be workspace-prefixed: {}",
                relation,
                text
            );
        }
    }

    #[test]
    fn run_thread_text_stamps_workspace_into_body_and_link() {
        let child = Uuid::new_v4();
        for relation in [Relation::Child, Relation::Top] {
            let text = relation.run_thread_success_text(child, "work");
            assert!(
                text.contains("workspace 'work'"),
                "{:?} thread ack must name the workspace in the body: {}",
                relation,
                text
            );
            assert!(
                text.contains(&format!("thread:work/{}", child)),
                "{:?} thread ack link must be workspace-prefixed: {}",
                relation,
                text
            );
        }
    }
}
