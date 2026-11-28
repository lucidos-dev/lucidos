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

#[test]
fn start_merge_and_get_prompt_signature_and_prompt_match() {
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

    let prompt = crate::engine::merge_prompts::build_merge_prompt(
        "main",
        None,
        Some("Some description"),
        false,
    );
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
