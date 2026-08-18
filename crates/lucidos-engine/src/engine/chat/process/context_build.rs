//! Free helpers for assembling the per-turn context snapshot: the extraction
//! topic summary, the `[LOADED KNOWHOW]` user-message block, and the
//! `ContextCaptured` section list rendered by the LLM Context Viewer. Pure
//! functions — no engine state — so they unit-test without standing up an
//! engine (see `process_tests.rs`).

use crate::engine::loaded_knowhow::LoadedKnowhow;

/// Cap on per-section body persistence in `ContextCaptured` events. The
/// section's `char_count` is always real; the `content` body is truncated
/// head/tail to this many characters so a 100 KB system prompt doesn't
/// bloat every captured row.
const SECTION_PERSIST_MAX: usize = 8_000;

/// Build the 500-char extraction summary fed to the memory extractor: pipe-
/// joined user turns from `prior` followed by the current `user_message`.
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

/// Per-section input to [`build_capture_sections`]. Carries the section
/// name (rendered as the row label in the LLM Context Viewer), its body,
/// the API role bucket the section ships under, and an optional inner
/// "tier" group used by the viewer to nest sections within a role.
///
/// The inner-group taxonomy mirrors the `[USER MESSAGE]` ordering in
/// `process_message_with_steps_internal` and the Phase 5 design diagram in
/// `docs/plans/2026-05-15-loaded-knowhow-and-context-viewer-reorg.md`:
/// Identity → Workspace inventory → Memory & history → Loaded knowhow →
/// Active context → System notices → The request.
struct LabeledSection<'a> {
    name: &'static str,
    content: &'a str,
    role: crate::engine::ContextRole,
    group: Option<&'static str>,
}

/// Build the `Vec<ContextSection>` snapshot persisted on every
/// `ContextCaptured` event and rendered by the LLM Context Viewer.
///
/// Three groups of rows are emitted in this order:
///
/// 1. **Tagged base sections** — every per-turn context block (system
///    prompt, profile, history, active context, request, …) tagged with
///    its API role (`System` / `User`) and inner tier so the viewer can
///    nest them. Empty bodies are filtered out.
/// 2. **Per-loaded-knowhow rows** — one row per doc in the per-thread
///    loaded set under the "Loaded knowhow" inner group. Mirrors what the
///    `[LOADED KNOWHOW]` user-message block actually contains so the
///    viewer shows the body in its own collapsible row instead of buried
///    inside the User Message blob.
/// 3. **Per-resume-tool-pair rows** — one row per `(ToolUse, ToolResult)`
///    pair from `resume_tool_blocks` under the `PriorMessage` role. Before
///    Phase 5.3 these were invisible in the viewer; surfacing them
///    matches the actual API request shape (system / prior messages /
///    user).
///
/// `capture_body` mirrors the workspace's `capture_context` preference:
/// when `false`, every row's `content` is `None` and only `name` +
/// `char_count` are persisted, so the viewer can render the budget
/// breakdown without billing event-row storage for full bodies.
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
    mcp_stopped_context: &str,
    setup_reminder: &str,
    thread_depth_context: &str,
    user_message: &str,
    current_time_block: &str,
    loaded_knowhow_docs: &[LoadedKnowhow],
    resume_tool_blocks: &[crate::llm::Message],
    capture_body: bool,
) -> Vec<crate::engine::ContextSection> {
    use crate::engine::context::truncate_head_tail;
    use crate::engine::{ContextRole, ContextSection};

    // Body capped at SECTION_PERSIST_MAX with head/tail preservation so a
    // 100 KB system prompt doesn't bloat every captured event row. char_count
    // remains the true length so the viewer's budget bar stays honest.
    let truncate = |content: &str| -> Option<String> {
        if !capture_body {
            return None;
        }
        Some(if content.len() > SECTION_PERSIST_MAX {
            truncate_head_tail(content, SECTION_PERSIST_MAX)
        } else {
            content.to_string()
        })
    };

    let labeled: [LabeledSection; 17] = [
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
            name: "Long-term Memory",
            content: memory_context,
            role: ContextRole::User,
            group: Some("Memory & history"),
        },
        LabeledSection {
            name: "Conversation History",
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
            name: "Stopped MCP Servers",
            content: mcp_stopped_context,
            role: ContextRole::User,
            group: Some("System notices"),
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
            name: "User Message",
            content: user_message,
            role: ContextRole::User,
            group: Some("The request"),
        },
        // Last, mirroring where it rides on the wire. It is the one section
        // that moves every turn, and it is here rather than in the system
        // prompt for exactly that reason (`super::turn_clock`).
        LabeledSection {
            name: "Current Time",
            content: current_time_block,
            role: ContextRole::User,
            group: Some("The request"),
        },
    ];

    let mut capture_sections: Vec<ContextSection> = labeled
        .into_iter()
        .filter(|ls| !ls.content.is_empty())
        .map(|ls| ContextSection {
            name: ls.name.to_string(),
            content: truncate(ls.content),
            char_count: ls.content.chars().count(),
            role: ls.role,
            group: ls.group.map(|s| s.to_string()),
        })
        .collect();

    // Phase 5.2: surface every loaded knowhow doc as its own collapsible
    // row so the viewer shows the body in a dedicated section instead of
    // buried inside the User Message blob. Char count reflects the full
    // body so the budget bar stays honest even when capture_body is off.
    for doc in loaded_knowhow_docs {
        capture_sections.push(ContextSection {
            name: format!("knowhow: {}", doc.id),
            content: truncate(&doc.body),
            char_count: doc.body.chars().count(),
            role: ContextRole::User,
            group: Some("Loaded knowhow".to_string()),
        });
    }

    // Phase 5.3: surface each (ToolUse, ToolResult) pair from
    // `resume_tool_blocks` as its own row under the PriorMessage role —
    // before this they were invisible in the viewer even though they
    // travel verbatim with every request and consume real budget.
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
        let content = if capture_body {
            Some(if pair_body.len() > SECTION_PERSIST_MAX {
                truncate_head_tail(&pair_body, SECTION_PERSIST_MAX)
            } else {
                pair_body
            })
        } else {
            None
        };
        capture_sections.push(ContextSection {
            name: format!("ToolUse: {}", tool_name),
            content,
            char_count: pair_chars,
            role: ContextRole::PriorMessage,
            group: None,
        });
    }

    capture_sections
}

/// Build the per-turn `[LOADED KNOWHOW]` block injected into the user
/// message — the source-of-truth location for loaded knowhow text after
/// the first turn. Returns `None` for an empty loaded set so callers can
/// skip pushing an empty section.
///
/// `doc.body` is already the formatted `[SYSTEM-KNOWHOW: <name>]` /
/// `[KNOW-HOW: <name>]` block produced by
/// `core::knowhow::load_one_knowhow_section` — pushed verbatim so the LLM
/// sees the same markers it did on first-turn inline returns. Re-wrapping
/// here would nest markers and mismatch id-vs-name.
pub(crate) fn build_loaded_knowhow_block(docs: &[LoadedKnowhow]) -> Option<String> {
    if docs.is_empty() {
        return None;
    }
    let mut s = String::from(
        "[LOADED KNOWHOW]\nThe following knowhow docs are loaded for this thread. Treat their guidance as authoritative.\n",
    );
    for doc in docs {
        s.push('\n');
        s.push_str(&doc.body);
    }
    s.push_str("\n[END LOADED KNOWHOW]");
    Some(s)
}
