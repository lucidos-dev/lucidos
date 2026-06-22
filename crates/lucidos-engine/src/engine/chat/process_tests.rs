use super::super::process_helpers::{
    build_system_knowhow_section, build_trigger_knowhow_section, build_trigger_started_event,
    classify_or_fallback, summarize_or_fallback, TriggerContext, APPLY_VERIFY_RULE,
    ENGINE_RESTART_RULE,
};
use super::{build_capture_sections, build_loaded_knowhow_block};
use crate::core::knowhow::KnowhowSummary;
use crate::engine::loaded_knowhow::LoadedKnowhow;
use crate::engine::thread_events::{
    EngineReason, EventChannel, MessageOrigin, ThreadEvent, TriggerInvocation,
};
use crate::engine::ContextRole;
use crate::memory::QueryClassification;
use std::future::pending;

/// Helper to invoke `build_capture_sections` with mostly empty context
/// strings so individual tests only fill in what they care about. Returns
/// the produced sections; tests filter/index by name.
#[allow(clippy::too_many_arguments)]
fn run_build(
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
    loaded: &[LoadedKnowhow],
    resume: &[crate::llm::Message],
) -> Vec<crate::engine::ContextSection> {
    build_capture_sections(
        system_prompt,
        profile_context,
        device_preferences_context,
        file_list_context,
        credentials_context,
        email_accounts_context,
        oauth_context,
        memory_context,
        history_context,
        app_context_section,
        file_context_section,
        url_context_section,
        mcp_stopped_context,
        setup_reminder,
        thread_depth_context,
        user_message,
        loaded,
        resume,
        false,
    )
}

/// After Phase 1 of the trigger-knowhow-discovery refactor, `TriggerContext`
/// no longer carries `knowhow_ids` or `event_payload`. The synthetic
/// `load_knowhow` tool turns the engine fabricated for trigger fires
/// (commits 6beff8f0b / b64df77f9) lived only in the in-memory messages
/// vec on the first turn; resume rebuilt context from events and dropped
/// the recipe body. Trigger threads now discover knowhow the same way
/// chat does — system-prompt list + LLM-driven `load_knowhow` calls. The
/// on_event triggering payload travels in the intent prefix instead of as
/// a fabricated `MessageReceived`. This struct-init guard asserts the
/// shape: a refactor that re-introduces either field will fail to compile
/// here, which is the contract.
#[test]
fn trigger_context_has_no_preload_fields() {
    let _tc = TriggerContext {
        trigger_id: "id".to_string(),
        trigger_name: "name".to_string(),
        slug: "name".to_string(),
        invocation: TriggerInvocation::Schedule,
        go_to_review: false,
        side_effect_grant: vec![],
    };
}

/// Regression for the v5-hash leak: the trigger id passed in (a
/// `config.id` from `/api/v1/triggers`) must propagate verbatim to both
/// `TriggerStarted.trigger_id` and `EngineReason::Scheduler.trigger_id`,
/// otherwise the dropdown filter (which posts the same `config.id`) finds
/// nothing in `thread_summaries.trigger_id`.
#[test]
fn build_trigger_started_event_preserves_config_id_verbatim() {
    let config_id = "5633f3e1-110c-4df4-a6fc-c0df8fd36df4";
    let (event, meta) = build_trigger_started_event(
        config_id,
        "Job Listing Check",
        &TriggerInvocation::Schedule,
        "Run the check.",
        false,
    );
    assert_eq!(meta.channel, Some(EventChannel::Trigger));
    let ThreadEvent::TriggerStarted {
        trigger_id,
        trigger_name,
        origin,
        ..
    } = event
    else {
        panic!("expected TriggerStarted");
    };
    assert_eq!(trigger_id, config_id);
    assert_eq!(trigger_name.as_deref(), Some("Job Listing Check"));
    let MessageOrigin::Engine {
        reason:
            EngineReason::Scheduler {
                trigger_id: origin_id,
                trigger_name: origin_name,
            },
    } = origin.expect("scheduler origin")
    else {
        panic!("expected Engine{{Scheduler}} origin");
    };
    assert_eq!(origin_id, config_id);
    assert_eq!(origin_name.as_deref(), Some("Job Listing Check"));
}

#[tokio::test(start_paused = true)]
async fn summarize_falls_back_when_flash_hangs() {
    let hang = pending::<Result<String, Box<dyn std::error::Error + Send + Sync>>>();
    let result = summarize_or_fallback(hang, 7).await;
    assert_eq!(result, "(7 earlier messages not shown)");
}

#[tokio::test(start_paused = true)]
async fn summarize_falls_back_on_provider_error() {
    let err =
        async { Err::<String, Box<dyn std::error::Error + Send + Sync>>("vertex 503".into()) };
    let result = summarize_or_fallback(err, 4).await;
    assert_eq!(result, "(4 earlier messages not shown)");
}

#[tokio::test(start_paused = true)]
async fn classify_falls_back_when_flash_hangs() {
    let hang = pending::<Result<QueryClassification, Box<dyn std::error::Error + Send + Sync>>>();
    let result = classify_or_fallback(hang).await;
    let default = QueryClassification::default();
    assert_eq!(result.needs_memory, default.needs_memory);
    assert_eq!(result.needs_file_list, default.needs_file_list);
    assert_eq!(result.needs_credentials, default.needs_credentials);
    assert!(result.sub_queries.is_empty());
}

#[tokio::test(start_paused = true)]
async fn classify_falls_back_on_provider_error() {
    let err = async {
        Err::<QueryClassification, Box<dyn std::error::Error + Send + Sync>>("bad json".into())
    };
    let result = classify_or_fallback(err).await;
    assert!(result.needs_memory);
    assert!(result.needs_file_list);
    assert!(result.needs_credentials);
    assert!(result.sub_queries.is_empty());
}

/// Regression: an earlier version of the system prompt said
/// "shipped with the Lucidos engine" with no live-reload clause. The LLM
/// repeatedly inferred "baked into the binary, restart required" and told
/// users to restart after editing system-knowhow files — even though the
/// engine reads them fresh from disk on every chat turn. The section MUST
/// state explicitly that no engine restart is needed and that Apply is the
/// only step required.
#[test]
fn system_knowhow_section_tells_llm_no_restart_needed_after_apply() {
    let summaries = vec![KnowhowSummary {
        id: "building-an-auth-handshake".into(),
        name: "Building an Auth Handshake".into(),
        description: "How to wire external auth.".into(),
    }];
    let section = build_system_knowhow_section(&summaries);
    assert!(
        section.contains("no engine restart"),
        "section must state no restart is needed:\n{section}"
    );
    assert!(
        section.contains("Apply"),
        "section must reference Apply as the activation step:\n{section}"
    );
    assert!(
        section.contains("system-knowhow/building-an-auth-handshake"),
        "section must list the summary entry:\n{section}"
    );
}

#[test]
fn system_knowhow_section_is_empty_when_no_summaries_loaded() {
    assert!(build_system_knowhow_section(&[]).is_empty());
}

/// Per-trigger knowhow listing must scope to the firing trigger's slug —
/// threads of OTHER triggers must not see this trigger's knowhow listed.
/// The id namespace is `triggers/<slug>/<file>` so the LLM's `load_knowhow`
/// call resolves through the same fallback path as bare ids.
#[test]
fn trigger_knowhow_section_scoped_to_firing_trigger() {
    let tmp = tempfile::tempdir().unwrap();
    let triggers_dir = tmp.path().to_path_buf();
    // Drop a knowhow file under nightly-build/knowhow/
    let kh_dir = triggers_dir.join("nightly-build").join("knowhow");
    std::fs::create_dir_all(&kh_dir).unwrap();
    std::fs::write(
        kh_dir.join("orchestration.md"),
        "---\nname: Orchestration\n---\nHow nightly orchestrates each phase.",
    )
    .unwrap();

    // Thread of nightly-build sees the section.
    let section = build_trigger_knowhow_section(&triggers_dir, "nightly-build");
    assert!(
        section.contains("## Trigger Know-how (this trigger only)"),
        "trigger thread must see Trigger Know-how header, got: {section}"
    );
    assert!(
        section.contains("Orchestration"),
        "section must list file's name, got: {section}"
    );
    assert!(
        section.contains("triggers/nightly-build/orchestration"),
        "section must use the trigger-scoped id namespace, got: {section}"
    );

    // Thread of a different trigger sees nothing.
    let other = build_trigger_knowhow_section(&triggers_dir, "some-other-trigger");
    assert!(
        other.is_empty(),
        "other trigger's thread must NOT see this trigger's knowhow, got: {other}"
    );
}

#[test]
fn trigger_knowhow_section_empty_when_no_files() {
    let tmp = tempfile::tempdir().unwrap();
    let triggers_dir = tmp.path().to_path_buf();
    std::fs::create_dir_all(triggers_dir.join("standalone").join("knowhow")).unwrap();
    // Empty knowhow dir → no listing.
    let section = build_trigger_knowhow_section(&triggers_dir, "standalone");
    assert!(section.is_empty());
}

/// Regression for the `Status of Authentication Migration` thread, where
/// the chat LLM signed off with "Etter restart er denne tråden borte —
/// jeg husker ingenting. Kom tilbake i en ny tråd og si …" — telling the
/// user the existing thread was gone and they had to start a NEW one.
/// Threads are event-sourced; the next turn after a restart loads the
/// full history from PostgreSQL. The rule must not claim threads, thread
/// memory, or thread context get wiped.
#[test]
fn engine_restart_rule_does_not_claim_thread_is_wiped_or_lost() {
    let lowered = ENGINE_RESTART_RULE.to_lowercase();
    for forbidden in [
        "wipe thread",
        "wipes thread",
        "wipe the thread",
        "thread is gone",
        "thread is no longer active",
        "thread is wiped",
        "no memory of",
        "have no memory",
        "start a new thread",
        "new thread to continue",
        "hard cut-off",
        "hard cutoff",
    ] {
        assert!(
                !lowered.contains(forbidden),
                "ENGINE_RESTART_RULE must not contain `{forbidden}` — threads survive restart and the next turn reloads history:\n{ENGINE_RESTART_RULE}"
            );
    }
}

/// The rule must positively state what actually survives (the thread and
/// its history) so the LLM sends users back to the same thread instead of
/// a fresh one.
#[test]
fn engine_restart_rule_says_thread_survives_and_history_reloads() {
    let lowered = ENGINE_RESTART_RULE.to_lowercase();
    assert!(
        lowered.contains("survives"),
        "rule must say the thread survives a restart:\n{ENGINE_RESTART_RULE}"
    );
    assert!(
        lowered.contains("history"),
        "rule must mention that history is preserved:\n{ENGINE_RESTART_RULE}"
    );
    assert!(
        lowered.contains("this thread") || lowered.contains("same thread"),
        "rule must direct the user back to the existing thread:\n{ENGINE_RESTART_RULE}"
    );
}

/// The fix must keep the original guidance against promising post-restart
/// continuation — that part of the old rule was correct and is the actual
/// trap we want the LLM to avoid.
#[test]
fn engine_restart_rule_still_blocks_post_restart_promises() {
    assert!(
        ENGINE_RESTART_RULE.contains("after the restart"),
        "rule must still ban `after the restart` promises:\n{ENGINE_RESTART_RULE}"
    );
    assert!(
        ENGINE_RESTART_RULE.contains("check back later"),
        "rule must still ban `check back later` promises:\n{ENGINE_RESTART_RULE}"
    );
}

/// The apply/verify rule must keep the chat agent from bouncing yes/no
/// "did you apply it? / did you restart?" confirmations at the user — the
/// font-fix-session failure pattern. It must (1) name both self-service tools,
/// (2) ban the two confirmation questions, (3) tell the agent to probe the
/// served asset instead of asking, (4) carry the workspace-prefixed-route
/// gateway gotcha, and (5) ban the "does it look right now?" closer.
#[test]
fn apply_verify_rule_tells_agent_to_act_and_verify_not_ask() {
    let lowered = APPLY_VERIFY_RULE.to_lowercase();
    assert!(
        APPLY_VERIFY_RULE.contains("list_changes") && APPLY_VERIFY_RULE.contains("apply_change"),
        "rule must name both self-service change tools:\n{APPLY_VERIFY_RULE}"
    );
    assert!(
        lowered.contains("have you applied it") && lowered.contains("did you restart"),
        "rule must explicitly ban the two confirmation questions:\n{APPLY_VERIFY_RULE}"
    );
    assert!(
        lowered.contains("cannot restart the engine"),
        "rule must state the agent cannot restart the engine (only the user can):\n{APPLY_VERIFY_RULE}"
    );
    assert!(
        lowered.contains("probing the served asset")
            || lowered.contains("probe the served asset")
            || lowered.contains("probe served assets"),
        "rule must tell the agent to verify by probing the served asset:\n{APPLY_VERIFY_RULE}"
    );
    assert!(
        APPLY_VERIFY_RULE.contains("/<workspace>/api/v1/")
            && lowered.contains("unknown workspace 'api'"),
        "rule must record the workspace-prefixed-route gateway gotcha:\n{APPLY_VERIFY_RULE}"
    );
    assert!(
        lowered.contains("does it look right now") || lowered.contains("does it match"),
        "rule must ban the post-apply confirmation-question closer:\n{APPLY_VERIFY_RULE}"
    );
}

/// The fix is a NARROW carve-out, not a ban on `ask_user_question` (the user
/// explicitly flagged this during the work). The rule must reference the tool
/// to scope the carve-out and must NOT blanket-ban it, so the LLM doesn't
/// over-correct into never asking genuine next-step choices.
#[test]
fn apply_verify_rule_does_not_disable_ask_user_question() {
    assert!(
        APPLY_VERIFY_RULE.contains("ask_user_question"),
        "rule must reference ask_user_question to scope the carve-out:\n{APPLY_VERIFY_RULE}"
    );
    let lowered = APPLY_VERIFY_RULE.to_lowercase();
    for forbidden in [
        "never use ask_user_question",
        "do not use ask_user_question",
        "stop using ask_user_question",
    ] {
        assert!(
            !lowered.contains(forbidden),
            "rule must not blanket-ban the question tool (`{forbidden}`):\n{APPLY_VERIFY_RULE}"
        );
    }
}

/// The rule must reinforce that the user works on Lucidos constantly and knows
/// the apply/restart/reload dance — so the agent doesn't re-explain it (the
/// other half of "don't interrogate the user").
#[test]
fn apply_verify_rule_reinforces_user_knows_the_dance() {
    let lowered = APPLY_VERIFY_RULE.to_lowercase();
    assert!(
        lowered.contains("knows the apply/restart/reload dance")
            && lowered.contains("do not re-explain it"),
        "rule must reinforce that the user knows the dance and must not be re-taught it:\n{APPLY_VERIFY_RULE}"
    );
}

/// Phase 3.1: every turn after the first must inject a `[LOADED KNOWHOW]`
/// block listing the docs `load_knowhow` brought in earlier in the thread.
/// The block lives in the user message so the LLM sees it on every turn —
/// Phase 4 stubs the resume tool blocks for `load_knowhow` so the same body
/// isn't sent twice.
#[test]
fn loaded_knowhow_block_emits_doc_bodies_verbatim() {
    // doc.body is already the formatted [SYSTEM-KNOWHOW: <name>] block
    // produced by core::knowhow::load_one_knowhow_section. The function
    // must push it verbatim — re-wrapping would double-nest markers and
    // mismatch id-vs-name (id is the file id, name is the doc's display
    // name from frontmatter).
    let docs = vec![
        LoadedKnowhow {
            id: "alpha".into(),
            body: "[SYSTEM-KNOWHOW: Alpha Doc]\nBody A\n[END SYSTEM-KNOWHOW]".into(),
        },
        LoadedKnowhow {
            id: "beta".into(),
            body: "[KNOW-HOW: Beta Doc]\nBody B\n[END KNOW-HOW]".into(),
        },
    ];
    let s = build_loaded_knowhow_block(&docs).expect("non-empty docs produce a block");
    assert!(
        s.starts_with("[LOADED KNOWHOW]"),
        "block must open with the [LOADED KNOWHOW] marker:\n{s}"
    );
    assert!(
        s.ends_with("[END LOADED KNOWHOW]"),
        "block must close with the [END LOADED KNOWHOW] marker:\n{s}"
    );
    // Bodies pass through verbatim — no re-wrapping with [SYSTEM-KNOWHOW: <id>].
    assert!(s.contains("[SYSTEM-KNOWHOW: Alpha Doc]"));
    assert!(s.contains("[KNOW-HOW: Beta Doc]"));
    assert!(s.contains("Body A"));
    assert!(s.contains("Body B"));
    // The id must NOT appear as an outer marker — that would mean re-wrapping.
    assert!(
        !s.contains("[SYSTEM-KNOWHOW: alpha]"),
        "must not re-wrap with id as outer marker:\n{s}"
    );
    assert!(
        !s.contains("[SYSTEM-KNOWHOW: beta]"),
        "must not re-wrap with id as outer marker:\n{s}"
    );
    // Header guidance is present so the LLM knows how to treat the section.
    assert!(
        s.contains("Treat their guidance as authoritative"),
        "header guidance missing from block:\n{s}"
    );
}

/// Empty loaded set must not produce a section — pushing an empty-string part
/// would put a stray double-newline pair into the user message.
#[test]
fn loaded_knowhow_block_returns_none_for_empty_docs() {
    assert!(build_loaded_knowhow_block(&[]).is_none());
}

/// Phase 5.1: every base section must carry the API role + inner-tier
/// group the viewer needs to render the new two-layer grouping. Empty
/// content is filtered out so this test fills every slot to exercise the
/// full `labeled` array.
#[test]
fn build_capture_sections_tags_existing_sections_with_role_and_group() {
    let sections = run_build(
        "sys",
        "profile",
        "device-prefs",
        "files",
        "creds",
        "emails",
        "oauth",
        "memory",
        "history",
        "app",
        "file",
        "url",
        "mcp-stopped",
        "setup-reminder",
        "depth",
        "user msg",
        &[],
        &[],
    );
    let by_name = |n: &str| {
        sections
            .iter()
            .find(|s| s.name == n)
            .cloned()
            .unwrap_or_else(|| panic!("section {n} missing"))
    };

    assert_eq!(by_name("System Instructions").role, ContextRole::System);
    assert_eq!(by_name("System Instructions").group, None);

    assert_eq!(by_name("User Profile").role, ContextRole::User);
    assert_eq!(
        by_name("User Profile").group,
        Some("Identity & profile".to_string())
    );
    assert_eq!(
        by_name("Device & Preferences").group,
        Some("Identity & profile".to_string())
    );

    // Spot-check one row from every inner tier the viewer renders.
    assert_eq!(
        by_name("File List").group,
        Some("Workspace inventory".to_string())
    );
    assert_eq!(
        by_name("Credentials").group,
        Some("Workspace inventory".to_string())
    );
    assert_eq!(
        by_name("Email Accounts").group,
        Some("Workspace inventory".to_string())
    );
    assert_eq!(
        by_name("OAuth").group,
        Some("Workspace inventory".to_string())
    );
    assert_eq!(
        by_name("Long-term Memory").group,
        Some("Memory & history".to_string())
    );
    assert_eq!(
        by_name("Conversation History").group,
        Some("Memory & history".to_string())
    );
    assert_eq!(
        by_name("App Context").group,
        Some("Active context".to_string())
    );
    assert_eq!(
        by_name("File Context").group,
        Some("Active context".to_string())
    );
    assert_eq!(
        by_name("URL Context").group,
        Some("Active context".to_string())
    );
    assert_eq!(
        by_name("Stopped MCP Servers").group,
        Some("System notices".to_string())
    );
    assert_eq!(
        by_name("Setup Reminder").group,
        Some("System notices".to_string())
    );
    assert_eq!(
        by_name("Thread Depth").group,
        Some("System notices".to_string())
    );
    assert_eq!(
        by_name("User Message").group,
        Some("The request".to_string())
    );
}

/// Phase 5.1: empty bodies must be filtered out — the viewer must not
/// render zero-char rows for sections that don't apply this turn.
#[test]
fn build_capture_sections_filters_empty_sections() {
    let sections = run_build(
        "sys", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "user msg", &[], &[],
    );
    let names: Vec<_> = sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["System Instructions", "User Message"]);
}

/// Phase 5.2: each loaded knowhow doc gets its own collapsible row under
/// the "Loaded knowhow" inner group. Char count reflects the body so the
/// viewer's budget bar stays honest even when capture_body is false.
#[test]
fn build_capture_sections_emits_one_row_per_loaded_knowhow_doc() {
    let docs = vec![
        LoadedKnowhow {
            id: "doc-a".into(),
            body: "BODY A".into(),
        },
        LoadedKnowhow {
            id: "doc-b".into(),
            body: "BODY BBBB".into(),
        },
    ];
    let sections = run_build(
        "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "user msg", &docs, &[],
    );

    let knowhow: Vec<_> = sections
        .iter()
        .filter(|s| s.group.as_deref() == Some("Loaded knowhow"))
        .collect();
    assert_eq!(knowhow.len(), 2, "one row per loaded doc");
    assert_eq!(knowhow[0].name, "knowhow: doc-a");
    assert_eq!(knowhow[1].name, "knowhow: doc-b");
    assert!(knowhow.iter().all(|s| s.role == ContextRole::User));
    // char_count is real (capture_body is false so content is None, but
    // the viewer's budget bar reads char_count, not the body).
    assert_eq!(knowhow[0].char_count, "BODY A".chars().count());
    assert_eq!(knowhow[1].char_count, "BODY BBBB".chars().count());
    assert!(knowhow.iter().all(|s| s.content.is_none()));
}

/// Phase 5.3: each `(ToolUse, ToolResult)` pair from `resume_tool_blocks`
/// becomes its own row under the `PriorMessage` role. Tool name + JSON
/// args preview live in the row name so the viewer can show what call the
/// pair represents without expanding it.
#[test]
fn build_capture_sections_emits_one_row_per_resume_tool_pair() {
    use crate::llm::{ContentBlock, Message, MessageContent};
    let resume = vec![
        Message {
            role: "assistant".into(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "id1".into(),
                name: "query_events".into(),
                input: serde_json::json!({"limit": 5}),
                thought_signature: None,
            }]),
        },
        Message {
            role: "user".into(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "id1".into(),
                content: "[]".into(),
            }]),
        },
        Message {
            role: "assistant".into(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "id2".into(),
                name: "load_knowhow".into(),
                input: serde_json::json!({"id": "x"}),
                thought_signature: None,
            }]),
        },
        Message {
            role: "user".into(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "id2".into(),
                content: "BODY".into(),
            }]),
        },
    ];
    let sections = run_build(
        "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "user msg", &[], &resume,
    );

    let prior: Vec<_> = sections
        .iter()
        .filter(|s| s.role == ContextRole::PriorMessage)
        .collect();
    assert_eq!(prior.len(), 2, "one row per resume pair");
    assert!(prior.iter().any(|s| s.name == "ToolUse: query_events"));
    assert!(prior.iter().any(|s| s.name == "ToolUse: load_knowhow"));
    assert!(prior.iter().all(|s| s.group.is_none()));
    // capture_body=false so bodies are dropped; char_count still reflects
    // the assembled "ToolUse: …\n\nToolResult:\n…" body so the viewer's
    // prior-messages budget stays accurate.
    assert!(prior.iter().all(|s| s.content.is_none()));
    assert!(prior.iter().all(|s| s.char_count > 0));
}

#[test]
fn build_capture_sections_includes_device_preferences_context() {
    let sections = run_build(
        "",
        "",
        "[USER DEVICE & PREFERENCES]\n- theme: light\n[END USER DEVICE & PREFERENCES]",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "user msg",
        &[],
        &[],
    );

    let section = sections
        .iter()
        .find(|s| s.name == "Device & Preferences")
        .expect("device/preferences section must be captured");
    assert_eq!(section.group.as_deref(), Some("Identity & profile"));
    assert_eq!(section.role, ContextRole::User);
    assert!(section.char_count > 0);
}

/// Capturing a body honors the `capture_body` flag: rows get full bodies
/// (truncated at SECTION_PERSIST_MAX) when on, `None` when off. The
/// truncation cap itself is exercised by the existing types::tests; this
/// test just guards the on/off wiring through the new free function.
#[test]
fn build_capture_sections_honors_capture_body_flag() {
    let docs = vec![LoadedKnowhow {
        id: "doc".into(),
        body: "BODY".into(),
    }];
    let on = build_capture_sections(
        "sys", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "user", &docs, &[], true,
    );
    let off = build_capture_sections(
        "sys", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "user", &docs, &[], false,
    );
    let sys_on = on.iter().find(|s| s.name == "System Instructions").unwrap();
    let sys_off = off.iter().find(|s| s.name == "System Instructions").unwrap();
    assert_eq!(sys_on.content.as_deref(), Some("sys"));
    assert!(sys_off.content.is_none());
    let kh_on = on.iter().find(|s| s.name == "knowhow: doc").unwrap();
    let kh_off = off.iter().find(|s| s.name == "knowhow: doc").unwrap();
    assert_eq!(kh_on.content.as_deref(), Some("BODY"));
    assert!(kh_off.content.is_none());
}

/// The chat-agent prompt must nudge use of `ask_user_question` for
/// choice-shaped questions (yes/no, A vs B, "what next?" follow-up menus)
/// — symmetric with the CC-side `chat_style_prompts_nudge_use_of_ask_user_question`
/// test in `agent_session::prompts`. Without this nudge the chat agent
/// reads ACTION FIRST as a blanket "don't ask the user anything" rule and
/// falls back to plaintext bullet lists for next-step alternatives — the
/// exact failure pattern the rule was added to prevent.
#[test]
fn chat_prompt_nudges_use_of_ask_user_question() {
    let rule = super::ASK_USER_QUESTION_RULE;
    assert!(
        rule.contains("ask_user_question"),
        "chat ASK_USER_QUESTION_RULE must name the tool (lowercase snake_case — \
         that's the actual chat-side tool name)",
    );
    assert!(
        rule.contains("2-4 discrete answers"),
        "chat ASK_USER_QUESTION_RULE must pin the trigger criteria (2-4 \
         discrete answers); softer phrasing let the LLM keep slipping into \
         plaintext for genuine choice-shaped questions",
    );
    assert!(
        rule.contains("mid-stream"),
        "chat ASK_USER_QUESTION_RULE must keep the mid-stream concept — \
         end-only examples let the chat agent slip plaintext yes/no questions \
         into the middle of long answers (mirrors the CC-side assertion in \
         `chat_style_prompts_nudge_use_of_ask_user_question`)",
    );
    assert!(
        rule.contains("what next"),
        "chat ASK_USER_QUESTION_RULE must include the concrete \"what next?\" \
         follow-up-menu example — that's the exact failure pattern in personal \
         workspace threads where the agent emits markdown bullets instead of \
         buttons",
    );
    assert!(
        rule.contains("ACTION FIRST"),
        "chat ASK_USER_QUESTION_RULE must explicitly carve itself out of \
         ACTION FIRST — without the carve-out the two rules fight and the \
         LLM defaults to silence on next-step alternatives",
    );
    assert!(
        rule.contains("NEVER parallel-call"),
        "chat ASK_USER_QUESTION_RULE must forbid parallel-calling \
         `ask_user_question` alongside other tools (see the parallel CC rule \
         in `agent_session::prompts::ASK_USER_QUESTION_RULE`)",
    );
    // Three observed Opus 4.7 leaks in personal (4bc99ec8, ba1b4ef1,
    // 66638f55) emitted `<ask_user_question>…</ask_user_question>` as
    // literal assistant text instead of a tool call. The rule must name
    // that exact failure mode so the next prompt edit can't silently drop
    // the anti-pattern callout.
    assert!(
        rule.contains("<ask_user_question"),
        "chat ASK_USER_QUESTION_RULE must show the forbidden literal tag \
         (`<ask_user_question`) — the model has emitted wrapper-tag text \
         instead of a real tool call multiple times on Opus 4.7 max effort, \
         and only naming the exact tag string makes the rule self-evident \
         to the model when it next debates the format",
    );
    assert!(
        rule.contains("INVOKE AS A TOOL CALL ONLY") || rule.contains("never write the tag as text"),
        "chat ASK_USER_QUESTION_RULE must carry the anti-inline-tag clause \
         (header `INVOKE AS A TOOL CALL ONLY — NEVER WRITE THE TAG AS TEXT`) \
         — without it the rule only nudges *when* to ask, not *how*, and \
         Opus 4.7 keeps inventing `<ask_user_question>` wrappers",
    );
}

// --- FreeText answer eligibility (child-completion vs. human follow-up) ---

use super::run::message_can_answer_pending_question;
use super::run::resolve_route_overrides;
use crate::core::{PreferenceStore, PREF_CHAT_MODEL, PREF_CHAT_REASONING_EFFORT};
use crate::engine::thread_events::ActorMode;
use crate::test_support::{setup_test_db, teardown_test_db};

/// Coding-agent requests use the same HTTP `reasoning_effort` field for an
/// explicit agent pick, but an omitted field means "fall through to agent
/// settings/defaults". It must not be filled from the Lucidos chat preference:
/// Codex and Claude Code have their own model/effort configuration surfaces.
#[tokio::test]
async fn coding_agent_route_does_not_inherit_chat_model_or_effort_defaults() {
    let (pool, db_name) = setup_test_db().await;
    PreferenceStore::set(&pool, PREF_CHAT_MODEL, "gemini-3.5-flash")
        .await
        .unwrap();
    PreferenceStore::set(&pool, PREF_CHAT_REASONING_EFFORT, "max")
        .await
        .unwrap();

    let (model, effort) =
        resolve_route_overrides(&pool, Some(true), Some("claude-opus-4-8[1m]"), None).await;

    assert_eq!(model, None);
    assert_eq!(effort, None);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn coding_agent_route_preserves_explicit_agent_effort_pick() {
    let (pool, db_name) = setup_test_db().await;
    PreferenceStore::set(&pool, PREF_CHAT_REASONING_EFFORT, "high")
        .await
        .unwrap();

    let (model, effort) = resolve_route_overrides(&pool, Some(true), None, Some("xhigh")).await;

    assert_eq!(model, None);
    assert_eq!(effort.as_deref(), Some("xhigh"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn chat_route_still_inherits_chat_model_and_effort_defaults() {
    let (pool, db_name) = setup_test_db().await;
    PreferenceStore::set(&pool, PREF_CHAT_MODEL, "claude-opus-4-8[1m]")
        .await
        .unwrap();
    PreferenceStore::set(&pool, PREF_CHAT_REASONING_EFFORT, "high")
        .await
        .unwrap();

    let (model, effort) = resolve_route_overrides(&pool, None, None, None).await;

    assert_eq!(model.as_deref(), Some("claude-opus-4-8[1m]"));
    assert_eq!(effort.as_deref(), Some("high"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A genuine human follow-up typed on a thread with an open question is the
/// one and only case that may be consumed as a FreeText answer.
#[test]
fn human_follow_up_can_answer_pending_question() {
    assert!(message_can_answer_pending_question(
        false,
        "repo is private, not public",
        ActorMode::Human,
    ));
}

/// Regression: an agent-driven child-completion wake (the `[CHILD THREAD
/// COMPLETED] …` block fed through `notify_parent_of_child_completion` with
/// `ActorMode::Agent`) must NOT be eligible to answer the parent's open
/// question. Before the `mode == Human` guard it was, producing a bogus
/// `UserQuestionAnswered { FreeText }` stamped with a `thread_link`/`child`
/// actor and silently consuming the user's question. It must instead fall
/// through to the injection fast-path (queued as `WakeFromChild`).
#[test]
fn child_completion_wake_cannot_answer_pending_question() {
    assert!(!message_can_answer_pending_question(
        false,
        "[CHILD THREAD COMPLETED] 59328631… success\nSession summary…",
        ActorMode::Agent,
    ));
}

/// Engine-driven re-entries (recovery notes, scheduler) are likewise never the
/// user's answer.
#[test]
fn engine_driven_message_cannot_answer_pending_question() {
    assert!(!message_can_answer_pending_question(
        false,
        "engine recovery note",
        ActorMode::Engine,
    ));
}

/// A new thread has no pending question to answer, and an empty message can't
/// be an answer regardless of who authored it.
#[test]
fn new_thread_or_empty_message_cannot_answer_pending_question() {
    assert!(!message_can_answer_pending_question(
        true,
        "first message on a brand-new thread",
        ActorMode::Human,
    ));
    assert!(!message_can_answer_pending_question(false, "", ActorMode::Human));
}
