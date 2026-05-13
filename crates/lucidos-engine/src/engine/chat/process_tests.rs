use super::super::process_helpers::{
    build_system_knowhow_section, build_trigger_knowhow_section, build_trigger_started_event,
    classify_or_fallback, summarize_or_fallback, TriggerContext, ENGINE_RESTART_RULE,
};
use crate::core::knowhow::KnowhowSummary;
use crate::engine::thread_events::{
    EngineReason, EventChannel, MessageOrigin, ThreadEvent, TriggerInvocation,
};
use crate::memory::QueryClassification;
use std::future::pending;

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
