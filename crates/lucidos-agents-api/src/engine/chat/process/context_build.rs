use super::turn_tail::TurnTail;
use crate::engine::loaded_knowhow::LoadedKnowhow;

const SECTION_PERSIST_MAX: usize = 8_000;

pub(super) fn summarize_user_topics(
    prior: &[crate::core::store::SessionMessage],
    user_message: &str,
) -> String {
    let user_topics: Vec<&str> = prior
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();
    let mut summary = user_topics.join(" | ");
    summary.push_str(" | ");
    summary.push_str(user_message);
    summary.chars().take(500).collect()
}

struct LabeledSection<'a> {
    name: &'static str,
    content: &'a str,
    role: crate::engine::ContextRole,
    group: Option<&'static str>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_capture_sections(
    system_prompt: &str,
    profile_context: &str,
    device_preferences_context: &str,
    file_list_context: &str,
    credentials_context: &str,
    email_accounts_context: &str,
    oauth_context: &str,
    memory_context: &str,
    history_context: &str,
    app_context_section: &str,
    file_context_section: &str,
    url_context_section: &str,
    mcp_servers_context: &str,
    mcp_dropped_notice: &str,
    trigger_knowhow_context: &str,
    setup_reminder: &str,
    thread_depth_context: &str,
    todo_list_block: &str,
    user_message: &str,
    tail: &super::turn_tail::TurnTail<'_>,
    loaded_knowhow_docs: &[LoadedKnowhow],
    resume_tool_blocks: &[crate::llm::Message],
) -> Vec<crate::engine::ContextSection> {
    use crate::engine::context::truncate_head_tail;
    use crate::engine::{ContextRole, ContextSection};

    let cap = crate::engine::eval_capture::body_cap(SECTION_PERSIST_MAX);
    let truncate = |content: &str| -> Option<String> {
        Some(match cap {
            Some(cap) if content.len() > cap => truncate_head_tail(content, cap),
            _ => content.to_string(),
        })
    };

    let labeled: [LabeledSection; 23] = [
        LabeledSection {
            name: "System Instructions",
            content: system_prompt,
            role: ContextRole::System,
            group: None,
        },
        LabeledSection {
            name: "User Profile",
            content: profile_context,
            role: ContextRole::User,
            group: Some("Identity & profile"),
        },
        LabeledSection {
            name: "Device & Preferences",
            content: device_preferences_context,
            role: ContextRole::User,
            group: Some("Identity & profile"),
        },
        LabeledSection {
            name: "File List",
            content: file_list_context,
            role: ContextRole::User,
            group: Some("Workspace inventory"),
        },
        LabeledSection {
            name: "Credentials",
            content: credentials_context,
            role: ContextRole::User,
            group: Some("Workspace inventory"),
        },
        LabeledSection {
            name: "Email Accounts",
            content: email_accounts_context,
            role: ContextRole::User,
            group: Some("Workspace inventory"),
        },
        LabeledSection {
            name: "OAuth",
            content: oauth_context,
            role: ContextRole::User,
            group: Some("Workspace inventory"),
        },
        LabeledSection {
            name: super::context_mode::MEMORY_SECTION,
            content: memory_context,
            role: ContextRole::User,
            group: Some("Memory & history"),
        },
        LabeledSection {
            name: super::context_mode::HISTORY_SECTION,
            content: history_context,
            role: ContextRole::User,
            group: Some("Memory & history"),
        },
        LabeledSection {
            name: "App Context",
            content: app_context_section,
            role: ContextRole::User,
            group: Some("Active context"),
        },
        LabeledSection {
            name: "File Context",
            content: file_context_section,
            role: ContextRole::User,
            group: Some("Active context"),
        },
        LabeledSection {
            name: "URL Context",
            content: url_context_section,
            role: ContextRole::User,
            group: Some("Active context"),
        },
        LabeledSection {
            name: "MCP Servers",
            content: mcp_servers_context,
            role: ContextRole::User,
            group: Some("System notices"),
        },
        LabeledSection {
            name: "MCP Tools Not Sent",
            content: mcp_dropped_notice,
            role: ContextRole::User,
            group: Some("System notices"),
        },
        LabeledSection {
            name: "Trigger Know-how",
            content: trigger_knowhow_context,
            role: ContextRole::User,
            group: Some("Workspace inventory"),
        },
        LabeledSection {
            name: "Setup Reminder",
            content: setup_reminder,
            role: ContextRole::User,
            group: Some("System notices"),
        },
        LabeledSection {
            name: "Thread Depth",
            content: thread_depth_context,
            role: ContextRole::User,
            group: Some("System notices"),
        },
        LabeledSection {
            name: "Todo List",
            content: todo_list_block,
            role: ContextRole::User,
            group: Some("System notices"),
        },
        LabeledSection {
            name: "User Message",
            content: user_message,
            role: ContextRole::User,
            group: Some("The request"),
        },
        LabeledSection {
            name: "Child Threads",
            content: tail.child_threads,
            role: ContextRole::User,
            group: Some("The request"),
        },
        LabeledSection {
            name: "Engine Build",
            content: tail.engine_build,
            role: ContextRole::User,
            group: Some("The request"),
        },
        LabeledSection {
            name: "Client URL",
            content: tail.client_url,
            role: ContextRole::User,
            group: Some("The request"),
        },
        LabeledSection {
            name: "Current Time",
            content: tail.current_time,
            role: ContextRole::User,
            group: Some("The request"),
        },
    ];

    let mut capture_sections: Vec<ContextSection> = labeled
        .into_iter()
        .filter(|ls| !ls.content.is_empty())
        .map(|ls| {
            let chars = ls.content.chars().count();
            ContextSection {
                name: ls.name.to_string(),
                content: truncate(ls.content),
                budget_delta_chars: chars,
                content_chars: Some(chars),
                role: ls.role,
                group: ls.group.map(|s| s.to_string()),
            }
        })
        .collect();

    for doc in loaded_knowhow_docs {
        let chars = doc.body.chars().count();
        capture_sections.push(ContextSection {
            name: format!("knowhow: {} ({})", doc.id, doc.retention_note()),
            content: truncate(&doc.body),
            budget_delta_chars: chars,
            content_chars: Some(chars),
            role: ContextRole::User,
            group: Some("Loaded knowhow".to_string()),
        });
    }

    for pair in resume_tool_blocks.chunks(2) {
        if pair.len() != 2 {
            continue;
        }
        let (use_msg, result_msg) = (&pair[0], &pair[1]);
        let (tool_name, args_preview) = match &use_msg.content {
            crate::llm::MessageContent::Blocks(blocks) => match blocks.first() {
                Some(crate::llm::ContentBlock::ToolUse { name, input, .. }) => (
                    name.clone(),
                    serde_json::to_string(input).unwrap_or_default(),
                ),
                _ => ("?".to_string(), String::new()),
            },
            _ => ("?".to_string(), String::new()),
        };
        let result_body = match &result_msg.content {
            crate::llm::MessageContent::Blocks(blocks) => match blocks.first() {
                Some(crate::llm::ContentBlock::ToolResult { content, .. }) => content.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let pair_body = format!(
            "ToolUse: {}({})\n\nToolResult:\n{}",
            tool_name, args_preview, result_body
        );
        let pair_chars = pair_body.chars().count();
        capture_sections.push(ContextSection {
            name: format!("ToolUse: {}", tool_name),
            content: truncate(&pair_body),
            budget_delta_chars: pair_chars,
            content_chars: Some(pair_chars),
            role: ContextRole::PriorMessage,
            group: None,
        });
    }

    capture_sections
}

/// INVARIANT: Build the `[LOADED KNOWHOW]` block injected into the user message: the only
/// place a loaded doc's body rides after the turn that loaded it. Returns
/// `None` for an empty loaded set so callers can skip pushing an empty section.
///
/// `doc.body` is already the formatted `[SYSTEM-KNOWHOW: <name>]` /
/// `[KNOW-HOW: <name>]` block produced by
/// `core::knowhow::load_one_knowhow_section`, pushed verbatim so the LLM
/// sees the same markers it did on first-turn inline returns. Each doc gets a
/// line with its id, since the marker carries the name.
///
/// The block is part of the stable context block, so its bytes change only when
/// a doc loads or leaves. A doc's countdown ticks every turn and rides in
/// [`build_knowhow_retention_block`] instead.
pub(crate) fn build_loaded_knowhow_block(docs: &[LoadedKnowhow]) -> Option<String> {
    if docs.is_empty() {
        return None;
    }
    let mut s = format!(
        "[LOADED KNOWHOW]\nThe following knowhow docs are loaded for this thread. Treat their guidance as authoritative.\n\
         A doc leaves after {} finished turns that do not load it. The [KNOWHOW RETENTION] section below says how many turns each one has left. To keep one you still follow, call load_knowhow on it again: that returns no body. Call unload_knowhow on one you are done with.\n",
        crate::engine::loaded_knowhow::IDLE_TURNS_BEFORE_UNLOAD
    );
    for doc in docs {
        s.push_str(&format!("\n(`{}`)\n", doc.id));
        s.push_str(&doc.body);
    }
    s.push_str("\n[END LOADED KNOWHOW]");
    Some(s)
}

pub(crate) fn build_knowhow_retention_block(docs: &[LoadedKnowhow]) -> Option<String> {
    if docs.is_empty() {
        return None;
    }
    let mut s = String::from("[KNOWHOW RETENTION]");
    for doc in docs {
        s.push_str(&format!("\n`{}` {}", doc.id, doc.retention_note()));
    }
    s.push_str("\n[END KNOWHOW RETENTION]");
    Some(s)
}

#[derive(Default)]
pub(crate) struct TurnSections<'a> {
    pub recent: &'a str,
    pub profile: &'a str,
    pub email_accounts: &'a str,
    pub oauth: &'a str,
    pub thread_depth: &'a str,
    pub loaded_knowhow: &'a str,
    pub device_preferences: &'a str,
    pub memory: &'a str,
    pub history: &'a str,
    pub file_list: &'a str,
    pub credentials: &'a str,
    pub knowhow_retention: &'a str,
    pub app_context: &'a str,
    pub file_context: &'a str,
    pub url_context: &'a str,
    pub todo_list: &'a str,
    pub mcp_servers: &'a str,
    pub mcp_dropped_notice: &'a str,
    pub trigger_knowhow: &'a str,
    pub setup_reminder: &'a str,
    pub request: &'a str,
    pub tail: TurnTail<'a>,
}

pub(crate) struct TurnText {
    pub stable: String,
    pub volatile: String,
}

pub(crate) fn assemble_turn_text(s: &TurnSections<'_>) -> TurnText {
    let join = |parts: &[&str]| {
        parts
            .iter()
            .copied()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(super::context_mode::PART_SEPARATOR)
    };
    let request = format!("Request: {}", s.request);
    TurnText {
        stable: join(&[
            s.profile,
            s.email_accounts,
            s.oauth,
            s.thread_depth,
            s.loaded_knowhow,
        ]),
        volatile: join(&[
            s.recent,
            s.device_preferences,
            s.memory,
            s.history,
            s.file_list,
            s.credentials,
            s.knowhow_retention,
            s.app_context,
            s.file_context,
            s.url_context,
            s.todo_list,
            s.mcp_servers,
            s.mcp_dropped_notice,
            s.trigger_knowhow,
            s.setup_reminder,
            &request,
            s.tail.child_threads,
            s.tail.engine_build,
            s.tail.client_url,
            s.tail.current_time,
        ]),
    }
}
