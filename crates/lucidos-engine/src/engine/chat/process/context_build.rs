//! Free helpers for assembling the per-turn context snapshot: the extraction
//! topic summary, the `[LOADED KNOWHOW]` user-message block, and the
//! `ContextCaptured` section list rendered by the LLM Context Viewer. Pure
//! functions — no engine state — so they unit-test without standing up an
//! engine (see `process_tests.rs`).

use crate::engine::loaded_knowhow::LoadedKnowhow;

/// Cap on per-section body persistence in `ContextCaptured` events. Both
/// sizes a section carries stay real; the `content` body is truncated
/// head/tail to this many characters so a 100 KB system prompt doesn't
/// bloat every captured row.
///
/// Every section built here is a true count, so its `budget_delta_chars` and
/// its `content_chars` agree. `Conversation` is the one row where they part,
/// and the agentic loop builds that one.
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
/// `capture_body` mirrors the workspace's `capture_context` preference.
/// When `false`, every row's `content` is `None` and only its name and its
/// two sizes are persisted. The viewer still renders the budget breakdown,
/// and event-row storage is not billed for full bodies.
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
    tail: &super::turn_tail::TurnTail<'_>,
    loaded_knowhow_docs: &[LoadedKnowhow],
    resume_tool_blocks: &[crate::llm::Message],
    capture_body: bool,
) -> Vec<crate::engine::ContextSection> {
    use crate::engine::context::truncate_head_tail;
    use crate::engine::{ContextRole, ContextSection};

    // Body capped at SECTION_PERSIST_MAX with head/tail preservation so a
    // 100 KB system prompt doesn't bloat every captured event row. Both sizes
    // remain the true length so the viewer's budget bar stays honest.
    //
    // The eval lifts that cap (ADR 0110), because a benchmark of context
    // handling has to be able to read what was in the context.
    let cap = crate::engine::eval_capture::body_cap(SECTION_PERSIST_MAX);
    let truncate = |content: &str| -> Option<String> {
        if !capture_body {
            return None;
        }
        Some(match cap {
            Some(cap) if content.len() > cap => truncate_head_tail(content, cap),
            _ => content.to_string(),
        })
    };

    let labeled: [LabeledSection; 19] = [
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
        // Both names come from `context_mode`, which is also where the eval's
        // manipulation check reads them. The name IS the contract: that check
        // asks whether these two rows are present on a round, so a rename here
        // would silently pass a lean arm that dropped nothing.
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
        // Last three, mirroring where they ride on the wire. These are the
        // sections that move every turn, and they are here rather than in the
        // system prompt for exactly that reason (`super::turn_tail`).
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
            // Nothing else counts these bytes, so the delta is the section's
            // own size and the two agree.
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

    // Phase 5.2: surface every loaded knowhow doc as its own collapsible
    // row so the viewer shows the body in a dedicated section instead of
    // buried inside the User Message blob. Both sizes reflect the full
    // body so the budget bar stays honest even when capture_body is off.
    for doc in loaded_knowhow_docs {
        let chars = doc.body.chars().count();
        capture_sections.push(ContextSection {
            name: format!("knowhow: {}", doc.id),
            content: truncate(&doc.body),
            budget_delta_chars: chars,
            content_chars: Some(chars),
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
        // The same cap and the same gate as the static sections above. A
        // resumed tool pair is a prior turn's work, which is exactly what a
        // two-turn task's replay needs to show.
        let content = if capture_body {
            Some(match cap {
                Some(cap) if pair_body.len() > cap => truncate_head_tail(&pair_body, cap),
                _ => pair_body,
            })
        } else {
            None
        };
        capture_sections.push(ContextSection {
            name: format!("ToolUse: {}", tool_name),
            content,
            budget_delta_chars: pair_chars,
            content_chars: Some(pair_chars),
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
