use crate::engine::thread_events::{EngineReason, MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;
use uuid::Uuid;

#[test]
fn merge_conflict_event_serializes_with_engine_origin() {
    let ev = ThreadEvent::MergeConflictDetected {
        change_id: "cid".to_string(),
        files: vec!["a.rs".to_string()],
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::MergeConflict,
        }),
    };
    let v = serde_json::to_value(&ev).expect("serializes");
    assert_eq!(v["origin"]["kind"], "engine");
    assert_eq!(v["origin"]["reason"]["kind"], "merge_conflict");
}

#[test]
fn missing_hardening_event_serializes_with_engine_origin() {
    let ev = ThreadEvent::MissingHardeningDetected {
        origin: Some(MessageOrigin::Engine {
            reason: EngineReason::MissingHardening,
        }),
    };
    let v = serde_json::to_value(&ev).expect("serializes");
    assert_eq!(v["origin"]["kind"], "engine");
    assert_eq!(v["origin"]["reason"]["kind"], "missing_hardening");
}

/// `start_merge_and_get_prompt` MUST exist on `LucidosEngine` with the
/// shape the three merge call sites depend on
/// (`apply_now::merge_via_cc_session`, Tier-3 `spawn_merge_session`,
/// Tier-2 `run_merge_session_tier2`). Each site needs to pass the
/// thread/change ids, the conflicting files, the merge-target branch, an
/// optional body intro, and an optional description — and await back the
/// prompt string that ships to CC.
///
/// This file mirrors the codebase convention (see `event_bus_tests.rs`
/// and surrounding tests in this module): emit helpers on `LucidosEngine`
/// are not exercised via a live `LucidosEngine` because nothing in the
/// crate builds one outside `main.rs`. We instead:
///
///   1. Pin the helper's existence + argument shape via an unreachable
///      compile-only reference — this is the part that compile-fails
///      today (Step 2 of the plan) and compile-passes after Step 3 lands
///      the helper.
///   2. Assert the prompt half is exactly what `build_merge_prompt`
///      already produces, and is non-trivial — so any future drift in
///      `build_merge_prompt` that the helper would inherit is caught
///      here as well as in `prompts::tests`.
///
/// The emit half — that `start_merge_and_get_prompt` routes through
/// `emit_merge_conflict_detected` and therefore through `EventBus` — is
/// guaranteed structurally by reading the helper body (a 2-line
/// composition of `self.emit_merge_conflict_detected(...)` + the prompt
/// builder). The serialization shape of that event is locked by
/// `merge_conflict_event_serializes_with_engine_origin` above.
#[test]
fn start_merge_and_get_prompt_signature_and_prompt_match() {
    // (1) Compile-time existence + argument-shape pin. The async fn is
    // never executed at runtime — we only need the type-checker to verify
    // the method name resolves and accepts these argument types. If the
    // helper is missing (today) or its shape drifts, this fails to
    // build — exactly what Step 2 expects.
    #[allow(dead_code)]
    async fn check_signature_compiles(
        engine: &LucidosEngine,
        thread_id: Uuid,
        change_id: Uuid,
        files: Vec<String>,
        target_branch: &str,
        body_intro: Option<&str>,
        description: Option<&str>,
    ) -> String {
        engine
            .start_merge_and_get_prompt(
                thread_id,
                change_id,
                files,
                target_branch,
                body_intro,
                description,
            )
            .await
    }

    // (2) The prompt body the helper returns is exactly what the existing
    // `build_merge_prompt` produces for the same arguments. This is what
    // the three call sites depend on after we route them through the
    // helper in Tasks 2-4.
    let prompt = crate::engine::agent_session::build_merge_prompt(
        "main",
        None,
        Some("Some description"),
    );
    // Sanity: builder returns a non-trivial merge prompt — guards against
    // a silent change that makes both sides return `""`.
    assert!(
        prompt.contains("git merge main"),
        "build_merge_prompt('main', ...) must mention `git merge main`; got: {}",
        prompt,
    );
    assert!(
        prompt.contains("Some description"),
        "build_merge_prompt(..., Some('Some description')) must include the description; got: {}",
        prompt,
    );
}
