use super::*;
use lucidos_core::core::event_hooks::{EventHooks, HOOKS_DIR, HOOK_DEFINITION_FILE};
use lucidos_core::core::DATA_DIR;
use lucidos_core::engine::event_bus::{BusEvent, SystemEvent};
use lucidos_core::engine::thread_events::EventMeta;
use serde_json::json;

const SLUG: &str = "gate";

const DEFINITION: &str =
    "name = \"Gate\"\nevent_type = \"MessageReceived\"\nscript = \"check.sh\"\n";

fn write_hook(workspace: &Path, definition: &str, script: &str) {
    let dir = workspace.join(DATA_DIR).join(HOOKS_DIR).join(SLUG);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(HOOK_DEFINITION_FILE), definition).unwrap();
    std::fs::write(dir.join("check.sh"), script).unwrap();
}

fn armed(workspace: &Path) -> ArmedHook {
    let listing = crate::WorkspaceHooks
        .list_definitions(workspace)
        .unwrap()
        .into_iter()
        .find(|listing| listing.slug() == SLUG)
        .unwrap();
    serde_json::from_value(json!({
        "slug": SLUG,
        "event_type": "MessageReceived",
        "content_hash": listing.content_hash().expect("a valid hook"),
        "armed_at": "2026-11-01T00:00:00Z",
    }))
    .unwrap()
}

fn pending(payload: serde_json::Value) -> PendingEvent {
    PendingEvent {
        event_type: "MessageReceived".to_string(),
        payload,
    }
}

#[test]
fn a_hook_that_still_hashes_as_armed_runs_from_its_copy() {
    let workspace = tempfile::tempdir().unwrap();
    write_hook(workspace.path(), DEFINITION, "echo allow\n");
    let (staged, definition) = checked_copy(workspace.path(), &armed(workspace.path())).unwrap();
    assert_eq!(definition.script, "check.sh");
    assert!(staged.root().join(SLUG).join("check.sh").is_file());
}

#[test]
fn an_edited_hook_refuses_as_changed() {
    let workspace = tempfile::tempdir().unwrap();
    write_hook(workspace.path(), DEFINITION, "echo allow\n");
    let armed = armed(workspace.path());
    write_hook(workspace.path(), DEFINITION, "echo edited\n");
    let refusal = checked_copy(workspace.path(), &armed).err().unwrap();
    assert_eq!(refusal.cause, RefusalCause::Changed, "{refusal:?}");
    assert!(refusal.reason.contains("arms it again"), "{refusal:?}");
}

#[test]
fn a_hook_that_broke_or_left_refuses_as_changed() {
    let workspace = tempfile::tempdir().unwrap();
    write_hook(workspace.path(), DEFINITION, "echo allow\n");
    let armed = armed(workspace.path());
    write_hook(workspace.path(), "not toml", "echo allow\n");
    let broke = checked_copy(workspace.path(), &armed).err().unwrap();
    assert_eq!(broke.cause, RefusalCause::Changed, "{broke:?}");

    std::fs::remove_dir_all(workspace.path().join(DATA_DIR).join(HOOKS_DIR)).unwrap();
    let left = checked_copy(workspace.path(), &armed).err().unwrap();
    assert_eq!(left.cause, RefusalCause::Changed, "{left:?}");
}

#[test]
fn the_condition_filters_before_any_script() {
    let mut definition: HookDefinition = toml::from_str(DEFINITION).unwrap();
    assert!(condition_matches(
        &definition,
        &pending(json!({"text": "hi"}))
    ));
    definition.condition = Some(json!({"first_message": true}));
    assert!(!condition_matches(
        &definition,
        &pending(json!({"text": "hi"}))
    ));
    assert!(condition_matches(
        &definition,
        &pending(json!({"text": "hi", "first_message": true}))
    ));
}

#[test]
fn each_answer_maps_to_its_verdict() {
    assert_eq!(
        verdict_of(SLUG, HookAnswer::Allow { events: vec![] }),
        HookVerdict::Allowed(vec![])
    );
    let HookVerdict::Refused(missing) = verdict_of(
        SLUG,
        HookAnswer::NeedsInput {
            argument: "ticket".to_string(),
        },
    ) else {
        panic!("a needs-input answer refuses");
    };
    assert_eq!(missing.cause, RefusalCause::MissingInput);
    assert!(missing.reason.contains("ticket"), "{missing:?}");
    let refused = Refusal {
        cause: RefusalCause::Refused,
        reason: "No plan.".to_string(),
        detail: None,
    };
    assert_eq!(
        verdict_of(SLUG, HookAnswer::Refuse(refused.clone())),
        HookVerdict::Refused(refused)
    );
}

#[tokio::test]
async fn an_armed_hook_stops_a_threads_first_message_as_itself() {
    lucidos_core::core::secret_store::key_store::install_test_key_store();
    lucidos_core::core::subprocess_origin::init_agent_origin_secret("asker-test".to_string());
    let (pool, db) = lucidos_core::test_support::setup_test_db().await;
    let (engine, _workspace) = lucidos_engine::test_support::boot_mock_engine_with_host(
        &db,
        Arc::new(lucidos_engine::engine::agent_host::NoAgentHost::default()),
    )
    .await;
    let workspace = engine.workspace_path().to_path_buf();
    write_hook(
        &workspace,
        DEFINITION,
        "printf '{\"answer\": \"refuse\", \"reason\": \"token %s thread %s\"}' \
         \"${LUCIDOS_AGENT_ORIGIN_TOKEN%%.*}\" \"${LUCIDOS_THREAD_ID:-none}\"\n",
    );
    let armed = armed(&workspace);
    engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::EventHookArmed {
            slug: armed.slug,
            event_type: armed.event_type,
            content_hash: armed.content_hash,
            actor: None,
        }))
        .await
        .unwrap();
    let (serving, serving_rx) = tokio::sync::watch::channel(false);
    engine
        .event_bus
        .install_event_hook_asker(Arc::new(WorkspaceHookAsker::new(&engine, serving_rx)))
        .await
        .unwrap();

    let asking = engine.clone();
    let mut ask = tokio::spawn(async move {
        asking
            .event_bus
            .ask_hooks(
                uuid::Uuid::new_v4(),
                &lucidos_core::test_support::message_received("hello"),
                &EventMeta::NONE,
            )
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), &mut ask)
            .await
            .is_err(),
        "no hook script runs before the engine serves"
    );
    assert_eq!(
        lucidos_core::test_support::hook_refusals(&pool, SLUG).await,
        0
    );
    serving.send(true).unwrap();
    let refusal = ask.await.unwrap().unwrap_err();
    assert_eq!(refusal.slug.as_deref(), Some(SLUG));
    assert_eq!(refusal.cause, RefusalCause::Refused);
    assert_eq!(refusal.reason, "token -@0@gate@- thread none");
    assert_eq!(
        lucidos_core::test_support::hook_refusals(&pool, SLUG).await,
        1
    );
    let copies = workspace.join(crate::staged::HOOK_RUNS_SUBPATH);
    assert_eq!(std::fs::read_dir(&copies).unwrap().count(), 0, "{copies:?}");

    lucidos_core::test_support::teardown_test_db(&db).await;
}
