use super::super::process_helpers::{
    build_removed_knowhow_section, build_system_knowhow_section, build_trigger_knowhow_section,
    build_trigger_started_event, classify_or_fallback, forced_classification, summarize_or_none,
    summary_is_too_thin, TriggerContext, TriggerFire, TriggerTurn, APPLY_VERIFY_DEV_ADDENDUM,
    APPLY_VERIFY_RULE, ENGINE_RESTART_RULE, FORCE_QUERY_CLASSIFICATION_ENV,
};
use super::{
    assemble_turn_text, build_capture_sections, build_knowhow_retention_block,
    build_loaded_knowhow_block, TurnSections, TurnTail,
};
use crate::core::knowhow::KnowhowSummary;
use crate::engine::loaded_knowhow::LoadedKnowhow;
use crate::engine::thread_events::{
    EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent, TriggerInvocation,
};
use crate::engine::ContextRole;
use crate::memory::QueryClassification;
use std::future::pending;

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
    mcp_servers_context: &str,
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
        mcp_servers_context,
        "",
        "",
        setup_reminder,
        thread_depth_context,
        "",
        user_message,
        &TurnTail {
            child_threads: "",
            engine_build: "",
            client_url: "",
            current_time: CLOCK_BLOCK,
        },
        loaded,
        resume,
    )
}

const CLOCK_BLOCK: &str =
    "[CURRENT TIME]\nNow: Monday, August 17, 2026 at 15:02 Europe/Oslo (UTC+2).\n\
     The same instant in UTC: Monday, August 17, 2026 at 13:02.\n[END CURRENT TIME]";

#[test]
fn trigger_context_has_no_preload_fields() {
    let _tc = trigger_context("id", "name");
}

fn trigger_context(trigger_id: &str, trigger_name: &str) -> TriggerContext {
    trigger_fire_context(trigger_id, trigger_name, |_| {})
}

fn trigger_fire_context(
    trigger_id: &str,
    trigger_name: &str,
    shape: impl FnOnce(&mut TriggerFire),
) -> TriggerContext {
    let mut fire = TriggerFire {
        retry_of: None,
        invocation: TriggerInvocation::Schedule,
        queue_entry_id: uuid::Uuid::new_v4(),
        continue_key: None,
        actor_email: None,
    };
    shape(&mut fire);
    TriggerContext {
        trigger_id: trigger_id.to_string(),
        trigger_name: trigger_name.to_string(),
        slug: trigger_name.to_string(),
        go_to_review: false,
        mcp_servers: vec![],
        turn: TriggerTurn::Fire(fire),
    }
}

fn started(
    tc: &TriggerContext,
    user_message: &str,
    selection: &crate::core::ResolvedModelSelection,
) -> (ThreadEvent, EventMeta) {
    build_trigger_started_event(tc, user_message, selection).expect("a fire opens its turn")
}

#[test]
fn a_resumed_trigger_turn_builds_no_starter_event() {
    let tc = TriggerContext {
        turn: TriggerTurn::Resume,
        ..trigger_context("t-1", "Ticket intake")
    };
    assert!(build_trigger_started_event(&tc, "Carry on.", &sel(None, None, None)).is_none());
}

#[test]
fn build_trigger_started_event_records_the_actor_email() {
    let tc = trigger_fire_context("t-1", "Ticket intake", |fire| {
        fire.actor_email = Some("sam@example.com".into());
    });
    let (event, _meta) = started(&tc, "Triage.", &sel(None, None, None));
    let ThreadEvent::TriggerStarted {
        origin:
            Some(MessageOrigin::Engine {
                reason: EngineReason::Scheduler { actor_email, .. },
            }),
        ..
    } = event
    else {
        panic!("expected a TriggerStarted with a scheduler origin");
    };
    assert_eq!(actor_email.as_deref(), Some("sam@example.com"));
}

#[test]
fn build_trigger_started_event_preserves_config_id_verbatim() {
    let config_id = "5633f3e1-110c-4df4-a6fc-c0df8fd36df4";
    let (event, meta) = started(
        &trigger_context(config_id, "Job Listing Check"),
        "Run the check.",
        &sel(None, None, None),
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
                ..
            },
    } = origin.expect("scheduler origin")
    else {
        panic!("expected Engine{{Scheduler}} origin");
    };
    assert_eq!(origin_id, config_id);
    assert_eq!(origin_name.as_deref(), Some("Job Listing Check"));
}

#[test]
fn build_trigger_started_event_records_the_resolved_model_and_effort() {
    let (event, _meta) = started(
        &trigger_context("t-1", "Daily Digest"),
        "Summarize today.",
        &sel(Some("gemini-3.8-flash"), Some("low"), Some("anthropic")),
    );
    let ThreadEvent::TriggerStarted {
        model,
        reasoning_effort,
        provider,
        ..
    } = event
    else {
        panic!("expected TriggerStarted");
    };
    assert_eq!(model.as_deref(), Some("gemini-3.8-flash"));
    assert_eq!(reasoning_effort.as_deref(), Some("low"));
    assert_eq!(
        provider.as_deref(),
        Some("anthropic"),
        "a trigger's pinned backend is recorded too, so a human follow-up on \
         the thread stays on the backend the fire ran on"
    );
}

fn sel(
    model: Option<&str>,
    effort: Option<&str>,
    provider: Option<&str>,
) -> crate::core::ResolvedModelSelection {
    crate::core::ResolvedModelSelection {
        model: model.map(str::to_string),
        reasoning_effort: effort.map(str::to_string),
        provider: provider.map(str::to_string),
    }
}

const TEST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

fn thick_summary() -> String {
    "Fixed the summariser. ".repeat(40)
}

#[tokio::test(start_paused = true)]
async fn summarize_yields_nothing_when_flash_hangs() {
    let hang = pending::<Result<String, Box<dyn std::error::Error + Send + Sync>>>();
    assert_eq!(summarize_or_none(hang, 7, TEST_DEADLINE).await, None);
}

#[tokio::test(start_paused = true)]
async fn summarize_yields_nothing_on_provider_error() {
    let err =
        async { Err::<String, Box<dyn std::error::Error + Send + Sync>>("vertex 503".into()) };
    assert_eq!(summarize_or_none(err, 4, TEST_DEADLINE).await, None);
}

#[tokio::test(start_paused = true)]
async fn summarize_yields_nothing_on_a_blank_answer() {
    let blank = async { Ok::<String, Box<dyn std::error::Error + Send + Sync>>("  \n ".into()) };
    assert_eq!(summarize_or_none(blank, 9, TEST_DEADLINE).await, None);
}

#[tokio::test(start_paused = true)]
async fn summarize_yields_nothing_on_a_thin_answer() {
    let thin = async {
        Ok::<String, Box<dyn std::error::Error + Send + Sync>>(
            "The assistant worked on the thread.".into(),
        )
    };
    assert_eq!(summarize_or_none(thin, 19, TEST_DEADLINE).await, None);
}

#[tokio::test(start_paused = true)]
async fn a_short_segment_may_have_a_short_summary() {
    let text = "Fixed the login redirect, and confirmed the session cookie now survives \
                a reload. Nothing else was touched in this segment.";
    let short = async { Ok::<String, Box<dyn std::error::Error + Send + Sync>>(text.into()) };
    assert_eq!(
        summarize_or_none(short, 2, TEST_DEADLINE).await,
        Some(text.to_string())
    );
}

#[tokio::test(start_paused = true)]
async fn summarize_returns_the_paragraph_it_got() {
    let text = thick_summary();
    let expected = text.clone();
    let ok = async { Ok::<String, Box<dyn std::error::Error + Send + Sync>>(text) };
    assert_eq!(
        summarize_or_none(ok, 9, TEST_DEADLINE).await,
        Some(expected)
    );
}

#[tokio::test(start_paused = true)]
async fn a_multibyte_summary_is_measured_in_characters() {
    let text = "要約".repeat(125);
    assert!(
        text.len() > 600,
        "the byte count would have passed the floor"
    );
    assert!(
        text.chars().count() < 600,
        "the character count must not pass it"
    );
    let thin = async { Ok::<String, Box<dyn std::error::Error + Send + Sync>>(text) };
    assert_eq!(summarize_or_none(thin, 19, TEST_DEADLINE).await, None);
}

#[test]
fn the_thin_output_floor_scales_then_caps() {
    assert!(!summary_is_too_thin(100, 1));
    assert!(summary_is_too_thin(100, 19));
    assert!(!summary_is_too_thin(600, 40));
    assert!(summary_is_too_thin(599, 40));
    assert!(!summary_is_too_thin(0, 0));
}

#[tokio::test(start_paused = true)]
async fn classify_falls_back_when_flash_hangs() {
    let hang = pending::<Result<QueryClassification, Box<dyn std::error::Error + Send + Sync>>>();
    let result = classify_or_fallback(hang, TEST_DEADLINE).await;
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
    let result = classify_or_fallback(err, TEST_DEADLINE).await;
    assert!(result.needs_memory);
    assert!(result.needs_file_list);
    assert!(result.needs_credentials);
    assert!(result.sub_queries.is_empty());
}

#[test]
fn the_all_pin_retrieves_every_section() {
    let pinned = forced_classification(Some("all")).expect("`all` is a pin");
    assert!(pinned.needs_memory);
    assert!(pinned.needs_file_list);
    assert!(pinned.needs_credentials);
    assert!(pinned.sub_queries.is_empty());
}

#[test]
fn the_none_pin_retrieves_nothing() {
    let pinned = forced_classification(Some("none")).expect("`none` is a pin");
    assert!(!pinned.needs_memory);
    assert!(!pinned.needs_file_list);
    assert!(!pinned.needs_credentials);
    assert!(pinned.sub_queries.is_empty());
}

#[test]
fn no_pin_means_the_classifier_runs_as_before() {
    assert!(forced_classification(None).is_none());
}

#[test]
fn a_value_that_is_neither_all_nor_none_is_ignored() {
    assert!(forced_classification(Some("yes")).is_none());
    assert!(forced_classification(Some("true")).is_none());
    assert!(forced_classification(Some("")).is_none());
    assert!(forced_classification(Some("   ")).is_none());
    assert!(forced_classification(Some("all,none")).is_none());
}

#[test]
fn a_padded_or_shouted_pin_is_still_a_pin() {
    assert!(forced_classification(Some("  all  ")).is_some());
    assert!(forced_classification(Some("ALL")).is_some());
    assert!(forced_classification(Some(" None\n")).is_some());
    assert_eq!(
        forced_classification(Some("NONE")).map(|c| c.needs_memory),
        Some(false)
    );
}

#[test]
fn the_pin_is_the_name_and_value_the_eval_harness_exports() {
    assert_eq!(
        FORCE_QUERY_CLASSIFICATION_ENV,
        "LUCIDOS_FORCE_QUERY_CLASSIFICATION"
    );
    assert!(forced_classification(Some("all")).is_some());
}

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

#[test]
fn trigger_knowhow_section_scoped_to_firing_trigger() {
    let tmp = tempfile::tempdir().unwrap();
    let triggers_dir = tmp.path().to_path_buf();
    let kh_dir = triggers_dir.join("nightly-build").join("knowhow");
    std::fs::create_dir_all(&kh_dir).unwrap();
    std::fs::write(
        kh_dir.join("orchestration.md"),
        "---\nname: Orchestration\n---\nHow nightly orchestrates each phase.",
    )
    .unwrap();

    let section = build_trigger_knowhow_section(&triggers_dir, "nightly-build");
    assert!(
        section.starts_with("[TRIGGER KNOW-HOW]") && section.ends_with("[END TRIGGER KNOW-HOW]"),
        "trigger thread must see the [TRIGGER KNOW-HOW] block, got: {section}"
    );
    assert!(
        section.contains("Orchestration"),
        "section must list file's name, got: {section}"
    );
    assert!(
        section.contains("triggers/nightly-build/orchestration"),
        "section must use the trigger-scoped id namespace, got: {section}"
    );

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
    let section = build_trigger_knowhow_section(&triggers_dir, "standalone");
    assert!(section.is_empty());
}

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

fn apply_verify_rule_dev_variant() -> String {
    format!("{APPLY_VERIFY_RULE}{APPLY_VERIFY_DEV_ADDENDUM}")
}

#[test]
fn apply_verify_rule_tells_agent_to_act_and_verify_not_ask() {
    let rule = apply_verify_rule_dev_variant();
    let lowered = rule.to_lowercase();
    assert!(
        rule.contains("`changes` tool")
            && rule.contains("action 'list'")
            && rule.contains("action 'apply'"),
        "rule must name the grouped `changes` tool and its list/apply actions:\n{rule}"
    );
    assert!(
        lowered.contains("have you applied it") && lowered.contains("did you restart"),
        "rule must explicitly ban the two confirmation questions:\n{rule}"
    );
    assert!(
        lowered.contains("cannot restart the engine"),
        "rule must state the agent cannot restart the engine (only the user can):\n{rule}"
    );
    assert!(
        lowered.contains("probing the served asset")
            || lowered.contains("probe the served asset")
            || lowered.contains("probe served assets"),
        "rule must tell the agent to verify by probing the served asset:\n{rule}"
    );
    assert!(
        rule.contains("/<workspace>/api/v1/") && lowered.contains("unknown workspace 'api'"),
        "rule must record the workspace-prefixed-route gateway gotcha:\n{rule}"
    );
    assert!(
        lowered.contains("does it look right now") || lowered.contains("does it match"),
        "rule must ban the post-apply confirmation-question closer:\n{rule}"
    );
}

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

#[test]
fn apply_verify_rule_reinforces_user_knows_the_dance() {
    let rule = apply_verify_rule_dev_variant();
    let lowered = rule.to_lowercase();
    assert!(
        lowered.contains("knows the apply/restart/reload dance")
            && lowered.contains("do not re-explain it"),
        "rule must reinforce that the user knows the dance and must not be re-taught it:\n{rule}"
    );
}

#[test]
fn loaded_knowhow_block_emits_doc_bodies_verbatim() {
    let docs = vec![
        LoadedKnowhow::loaded_turns_ago(
            "alpha",
            "[SYSTEM-KNOWHOW: Alpha Doc]\nBody A\n[END SYSTEM-KNOWHOW]",
            1,
        ),
        LoadedKnowhow::loaded_turns_ago("beta", "[KNOW-HOW: Beta Doc]\nBody B\n[END KNOW-HOW]", 3),
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
    assert!(s.contains("[SYSTEM-KNOWHOW: Alpha Doc]"));
    assert!(s.contains("[KNOW-HOW: Beta Doc]"));
    assert!(s.contains("Body A"));
    assert!(s.contains("Body B"));
    assert!(
        !s.contains("[SYSTEM-KNOWHOW: alpha]"),
        "must not re-wrap with id as outer marker:\n{s}"
    );
    assert!(
        !s.contains("[SYSTEM-KNOWHOW: beta]"),
        "must not re-wrap with id as outer marker:\n{s}"
    );
    assert!(
        s.contains("Treat their guidance as authoritative"),
        "header guidance missing from block:\n{s}"
    );
    assert!(s.contains("call load_knowhow on it again"), "{s}");
    assert!(s.contains("unload_knowhow"), "{s}");
    for doc in &docs {
        let line = format!("(`{}`)", doc.id);
        assert!(s.contains(&line), "missing `{line}`:\n{s}");
        assert!(
            !s.contains(&doc.retention_note()),
            "a retention note changes every turn, so it cannot sit in the stable block:\n{s}"
        );
    }
}

#[test]
fn loaded_knowhow_block_is_identical_at_every_idle_age() {
    let at = |idle_turns| {
        build_loaded_knowhow_block(&[LoadedKnowhow::loaded_turns_ago(
            "alpha",
            "[KNOW-HOW: Alpha]\nBody\n[END KNOW-HOW]",
            idle_turns,
        )])
    };
    assert_eq!(at(0), at(2));
    assert_eq!(at(0), at(3));
}

#[test]
fn knowhow_retention_block_carries_one_note_per_doc() {
    let docs = vec![
        LoadedKnowhow::loaded_turns_ago("alpha", "Body A", 0),
        LoadedKnowhow::loaded_turns_ago("beta", "Body B", 2),
    ];
    let s = build_knowhow_retention_block(&docs).expect("non-empty docs produce a block");
    assert!(s.starts_with("[KNOWHOW RETENTION]"), "{s}");
    assert!(s.ends_with("[END KNOWHOW RETENTION]"), "{s}");
    for doc in &docs {
        let line = format!("`{}` {}", doc.id, doc.retention_note());
        assert!(s.contains(&line), "missing `{line}`:\n{s}");
    }
    assert!(!s.contains("Body A") && !s.contains("Body B"), "{s}");
}

#[test]
fn knowhow_retention_block_returns_none_for_empty_docs() {
    assert!(build_knowhow_retention_block(&[]).is_none());
}

#[test]
fn loaded_knowhow_block_returns_none_for_empty_docs() {
    assert!(build_loaded_knowhow_block(&[]).is_none());
}

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
        by_name("MCP Servers").group,
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

#[test]
fn build_capture_sections_filters_empty_sections() {
    let sections = run_build(
        "sys",
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
        "",
        "",
        "user msg",
        &[],
        &[],
    );
    let names: Vec<_> = sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["System Instructions", "User Message", "Current Time"]
    );
}

#[test]
fn build_capture_sections_bills_the_todo_list_block() {
    let block = "[TODO LIST]\n1. [abandoned] a (doing a)\n[END TODO LIST]";
    let sections = build_capture_sections(
        "sys",
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
        "",
        "",
        "",
        "",
        block,
        "user msg",
        &TurnTail {
            child_threads: "",
            engine_build: "",
            client_url: "",
            current_time: CLOCK_BLOCK,
        },
        &[],
        &[],
    );

    let row = sections
        .iter()
        .find(|s| s.name == "Todo List")
        .expect("the todo block needs a row of its own");
    assert_eq!(row.role, ContextRole::User, "it rides in the user message");
    assert_eq!(row.group.as_deref(), Some("System notices"));
    assert_eq!(row.content.as_deref(), Some(block));
    assert_eq!(row.budget_delta_chars, block.chars().count());

    let empty = run_build(
        "sys",
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
        "",
        "",
        "user msg",
        &[],
        &[],
    );
    assert!(
        !empty.iter().any(|s| s.name == "Todo List"),
        "a turn with no block must not carry an empty row"
    );
}

#[test]
fn build_capture_sections_bills_the_mcp_tools_not_sent_notice() {
    let notice = "[MCP TOOLS NOT SENT] server 'github' (2: a, b)";
    let sections = build_capture_sections(
        "sys",
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
        notice,
        "",
        "",
        "",
        "",
        "user msg",
        &TurnTail {
            child_threads: "",
            engine_build: "",
            client_url: "",
            current_time: CLOCK_BLOCK,
        },
        &[],
        &[],
    );

    let row = sections
        .iter()
        .find(|s| s.name == "MCP Tools Not Sent")
        .expect("the notice needs a row of its own");
    assert_eq!(row.role, ContextRole::User);
    assert_eq!(row.group.as_deref(), Some("System notices"));
    assert_eq!(row.content.as_deref(), Some(notice));
}

#[test]
fn build_capture_sections_surfaces_the_two_relocated_tail_blocks() {
    let sections = build_capture_sections(
        "sys",
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
        "",
        "",
        "",
        "",
        "",
        "user msg",
        &TurnTail {
            child_threads: "[CHILD THREADS]\n- \"Split\": working now\n[END CHILD THREADS]",
            engine_build: "[ENGINE BUILD]\nCURRENT\n[END ENGINE BUILD]",
            client_url: "[CLIENT URL]\nhttps://localhost:5173\n[END CLIENT URL]",
            current_time: CLOCK_BLOCK,
        },
        &[],
        &[],
    );

    let names: Vec<_> = sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "System Instructions",
            "User Message",
            "Child Threads",
            "Engine Build",
            "Client URL",
            "Current Time"
        ]
    );
    for name in [
        "Child Threads",
        "Engine Build",
        "Client URL",
        "Current Time",
    ] {
        let row = sections
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("section {name} missing"));
        assert_eq!(
            row.role,
            ContextRole::User,
            "{name} rides in the user message"
        );
        assert_eq!(row.group, Some("The request".to_string()), "{name}");
    }
}

#[test]
fn build_capture_sections_emits_one_row_per_loaded_knowhow_doc() {
    let docs = vec![
        LoadedKnowhow::loaded_turns_ago("doc-a", "BODY A", 1),
        LoadedKnowhow::loaded_turns_ago("doc-b", "BODY BBBB", 3),
    ];
    let sections = run_build(
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
        "",
        "",
        "",
        "user msg",
        &docs,
        &[],
    );

    let knowhow: Vec<_> = sections
        .iter()
        .filter(|s| s.group.as_deref() == Some("Loaded knowhow"))
        .collect();
    assert_eq!(knowhow.len(), 2, "one row per loaded doc");
    assert_eq!(
        knowhow[0].name,
        format!("knowhow: doc-a ({})", docs[0].retention_note())
    );
    assert_eq!(
        knowhow[1].name,
        format!("knowhow: doc-b ({})", docs[1].retention_note())
    );
    assert!(knowhow.iter().all(|s| s.role == ContextRole::User));
    assert_eq!(knowhow[0].budget_delta_chars, "BODY A".chars().count());
    assert_eq!(knowhow[1].budget_delta_chars, "BODY BBBB".chars().count());
    assert_eq!(knowhow[0].content_chars, Some("BODY A".chars().count()));
    assert_eq!(knowhow[1].content_chars, Some("BODY BBBB".chars().count()));
    assert_eq!(knowhow[0].content.as_deref(), Some("BODY A"));
    assert_eq!(knowhow[1].content.as_deref(), Some("BODY BBBB"));
}

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
        "",
        "",
        "",
        "user msg",
        &[],
        &resume,
    );

    let prior: Vec<_> = sections
        .iter()
        .filter(|s| s.role == ContextRole::PriorMessage)
        .collect();
    assert_eq!(prior.len(), 2, "one row per resume pair");
    assert!(prior.iter().any(|s| s.name == "ToolUse: query_events"));
    assert!(prior.iter().any(|s| s.name == "ToolUse: load_knowhow"));
    assert!(prior.iter().all(|s| s.group.is_none()));
    assert!(prior.iter().all(|s| s.content.is_some()));
    assert!(prior.iter().all(|s| s.budget_delta_chars > 0));
    assert!(prior
        .iter()
        .all(|s| s.content_chars == Some(s.budget_delta_chars)));
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
    assert!(section.budget_delta_chars > 0);
}

#[test]
fn build_capture_sections_always_carries_the_bodies() {
    let docs = vec![LoadedKnowhow::loaded_turns_ago("doc", "BODY", 1)];
    let sections = run_build(
        "sys",
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
        "",
        "",
        "user",
        &docs,
        &[],
    );
    let system = sections
        .iter()
        .find(|s| s.name == "System Instructions")
        .unwrap();
    assert_eq!(system.content.as_deref(), Some("sys"));
    let knowhow = sections
        .iter()
        .find(|s| s.group.as_deref() == Some("Loaded knowhow"))
        .unwrap();
    assert_eq!(knowhow.content.as_deref(), Some("BODY"));
}

#[test]
fn a_capped_body_still_reports_its_true_length() {
    let long = "x".repeat(20_000);
    let sections = run_build(
        &long,
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
        "",
        "",
        "user",
        &[],
        &[],
    );
    let system = sections
        .iter()
        .find(|s| s.name == "System Instructions")
        .expect("the system prompt is always a section");
    assert_eq!(
        system.budget_delta_chars,
        long.chars().count(),
        "the delta is the assembled length, not the persisted one"
    );
    assert_eq!(
        system.content_chars,
        Some(long.chars().count()),
        "a clipped body does not move the region's size"
    );
    let body = system.content.as_deref().unwrap_or_default();
    assert!(
        body.chars().count() < system.budget_delta_chars,
        "an over-cap body is clipped by default"
    );
}

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
         follow-up-menu example — that's the exact failure pattern in \
         observed threads where the agent emits markdown bullets instead of \
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
    assert!(
        rule.contains("<ask_user_question"),
        "chat ASK_USER_QUESTION_RULE must show the forbidden literal tag \
         (`<ask_user_question`) — the model has emitted wrapper-tag text \
         instead of a real tool call multiple times on Opus 4.7 max effort, \
         and only naming the exact tag string makes the rule self-evident \
         to the model when it next debates the format",
    );
    assert!(
        rule.contains("INVOKE IT AS A TOOL CALL, NEVER AS TEXT"),
        "chat ASK_USER_QUESTION_RULE must carry the anti-inline-tag clause \
         under a heading of its own. Without it the rule only nudges *when* \
         to ask, not *how*, and Opus 4.7 keeps inventing \
         `<ask_user_question>` wrappers",
    );
}

#[test]
fn chat_prompt_forbids_faking_repeated_actions() {
    let rule = super::REPEATED_ACTION_RULE;
    assert!(
        rule.contains("again"),
        "REPEATED_ACTION_RULE must name the repeat-request trigger word \
         (\"again\") — that's the exact phrasing the user used when the agent \
         claimed a send it never made",
    );
    assert!(
        rule.contains("CURRENT TURN") || rule.contains("THIS turn") || rule.contains("this turn"),
        "REPEATED_ACTION_RULE must pin the action to the CURRENT turn — the \
         failure was the model treating a PRIOR turn's tool call as if it \
         counted for this one",
    );
    assert!(
        rule.contains("send_notification"),
        "REPEATED_ACTION_RULE must name `send_notification` — the concrete \
         action tool the model faked on repeat; naming it keeps the rule \
         self-evident to the model when it next debates whether a prior call \
         counts",
    );
    assert!(
        rule.contains("PREVIOUS turn") || rule.contains("does NOT mean the action happened"),
        "REPEATED_ACTION_RULE must spell out that a prior identical tool call \
         in context is a record of a PREVIOUS turn, not proof the action \
         happened this time — that misread is the root of the bug",
    );
}

use super::run::message_can_answer_pending_question;
use super::run::resolve_route_overrides;

#[tokio::test]
async fn saved_draft_execution_keeps_absent_effort_after_defaults_change() {
    use crate::core::{ModelDefaults, ResolvedModelSelection};
    let (pool, db) = crate::test_support::setup_test_db().await;
    let registry = crate::llm::model_registry::empty();
    let accepted = ResolvedModelSelection {
        model: Some("gpt-5".into()),
        reasoning_effort: None,
        provider: Some("openai".into()),
    };
    let origin = MessageOrigin::SavedDraft {
        submission_event_id: Uuid::new_v4(),
        submitted_by_thread_id: Uuid::new_v4(),
    };
    for effort in ["high", "low"] {
        crate::test_support::seed_preference(
            &pool,
            crate::core::prefs::CHAT_REASONING_EFFORTS.key(),
            &format!("gpt-5={effort}"),
        )
        .await
        .unwrap();
        let ordinary = resolve_route_overrides(
            &pool,
            ModelDefaults {
                registry: &registry,
                fallback_model: "gpt-5",
            },
            |_| true,
            Some(false),
            None,
            None,
            accepted.clone(),
        );
        let resolved = super::run::resolve_input_settings(
            Some(&origin),
            Some(false),
            accepted.clone(),
            ordinary,
        )
        .await
        .unwrap();
        assert_eq!(
            resolved, accepted,
            "the accepted absence must not inherit a later default"
        );
    }
    pool.close().await;
    crate::test_support::teardown_test_db(&db).await;
}
#[tokio::test]
async fn saved_draft_execution_requires_frozen_chat_settings_only() {
    use crate::core::ResolvedModelSelection;
    let origin = MessageOrigin::SavedDraft {
        submission_event_id: Uuid::new_v4(),
        submitted_by_thread_id: Uuid::new_v4(),
    };
    let ordinary = ResolvedModelSelection {
        model: Some("ordinary-model".into()),
        reasoning_effort: Some("high".into()),
        provider: Some("openai".into()),
    };
    for (origin, coding) in [(None, false), (Some(&origin), true)] {
        assert_eq!(
            super::run::resolve_input_settings(origin, Some(coding), Default::default(), async {
                ordinary.clone()
            })
            .await
            .unwrap(),
            ordinary
        );
    }
    for requested in [
        ResolvedModelSelection::default(),
        ResolvedModelSelection {
            model: None,
            ..ordinary.clone()
        },
        ResolvedModelSelection {
            provider: None,
            ..ordinary.clone()
        },
    ] {
        let error =
            super::run::resolve_input_settings(Some(&origin), Some(false), requested, async {
                panic!("saved chat must not read defaults")
            })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "saved_draft_chat_settings_unresolved");
    }
}

use super::PreEmittedOrigin;
use crate::core::prefs;
use crate::engine::thread_events::ActorMode;
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

fn registry() -> crate::llm::ModelRegistry {
    crate::llm::model_registry::empty()
}

const FALLBACK_MODEL: &str = "router-default";

async fn seed_chat_effort(pool: &sqlx::PgPool, model: &str, effort: &str) {
    crate::test_support::seed_preference(
        pool,
        prefs::CHAT_REASONING_EFFORTS.key(),
        &format!("{model}={effort}"),
    )
    .await
    .unwrap();
}

fn registry_with(id: &str, provider: crate::llm::ProviderKind) -> crate::llm::ModelRegistry {
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};
    let mut map = HashMap::new();
    map.insert(
        id.to_string(),
        crate::llm::model_registry::ModelRouting::single(provider, id),
    );
    Arc::new(RwLock::new(map))
}

#[tokio::test]
async fn coding_agent_route_does_not_inherit_chat_model_or_effort_defaults() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "gemini-3.8-flash")
        .await
        .unwrap();
    seed_chat_effort(&pool, "gemini-3.8-flash", "max").await;

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry(),
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        Some(true),
        None,
        None,
        sel(Some("claude-opus-4-8[1m]"), None, None),
    )
    .await;

    assert_eq!(resolved.model, None);
    assert_eq!(resolved.reasoning_effort, None);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn coding_agent_route_preserves_explicit_agent_effort_pick() {
    let (pool, db_name) = setup_test_db().await;
    seed_chat_effort(&pool, "claude-opus-4-8[1m]", "high").await;

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry(),
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        Some(true),
        None,
        None,
        sel(None, Some("xhigh"), None),
    )
    .await;

    assert_eq!(resolved.model, None);
    assert_eq!(resolved.reasoning_effort.as_deref(), Some("xhigh"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn chat_route_still_inherits_chat_model_and_effort_defaults() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "claude-opus-4-8[1m]")
        .await
        .unwrap();
    seed_chat_effort(&pool, "claude-opus-4-8[1m]", "high").await;

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry(),
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        sel(None, None, None),
    )
    .await;

    assert_eq!(resolved.model.as_deref(), Some("claude-opus-4-8[1m]"));
    assert_eq!(resolved.reasoning_effort.as_deref(), Some("high"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn chat_route_reuses_thread_last_model_over_preference() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "account-model")
        .await
        .unwrap();
    seed_chat_effort(&pool, "account-model", "high").await;
    let tid = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2, 'MessageReceived', $3, now(), $4)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(tid.to_string())
    .bind(serde_json::json!({ "text": "hi", "model": "thread-model", "reasoning_effort": "low" }))
    .bind(tid)
    .execute(&pool)
    .await
    .unwrap();

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry(),
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        Some(tid),
        None,
        sel(None, None, None),
    )
    .await;

    assert_eq!(resolved.model.as_deref(), Some("thread-model"));
    assert_eq!(resolved.reasoning_effort.as_deref(), Some("low"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn follow_up_on_a_trigger_thread_reuses_the_fire_model() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "account-model")
        .await
        .unwrap();
    seed_chat_effort(&pool, "account-model", "high").await;
    let tid = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2, 'TriggerStarted', $3, now(), $4)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(tid.to_string())
    .bind(serde_json::json!({
        "trigger_id": "t-1",
        "model": "gemini-3.8-flash",
        "reasoning_effort": "low",
    }))
    .bind(tid)
    .execute(&pool)
    .await
    .unwrap();

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry(),
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        Some(tid),
        None,
        sel(None, None, None),
    )
    .await;

    assert_eq!(resolved.model.as_deref(), Some("gemini-3.8-flash"));
    assert_eq!(resolved.reasoning_effort.as_deref(), Some("low"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_route_prefers_the_triggers_own_model_and_effort() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "account-model")
        .await
        .unwrap();
    seed_chat_effort(&pool, "account-model", "high").await;

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry(),
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        sel(Some("gemini-3.8-flash"), Some("low"), None),
    )
    .await;

    assert_eq!(resolved.model.as_deref(), Some("gemini-3.8-flash"));
    assert_eq!(resolved.reasoning_effort.as_deref(), Some("low"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trigger_route_resolves_model_and_effort_independently() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "claude-opus-5")
        .await
        .unwrap();
    seed_chat_effort(&pool, "claude-opus-5", "high").await;

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry(),
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        sel(Some("gemini-3.8-flash"), None, None),
    )
    .await;
    assert_eq!(resolved.model.as_deref(), Some("gemini-3.8-flash"));
    assert_eq!(
        resolved.reasoning_effort, None,
        "the account's tier belongs to its own model"
    );

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry(),
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        sel(None, Some("max"), None),
    )
    .await;
    assert_eq!(
        resolved.model.as_deref(),
        Some("claude-opus-5"),
        "account model still applies"
    );
    assert_eq!(resolved.reasoning_effort.as_deref(), Some("max"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_resolved_effort_is_clamped_to_what_the_resolved_model_supports() {
    let (pool, db_name) = setup_test_db().await;
    seed_chat_effort(&pool, "muse-glimmer:30b-mlx", "xhigh").await;
    let registry = registry_with("muse-glimmer:30b-mlx", crate::llm::ProviderKind::Local);

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        sel(Some("muse-glimmer:30b-mlx"), None, None),
    )
    .await;

    assert_eq!(resolved.model.as_deref(), Some("muse-glimmer:30b-mlx"));
    assert_eq!(
        resolved.reasoning_effort.as_deref(),
        Some("high"),
        "a local model cannot run xhigh, so the turn must not record that it did"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn sub_thread_route(
    pool: &sqlx::PgPool,
    registry: &crate::llm::ModelRegistry,
    args: serde_json::Value,
    caller: crate::core::ResolvedModelSelection,
) -> crate::core::ResolvedModelSelection {
    let overrides = crate::engine::agentic_loop::sub_thread_route_overrides(&args, &caller)
        .expect("valid pins");
    resolve_route_overrides(
        pool,
        crate::core::ModelDefaults {
            registry,
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        overrides,
    )
    .await
}

#[tokio::test]
async fn an_unpinned_child_runs_on_its_callers_route_not_the_chat_model() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "account-model")
        .await
        .unwrap();
    seed_chat_effort(&pool, "account-model", "high").await;
    let registry = registry_with("claude-sonnet-5", crate::llm::ProviderKind::Anthropic);

    let resolved = sub_thread_route(
        &pool,
        &registry,
        serde_json::json!({ "prompt": "p" }),
        sel(Some("claude-sonnet-5"), Some("low"), Some("anthropic")),
    )
    .await;

    assert_eq!(
        resolved,
        sel(Some("claude-sonnet-5"), Some("low"), Some("anthropic"))
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_childs_pinned_effort_is_clamped_to_the_inherited_route() {
    let (pool, db_name) = setup_test_db().await;
    let registry = registry_with("muse-glimmer:30b-mlx", crate::llm::ProviderKind::Local);

    let resolved = sub_thread_route(
        &pool,
        &registry,
        serde_json::json!({ "prompt": "p", "reasoning_effort": "xhigh" }),
        sel(Some("muse-glimmer:30b-mlx"), Some("low"), None),
    )
    .await;

    assert_eq!(resolved.model.as_deref(), Some("muse-glimmer:30b-mlx"));
    assert_eq!(
        resolved.reasoning_effort.as_deref(),
        Some("high"),
        "a local model cannot run xhigh, so the child must not stamp it"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_coding_agent_effort_is_not_clamped_against_the_chat_registry() {
    let (pool, db_name) = setup_test_db().await;
    let registry = registry_with("muse-glimmer:30b-mlx", crate::llm::ProviderKind::Local);

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        Some(true),
        None,
        None,
        sel(None, Some("xhigh"), None),
    )
    .await;

    assert_eq!(resolved.model, None);
    assert_eq!(resolved.reasoning_effort.as_deref(), Some("xhigh"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_coding_agent_effort_survives_a_stored_chat_model_that_cannot_run_it() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "muse-glimmer:30b-mlx")
        .await
        .unwrap();
    let registry = registry_with("muse-glimmer:30b-mlx", crate::llm::ProviderKind::Local);

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        Some(true),
        None,
        None,
        sel(None, Some("xhigh"), None),
    )
    .await;

    assert_eq!(resolved.model, None);
    assert_eq!(resolved.reasoning_effort.as_deref(), Some("xhigh"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_model_on_two_backends_is_clamped_for_the_backend_the_turn_pinned() {
    use crate::llm::model_registry::{ModelRouting, RouteEntry};
    use crate::llm::ProviderKind;
    let (pool, db_name) = setup_test_db().await;
    let registry = crate::llm::model_registry::empty();
    registry.write().unwrap().insert(
        "two-routes".to_string(),
        ModelRouting {
            routes: vec![
                RouteEntry::new(ProviderKind::OpenAi, "gpt-5.2"),
                RouteEntry::new(ProviderKind::Local, "gpt-5.2"),
            ],
            preferred: None,
            vision: false,
            default_effort: None,
            successor: None,
        },
    );

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        sel(Some("two-routes"), Some("xhigh"), Some("local")),
    )
    .await;

    assert_eq!(resolved.provider.as_deref(), Some("local"));
    assert_eq!(
        resolved.reasoning_effort.as_deref(),
        Some("high"),
        "the pinned local route cannot run xhigh, whatever the first route can"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn wake_parent_follows_the_shared_workspace_schemas() {
    crate::test_support::run_on_engine_runtime(
        "engine::chat::process::tests::wake_parent_follows_the_shared_workspace_schemas",
        wake_parent_position_case,
    );
}

async fn wake_parent_position_case() {
    use super::run::ProcessRunExt;
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    for (thread_id, parent_thread_id) in [(parent, None), (child, Some(parent))] {
        engine
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::message_events::make_message_received(
                    std::path::Path::new("/tmp"),
                    "work",
                    None,
                    None,
                    parent_thread_id,
                    None,
                    None,
                    ActorMode::Human,
                    None,
                    None,
                    None,
                    None,
                    None,
                ),
                meta: crate::engine::thread_events::EventMeta {
                    channel: Some(crate::engine::thread_events::EventChannel::Chat),
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await
            .unwrap();
    }
    let gates = crate::llm::ToolCapabilities::default();
    let names = |tools: crate::engine::agentic_loop::TurnTools| -> Vec<String> {
        tools.defs().iter().map(|d| d.name.clone()).collect()
    };
    let alone = names(
        engine
            .chat_turn_tools(parent, &gates, 200_000, "claude-opus-5-5")
            .await,
    );
    let reporting = names(
        engine
            .chat_turn_tools(child, &gates, 200_000, "claude-opus-5-5")
            .await,
    );
    let wake = crate::llm::tools::wake_parent_tool().name;
    assert!(!alone.contains(&wake));
    assert_eq!(reporting.last(), Some(&wake));
    assert_eq!(
        reporting[..reporting.len() - 1],
        alone[..],
        "every schema before wake_parent keeps the workspace's shared prefix"
    );
    crate::test_support::teardown_test_db(&db).await;
}

#[test]
fn trimming_memory_context_never_cuts_inside_a_character() {
    let mut memory = "\u{e9}".repeat(10);
    super::run::cut_memory_tail(&mut memory, 3);
    assert_eq!(memory, "\u{e9}".repeat(8));

    let mut lines = "first line\nsecond \u{4e16}\u{754c} line".to_string();
    super::run::cut_memory_tail(&mut lines, 2);
    assert_eq!(lines, "first line");
}

#[test]
fn the_history_read_mark_precedes_each_history_read() {
    let source = include_str!("process/run.rs");
    let load = source
        .find("self.load_chat_history(")
        .expect("the history load must still be here");
    let views = source
        .find("self.turn_memory_views(")
        .expect("the Tree memory views must still be read here");
    let marks: Vec<usize> = source
        .match_indices("self.note_history_read_begun(")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(marks.len(), 2, "one mark per history read");
    assert!(
        marks[0] < load && load < marks[1] && marks[1] < views,
        "a row arriving between the load and its mark would be missed"
    );
}

#[test]
fn the_tool_call_cap_is_read_once_per_turn() {
    let source = include_str!("process/run.rs");
    assert_eq!(
        source.matches("PreferenceStore::max_tool_calls(").count(),
        1,
        "the prompt and the loop must share one read of the cap"
    );
}

#[test]
fn human_follow_up_can_answer_pending_question() {
    assert!(message_can_answer_pending_question(
        false,
        "repo is private, not public",
        false,
        ActorMode::Human,
        false,
        None,
    ));
}

#[test]
fn unattested_follow_up_cannot_answer_pending_question() {
    assert!(!message_can_answer_pending_question(
        false,
        "allow it",
        false,
        ActorMode::Human,
        true,
        None,
    ));
}

#[test]
fn image_only_follow_up_can_answer_pending_question() {
    assert!(message_can_answer_pending_question(
        false,
        "",
        true,
        ActorMode::Human,
        false,
        None,
    ));
}

#[test]
fn pre_emitted_message_cannot_also_answer_pending_question() {
    assert!(!message_can_answer_pending_question(
        false,
        "repo is private, not public",
        false,
        ActorMode::Human,
        false,
        Some(PreEmittedOrigin::Message(Uuid::new_v4())),
    ));
}

#[test]
fn child_completion_wake_cannot_answer_pending_question() {
    assert!(!message_can_answer_pending_question(
        false,
        "[CHILD THREAD COMPLETED] 59328631… success\nSession summary…",
        false,
        ActorMode::Agent,
        false,
        None,
    ));
}

#[test]
fn engine_driven_message_cannot_answer_pending_question() {
    assert!(!message_can_answer_pending_question(
        false,
        "engine recovery note",
        false,
        ActorMode::Engine,
        false,
        None,
    ));
}

#[test]
fn new_thread_or_empty_message_cannot_answer_pending_question() {
    assert!(!message_can_answer_pending_question(
        true,
        "first message on a brand-new thread",
        false,
        ActorMode::Human,
        false,
        None,
    ));
    assert!(!message_can_answer_pending_question(
        false,
        "",
        false,
        ActorMode::Human,
        false,
        None,
    ));
}

#[test]
fn the_question_supersede_is_wired_below_the_answer_fast_path() {
    let source = include_str!("process/run.rs");
    let fast_path = source
        .find("lookup_active_question_tool_use_id")
        .expect("the FreeText answer fast-path must still run first");
    let permission_supersede = source
        .find("resolve_pending_permissions_as_superseded")
        .expect("the coding-agent-only supersede block must still exist");
    let question_supersede = source
        .find("resolve_pending_question_as_superseded(")
        .expect("the message router must supersede an unanswerable open question");

    assert!(
        fast_path < question_supersede,
        "a message that can answer the question must answer it, never supersede it"
    );
    assert!(
        permission_supersede < question_supersede,
        "the question supersede belongs in the coding-agent-only block, beside the permission one"
    );

    let keep_check = source
        .find("follow_up_keeps_open_question")
        .expect("the supersede must consult follow_up_keeps_open_question");
    assert!(
        permission_supersede < keep_check && keep_check < question_supersede,
        "the keep-open check must gate the question supersede"
    );

    let hold = source
        .find("held_messages::message_is_held")
        .expect("the router must hold agent messages behind an open question");
    let release = source
        .find("deliver_held_messages")
        .expect("a human message must release the held messages");
    assert!(
        fast_path < hold && hold < permission_supersede,
        "the hold sits after the answer fast path, before any supersede"
    );
    assert!(
        question_supersede < release,
        "the release follows the human message's own supersedes"
    );
}

#[tokio::test]
async fn a_provider_the_model_cannot_use_is_not_stamped() {
    let (pool, db_name) = setup_test_db().await;
    let registry = registry_with("m", crate::llm::ProviderKind::Vertex);

    let off_route = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        sel(Some("m"), Some("high"), Some("anthropic")),
    )
    .await;
    assert_eq!(off_route.provider, None);

    let on_route = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        sel(Some("m"), Some("high"), Some("vertex")),
    )
    .await;
    assert_eq!(on_route.provider.as_deref(), Some("vertex"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn every_section() -> TurnSections<'static> {
    TurnSections {
        recent: "RECENT",
        profile: "PROFILE",
        email_accounts: "EMAIL",
        oauth: "OAUTH",
        thread_depth: "DEPTH",
        loaded_knowhow: "KNOWHOW",
        device_preferences: "DEVICE",
        memory: "MEMORY",
        history: "HISTORY",
        file_list: "LIST",
        credentials: "CREDS",
        knowhow_retention: "RETENTION",
        app_context: "APP",
        file_context: "OPENFILE",
        url_context: "URL",
        todo_list: "TODO",
        mcp_servers: "MCP",
        mcp_dropped_notice: "DROPPED",
        trigger_knowhow: "TRIGGERDOCS",
        setup_reminder: "SETUP",
        request: "ASK",
        tail: TurnTail {
            child_threads: "CHILDREN",
            engine_build: "BUILD",
            client_url: "CLIENT",
            current_time: "CLOCK",
        },
    }
}

fn positions(text: &str, tokens: &[&str]) -> Vec<usize> {
    tokens
        .iter()
        .map(|t| {
            text.find(t)
                .unwrap_or_else(|| panic!("`{t}` is missing:\n{text}"))
        })
        .collect()
}

fn is_ascending(xs: &[usize]) -> bool {
    xs.windows(2).all(|w| w[0] < w[1])
}

#[test]
fn the_stable_text_holds_the_five_stable_sections_in_order() {
    let text = assemble_turn_text(&every_section());
    assert_eq!(text.stable, "PROFILE\n\nEMAIL\n\nOAUTH\n\nDEPTH\n\nKNOWHOW");
    for token in ["PROFILE", "EMAIL", "OAUTH", "DEPTH", "KNOWHOW"] {
        assert!(
            !text.volatile.contains(token),
            "{token} leaked:\n{}",
            text.volatile
        );
    }
}

#[test]
fn the_volatile_text_keeps_the_relative_order() {
    let text = assemble_turn_text(&every_section());
    let order = [
        "RECENT",
        "DEVICE",
        "MEMORY",
        "HISTORY",
        "LIST",
        "CREDS",
        "RETENTION",
        "APP",
        "OPENFILE",
        "URL",
        "TODO",
        "MCP",
        "DROPPED",
        "TRIGGERDOCS",
        "SETUP",
        "Request: ASK",
        "CHILDREN",
        "BUILD",
        "CLIENT",
        "CLOCK",
    ];
    assert!(
        is_ascending(&positions(&text.volatile, &order)),
        "{}",
        text.volatile
    );
}

#[test]
fn the_request_and_the_tail_end_the_volatile_text() {
    let text = assemble_turn_text(&every_section());
    assert!(
        text.volatile
            .ends_with("Request: ASK\n\nCHILDREN\n\nBUILD\n\nCLIENT\n\nCLOCK"),
        "{}",
        text.volatile
    );
}

#[test]
fn every_section_appears_exactly_once_across_the_two_texts() {
    let text = assemble_turn_text(&every_section());
    let whole = format!("{}\n\n{}", text.stable, text.volatile);
    for token in [
        "RECENT",
        "PROFILE",
        "EMAIL",
        "OAUTH",
        "DEPTH",
        "KNOWHOW",
        "DEVICE",
        "MEMORY",
        "HISTORY",
        "LIST",
        "OPENFILE",
        "URL",
        "CREDS",
        "RETENTION",
        "APP",
        "TODO",
        "MCP",
        "DROPPED",
        "TRIGGERDOCS",
        "SETUP",
        "CHILDREN",
        "BUILD",
        "CLIENT",
        "CLOCK",
    ] {
        assert_eq!(whole.matches(token).count(), 1, "{token}:\n{whole}");
    }
}

#[test]
fn the_stable_text_ignores_every_volatile_input() {
    let base = assemble_turn_text(&every_section());
    let changed = assemble_turn_text(&TurnSections {
        recent: "other recent",
        device_preferences: "seen 9 min ago",
        memory: "other memory",
        history: "longer history",
        file_list: "",
        credentials: "other credentials",
        knowhow_retention: "stays 1 more turn",
        app_context: "",
        file_context: "other file",
        url_context: "other url",
        todo_list: "other todo",
        mcp_servers: "",
        mcp_dropped_notice: "",
        trigger_knowhow: "other docs",
        setup_reminder: "",
        request: "another request",
        tail: TurnTail {
            child_threads: "other children",
            engine_build: "other build",
            client_url: "other client",
            current_time: "other clock",
        },
        ..every_section()
    });
    assert_eq!(base.stable, changed.stable);
    assert_ne!(base.volatile, changed.volatile);
}

#[test]
fn empty_sections_leave_no_separator_behind() {
    let text = assemble_turn_text(&TurnSections {
        request: "ASK",
        tail: TurnTail {
            child_threads: "",
            engine_build: "",
            client_url: "CLIENT",
            current_time: "CLOCK",
        },
        ..TurnSections::default()
    });
    assert_eq!(text.stable, "");
    assert_eq!(text.volatile, "Request: ASK\n\nCLIENT\n\nCLOCK");
}

#[test]
fn a_trigger_knowhow_listing_rides_in_the_volatile_text() {
    let tmp = tempfile::tempdir().unwrap();
    let kh_dir = tmp.path().join("nightly-build").join("knowhow");
    std::fs::create_dir_all(&kh_dir).unwrap();
    std::fs::write(
        kh_dir.join("orchestration.md"),
        "---\nname: Orchestration\n---\nHow nightly orchestrates each phase.",
    )
    .unwrap();
    let listing = build_trigger_knowhow_section(tmp.path(), "nightly-build");
    let text = assemble_turn_text(&TurnSections {
        trigger_knowhow: &listing,
        request: "fire",
        ..TurnSections::default()
    });
    assert!(
        text.volatile
            .contains("triggers/nightly-build/orchestration"),
        "{}",
        text.volatile
    );
    assert!(!text.stable.contains("nightly-build"), "{}", text.stable);
}

#[test]
fn a_removed_knowhow_block_rides_in_the_volatile_text() {
    let removed = [crate::triggers::affected::RemovedKnowhow {
        path: "apps/token-cost/knowhow/daily-price-check.md".to_string(),
        removed_by: "the Token Cost update to v0.6.0".to_string(),
    }];
    let block = build_removed_knowhow_section(&removed);
    assert_eq!(
        block,
        "[REMOVED KNOW-HOW]\nA plugin removed know-how this trigger relied on:\n\
         - `apps/token-cost/knowhow/daily-price-check.md`, removed by the Token Cost update \
         to v0.6.0\nDo not invent a replacement recipe silently. Look for current know-how \
         with `load_knowhow`. Say in your result that this trigger's know-how was removed, \
         and suggest the user edit the trigger.\n[END REMOVED KNOW-HOW]"
    );
    let text = assemble_turn_text(&TurnSections {
        trigger_knowhow: &block,
        request: "fire",
        ..TurnSections::default()
    });
    assert!(
        text.volatile.contains("[REMOVED KNOW-HOW]"),
        "{}",
        text.volatile
    );
    assert!(!text.stable.contains("REMOVED"), "{}", text.stable);
    assert_eq!(build_removed_knowhow_section(&[]), "");
}

use super::run::trigger_model_override;

const TRIGGER_DEFAULT: &str = "claude-sonnet-5-5";

fn trigger_default_registry() -> crate::llm::ModelRegistry {
    let registry = registry_with(TRIGGER_DEFAULT, crate::llm::ProviderKind::Anthropic);
    registry
        .write()
        .unwrap()
        .get_mut(TRIGGER_DEFAULT)
        .unwrap()
        .default_effort = Some("high");
    registry
}

#[tokio::test]
async fn an_unpinned_trigger_runs_on_the_trigger_model_not_the_chat_model() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "claude-opus-5-5")
        .await
        .unwrap();
    let registry = trigger_default_registry();

    let model = trigger_model_override(&pool, &registry, |_| true, None).await;
    assert_eq!(model.as_deref(), Some(prefs::TRIGGER_MODEL.default_text()));
    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry,
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        sel(model.as_deref(), None, None),
    )
    .await;

    assert_eq!(resolved.model.as_deref(), Some(TRIGGER_DEFAULT));
    assert_eq!(resolved.reasoning_effort.as_deref(), Some("high"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_pinned_trigger_keeps_its_model() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::TRIGGER_MODEL.key(), "account-model")
        .await
        .unwrap();

    let model = trigger_model_override(&pool, &registry(), |_| true, Some("pinned-model")).await;
    assert_eq!(model.as_deref(), Some("pinned-model"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_stored_trigger_model_replaces_the_default() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::TRIGGER_MODEL.key(), "account-model")
        .await
        .unwrap();

    let model = trigger_model_override(&pool, &registry(), |_| true, None).await;
    assert_eq!(model.as_deref(), Some("account-model"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_unreachable_trigger_default_leaves_the_fire_to_the_chat_resolver() {
    let (pool, db_name) = setup_test_db().await;
    let registry = trigger_default_registry();

    let model = trigger_model_override(
        &pool,
        &registry,
        |kind| kind != crate::llm::ProviderKind::Anthropic,
        None,
    )
    .await;
    assert_eq!(model, None);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_trigger_model_never_reaches_a_chat_turn() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "chat-account-model")
        .await
        .unwrap();
    crate::test_support::seed_preference(
        &pool,
        prefs::TRIGGER_MODEL.key(),
        "trigger-account-model",
    )
    .await
    .unwrap();

    let resolved = resolve_route_overrides(
        &pool,
        crate::core::ModelDefaults {
            registry: &registry(),
            fallback_model: FALLBACK_MODEL,
        },
        |_| true,
        None,
        None,
        None,
        sel(None, None, None),
    )
    .await;

    assert_eq!(resolved.model.as_deref(), Some("chat-account-model"));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn trigger_started_carries_retry_provenance() {
    let retry_of = crate::engine::thread_events::RetryOf {
        thread_id: uuid::Uuid::new_v4(),
        deadline: chrono::Utc::now(),
    };
    let tc = trigger_fire_context("daily-report", "Daily report", |fire| {
        fire.retry_of = Some(retry_of.clone());
    });
    let (event, _) = started(&tc, "Run the check.", &sel(None, None, None));
    let ThreadEvent::TriggerStarted {
        retry_of: actual, ..
    } = event
    else {
        panic!("expected TriggerStarted");
    };
    assert_eq!(actual, Some(retry_of));
}

#[tokio::test]
async fn a_coding_agent_thread_decides_its_lane() {
    use super::run::resolve_coding_agent_lane;
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let coding = crate::test_support::seed_coding_thread(&bus, None).await;
    let chat = crate::test_support::seed_chat_thread(&bus, "hello").await;
    for requested in [None, Some(false), Some(true)] {
        assert_eq!(
            resolve_coding_agent_lane(&pool, coding, requested)
                .await
                .unwrap(),
            Some(true)
        );
        assert_eq!(
            resolve_coding_agent_lane(&pool, chat, requested)
                .await
                .unwrap(),
            requested
        );
        assert_eq!(
            resolve_coding_agent_lane(&pool, uuid::Uuid::new_v4(), requested)
                .await
                .unwrap(),
            requested
        );
    }
    sqlx::query("UPDATE thread_summaries SET state = 'composing' WHERE thread_id = $1")
        .bind(coding)
        .execute(&pool)
        .await
        .unwrap();
    for requested in [None, Some(false), Some(true)] {
        assert_eq!(
            resolve_coding_agent_lane(&pool, coding, requested)
                .await
                .unwrap(),
            requested,
            "a draft's lagged source cannot override the first send"
        );
    }
    pool.close().await;
    assert!(
        resolve_coding_agent_lane(&pool, coding, None)
            .await
            .is_err(),
        "an unreadable row is an unknown lane, never the caller's choice"
    );
    crate::test_support::teardown_test_db(&db).await;
}

#[test]
fn an_orphan_batch_precedes_newer_inputs_on_a_starting_run() {
    crate::test_support::run_on_engine_runtime(
        "engine::chat::process::tests::an_orphan_batch_precedes_newer_inputs_on_a_starting_run",
        orphan_batch_on_starting_run_case,
    );
}

async fn orphan_batch_on_starting_run_case() {
    use crate::engine::{AgentInputKind, AgentUserInput};
    let (_pool, db) = crate::test_support::setup_test_db().await;
    let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
    let thread_id = crate::test_support::seed_coding_thread(&engine.event_bus, None).await;
    let mut claim = engine
        .claim_thread_for_spawn(thread_id, None)
        .await
        .unwrap()
        .unwrap();
    let batch_id = uuid::Uuid::new_v4();
    let newer_id = uuid::Uuid::new_v4();
    let input = |text: &str, id| AgentUserInput {
        text: text.into(),
        images: None,
        origin_event_id: Some(id),
        kind: AgentInputKind::User,
        completion: None,
    };
    assert!(
        engine
            .route_to_live_run(thread_id, input("newer", newer_id))
            .await
    );
    let mut batch = vec![input("older batch tail", batch_id)];
    super::run::prepend_claimed_followups(&engine, &mut claim, &mut batch).await;
    assert!(batch.is_empty(), "the claimed run now owns the batch");
    for (text, id) in [("older batch tail", batch_id), ("newer", newer_id)] {
        let queued = claim.entry.msg_rx.try_recv().unwrap();
        assert_eq!(queued.text, text);
        assert_eq!(queued.origin_event_id, Some(id));
        assert_eq!(queued.kind, AgentInputKind::User);
    }
    assert!(
        claim.entry.msg_rx.try_recv().is_err(),
        "each input queues once"
    );
    let sessions = engine.agent_sessions.lock().await;
    let session = sessions.get(&thread_id).unwrap();
    assert!(session.unforwarded_inputs.contains(&batch_id));
    assert!(session.unforwarded_inputs.contains(&newer_id));
    drop(sessions);
    drop(claim);
    crate::test_support::teardown_test_db(&db).await;
}

#[tokio::test]
async fn available_defaults_reach_chat_calls_without_changing_pins_or_history() {
    use crate::llm::{model_registry, ProviderKind};
    use crate::runtime::intelligence_ladder::{load, LadderSurface, ModelRoutes};
    let (pool, db) = setup_test_db().await;
    let registry = model_registry::empty();
    model_registry::reload(&registry, &pool).await;
    let mut routes = ModelRoutes {
        registry,
        configured: vec![ProviderKind::OpenAi],
        provider: None,
    };
    let resolved = super::run::resolve_available_route_overrides(
        &pool,
        &routes,
        None,
        None,
        None,
        sel(None, None, None),
    )
    .await;
    assert_eq!(resolved.model.as_deref(), Some("gpt-6.2-sol"));
    assert_eq!(resolved.reasoning_effort.as_deref(), Some("medium"));
    let ladder = load(&pool, &routes, LadderSurface::Chat).await;
    let default = ladder.default_intelligence_rung.unwrap();
    assert_eq!(resolved.model.as_deref(), Some(default.model.as_str()));
    assert_eq!(
        resolved.reasoning_effort.as_deref(),
        Some(default.effort.as_str())
    );
    let pinned_provider = super::run::resolve_available_route_overrides(
        &pool,
        &routes,
        None,
        None,
        None,
        sel(None, None, Some("openrouter")),
    )
    .await;
    assert_eq!(pinned_provider.provider.as_deref(), Some("openrouter"));
    let ledger = crate::llm::not_served::NotServedLedger::default();
    let router = crate::llm::RoutingProvider::new(
        crate::llm::RoutingBackends {
            openai: Some(std::sync::Arc::new(
                crate::llm::OpenAiProvider::new("fixture-key".into(), "claude-opus-5-5".into())
                    .unwrap(),
            )),
            ..Default::default()
        },
        routes.registry.clone(),
        "claude-opus-5-5".into(),
    )
    .observing(ledger.clone());
    routes.provider = Some(std::sync::Arc::new(router));
    ledger.mark(ProviderKind::OpenAi, "gpt-6.2-sol");
    let excluded = super::run::resolve_available_route_overrides(
        &pool,
        &routes,
        None,
        None,
        None,
        sel(None, None, None),
    )
    .await;
    assert_eq!(excluded.model.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(
        load(&pool, &routes, LadderSurface::Chat)
            .await
            .default_intelligence_rung
            .unwrap()
            .model,
        "gpt-6.1-sol"
    );
    routes.provider = None;
    let tid = Uuid::new_v4();
    sqlx::query("INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) VALUES ($1, 'thread', $2, 'MessageReceived', $3, now(), $4)")
        .bind(Uuid::new_v4()).bind(tid.to_string())
        .bind(serde_json::json!({"text":"hi", "model":"gpt-6.1-sol", "reasoning_effort":"high"}))
        .bind(tid).execute(&pool).await.unwrap();
    for selection in [
        sel(Some("gpt-6.1-sol"), Some("high"), None),
        sel(None, None, None),
    ] {
        let resolved = super::run::resolve_available_route_overrides(
            &pool,
            &routes,
            None,
            Some(tid),
            None,
            selection,
        )
        .await;
        assert_eq!(resolved.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("high"));
    }
    crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "gpt-6.1-sol")
        .await
        .unwrap();
    seed_chat_effort(&pool, "gpt-6.1-sol", "high").await;
    let pinned = super::run::resolve_available_route_overrides(
        &pool,
        &routes,
        None,
        None,
        None,
        sel(None, None, None),
    )
    .await;
    assert_eq!(pinned.model.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(pinned.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(
        prefs::CHAT_MODEL.stored(&pool).await.as_deref(),
        Some("gpt-6.1-sol")
    );
    teardown_test_db(&db).await;
}

#[test]
fn a_raw_tool_echo_is_replaced_by_the_canned_line() {
    for echo in [
        "Tool results: ok",
        "[list_files] a b",
        "[read_file] x",
        "[run_python] y",
        "run_python({\"code\": \"1\"})",
        "  Tool results: padded",
    ] {
        assert_eq!(
            super::clean_response_text(echo),
            "Task completed. Check the workspace for any created files.",
            "{echo}"
        );
    }
}

#[test]
fn a_reply_that_only_quotes_a_tool_echo_keeps_its_answer() {
    for quoted in [
        "The guard fires when you call run_python({\"code\": \"1\"}) twice.",
        "Output looked like Tool results: and then more, so I stopped.",
        "See [list_files] and [read_file] and [run_python] above.",
    ] {
        assert_eq!(super::clean_response_text(quoted), quoted);
    }
}

#[tokio::test]
async fn a_later_terminator_cannot_mask_the_fires_first_abort() {
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        emit_response_aborted, AbortCause, EventChannel, EventMeta, ThreadEvent, TriggerInvocation,
    };

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let meta = || EventMeta {
        channel: Some(EventChannel::Trigger),
        ..EventMeta::NONE
    };
    let start_id = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::TriggerStarted {
                retry_of: None,
                provider: None,
                trigger_id: "masked-fire".into(),
                trigger_name: Some("Nightly report".into()),
                prompt: Some("Prepare the report".into()),
                invocation: Some(TriggerInvocation::Schedule),
                origin: None,
                go_to_review: false,
                model: None,
                reasoning_effort: None,
                continue_key: None,
            },
            meta: meta(),
        })
        .await
        .unwrap()
        .unwrap()
        .event_id;
    emit_response_aborted(
        &bus,
        thread_id,
        AbortCause::EngineShutdown,
        "This response was interrupted by an engine shutdown.".into(),
        vec![],
        None,
        None,
        EventMeta {
            request_event_id: Some(start_id),
            ..meta()
        },
        "[Test] abort before the later wake",
    )
    .await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "A later wake answered".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: meta(),
    })
    .await
    .unwrap();
    let later: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseGenerated'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(later, 1, "the later wake's terminator is on the thread");

    let result = crate::engine::agentic_loop::terminal_result(
        "A later wake answered".into(),
        vec![],
        Uuid::new_v4(),
        thread_id,
        false,
    );
    let error = super::trigger_process_result(&pool, result)
        .await
        .expect_err("the first terminator was an abort");
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::Interrupted
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
