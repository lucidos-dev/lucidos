use super::helpers::build_archive;
use super::*;
use crate::core::event_hooks::{armed_hooks, EventHooks, HookListing, HookedEventKind};
use crate::test_support::{boot_mock_engine, setup_test_db, teardown_test_db};

struct TestHooks;

impl EventHooks for TestHooks {
    fn list_definitions(&self, workspace: &Path) -> Result<Vec<HookListing>, String> {
        self.list_at(&workspace.join(DATA_DIR).join("hooks"))
    }

    fn list_at(&self, root: &Path) -> Result<Vec<HookListing>, String> {
        let Ok(entries) = std::fs::read_dir(root) else {
            return Ok(Vec::new());
        };
        entries
            .map(|entry| {
                let entry = entry.map_err(|e| e.to_string())?;
                let slug = entry.file_name().into_string().unwrap();
                let text = std::fs::read_to_string(entry.path().join("hook.toml"))
                    .map_err(|e| e.to_string())?;
                let definition = toml::from_str(&text).map_err(|e| format!("{e}"))?;
                let script =
                    std::fs::read(entry.path().join("check.py")).map_err(|e| e.to_string())?;
                let content_hash = git2::Oid::hash_object(
                    git2::ObjectType::Blob,
                    &[text.as_bytes(), &script].concat(),
                )
                .unwrap()
                .to_string();
                Ok(HookListing::Valid {
                    slug,
                    event_kind: HookedEventKind::Thread,
                    definition,
                    content_hash,
                })
            })
            .collect()
    }
}

fn archive(
    root: &Path,
    id: &str,
    version: &str,
    hook: Option<(&str, &[u8])>,
) -> std::path::PathBuf {
    let manifest = format!(
        "id = '{id}'\nversion = '{version}'\nname = 'Gate pack'\ndescription = 'A gate.'\n"
    );
    let mut files: Vec<(&str, &[u8])> = vec![("knowhow/gate-guide.md", b"Gate guide")];
    let definition = hook.map(|(event, script)| {
        files.push(("hooks/gate/check.py", script));
        format!("name = 'Gate'\nevent_type = '{event}'\nscript = 'check.py'\n")
    });
    if let Some(definition) = &definition {
        files.push(("hooks/gate/hook.toml", definition.as_bytes()));
    }
    build_archive(
        root,
        &format!("{id}-{version}.lucidos-plugin"),
        &manifest,
        &files,
    )
}

async fn stage(engine: &LucidosEngine, source: &Path) -> serde_json::Value {
    let staged = stage_install_request(engine, source.to_str().unwrap()).await;
    serde_json::from_str(
        staged
            .strip_prefix(PLUGIN_INSTALL_REQUEST_PREFIX)
            .unwrap_or_else(|| panic!("{staged}")),
    )
    .unwrap()
}

async fn confirm(engine: &LucidosEngine, preview: &serde_json::Value) {
    confirm_pending_install(
        engine,
        preview["install_id"].as_str().unwrap(),
        true,
        true,
        None,
    )
    .await
    .unwrap();
}

async fn stage_uninstall(engine: &LucidosEngine) -> serde_json::Value {
    let staged = stage_uninstall_request(engine, "gate-pack").await;
    serde_json::from_str(
        staged
            .strip_prefix(PLUGIN_UNINSTALL_REQUEST_PREFIX)
            .unwrap_or_else(|| panic!("{staged}")),
    )
    .unwrap()
}

#[tokio::test]
async fn plugin_hooks_need_owner_confirmation_and_keep_the_disarmed_choice_until_widened() {
    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    engine
        .event_hook_catalog
        .set(std::sync::Arc::new(TestHooks))
        .ok()
        .unwrap();
    let source = tempfile::tempdir().unwrap();
    let stage_version = |version: &str, event_type: &str, script: &[u8]| {
        archive(
            source.path(),
            "gate-pack",
            version,
            Some((event_type, script)),
        )
    };

    let install = stage(
        &engine,
        &stage_version("1.0.0", "MessageReceived", b"# check"),
    )
    .await;
    let id = install["install_id"].as_str().unwrap();
    assert_eq!(install["hooks"][0]["event_type"], "MessageReceived");
    assert!(!workspace.path().join("data/hooks/gate").exists());
    assert!(armed_hooks(&pool).await.unwrap().is_empty());
    assert!(confirm_pending_install(&engine, id, true, false, None)
        .await
        .is_err());
    confirm(&engine, &install).await;
    assert_eq!(
        armed_hooks(&pool).await.unwrap()[0].event_type,
        "MessageReceived"
    );

    let first_hash = armed_hooks(&pool).await.unwrap()[0].content_hash.clone();
    let changed = stage(
        &engine,
        &stage_version("1.1.0", "MessageReceived", b"# revised"),
    )
    .await;
    assert_eq!(
        armed_hooks(&pool).await.unwrap()[0].content_hash,
        first_hash
    );
    confirm(&engine, &changed).await;
    let revised_hash = changed["hooks"][0]["content_hash"].as_str().unwrap();
    assert_ne!(first_hash, revised_hash);
    assert_eq!(
        armed_hooks(&pool).await.unwrap()[0].content_hash,
        revised_hash
    );
    assert_eq!(
        event_hook_plugin_ids(&pool).await.unwrap()["gate"],
        "gate-pack"
    );

    engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::EventHookDisarmed {
            slug: "gate".into(),
            actor: None,
        }))
        .await
        .unwrap();
    let unchanged = stage(
        &engine,
        &stage_version("1.2.0", "MessageReceived", b"# revised"),
    )
    .await;
    confirm(&engine, &unchanged).await;
    assert!(armed_hooks(&pool).await.unwrap().is_empty());

    let widened = stage(
        &engine,
        &stage_version("1.3.0", "ChangeProposed", b"# revised"),
    )
    .await;
    assert_eq!(widened["hooks"][0]["event_type"], "ChangeProposed");
    assert!(armed_hooks(&pool).await.unwrap().is_empty());
    confirm(&engine, &widened).await;
    assert_eq!(
        armed_hooks(&pool).await.unwrap()[0].event_type,
        "ChangeProposed"
    );

    engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::EventHookArmed {
            slug: "another".into(),
            event_type: "MessageReceived".into(),
            content_hash: "unrelated".into(),
            actor: None,
        }))
        .await
        .unwrap();
    let preview = stage_uninstall(&engine).await;
    let uninstall_id = preview["uninstall_id"].as_str().unwrap();
    assert!(
        confirm_pending_uninstall(&engine, uninstall_id, false, None)
            .await
            .is_err()
    );
    confirm_pending_uninstall(&engine, uninstall_id, true, None)
        .await
        .unwrap();
    assert!(!workspace.path().join("data/hooks/gate").exists());
    let remaining = armed_hooks(&pool).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].slug, "another");
    assert!(event_hook_plugin_ids(&pool).await.unwrap().is_empty());
    drop(engine);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn plugin_hooks_cannot_claim_a_workspace_hook() {
    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    engine
        .event_hook_catalog
        .set(std::sync::Arc::new(TestHooks))
        .ok()
        .unwrap();
    let source = tempfile::tempdir().unwrap();
    let archive = archive(
        source.path(),
        "gate-pack",
        "1.0.0",
        Some(("MessageReceived", b"# check")),
    );
    let install = stage(&engine, &archive).await;
    let local = workspace.path().join("data/hooks/gate");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("check.py"), "owned by workspace").unwrap();
    assert!(confirm_pending_install(
        &engine,
        install["install_id"].as_str().unwrap(),
        true,
        true,
        None,
    )
    .await
    .is_err());
    assert_eq!(
        std::fs::read_to_string(local.join("check.py")).unwrap(),
        "owned by workspace"
    );
    assert!(armed_hooks(&pool).await.unwrap().is_empty());
    drop(engine);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn plugin_hooks_reject_an_edit_to_the_staged_content_after_review() {
    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    engine
        .event_hook_catalog
        .set(std::sync::Arc::new(TestHooks))
        .ok()
        .unwrap();
    let source = tempfile::tempdir().unwrap();
    let archive = archive(
        source.path(),
        "gate-pack",
        "1.0.0",
        Some(("MessageReceived", b"# check")),
    );
    let install = stage(&engine, &archive).await;
    let id = install["install_id"].as_str().unwrap();
    let root = engine.pending_installs.lock().unwrap()[id]
        .plugin_root
        .clone();
    std::fs::write(root.join("hooks/gate/check.py"), "# changed").unwrap();
    let Err(error) = confirm_pending_install(&engine, id, true, true, None).await else {
        panic!("the staged hook changed");
    };
    assert!(error.contains("changed after review"), "{error}");
    assert!(!workspace.path().join("data/hooks/gate").exists());
    assert!(armed_hooks(&pool).await.unwrap().is_empty());
    drop(engine);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn plugin_hooks_reject_a_stale_uninstall_after_a_same_version_reinstall() {
    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    engine
        .event_hook_catalog
        .set(std::sync::Arc::new(TestHooks))
        .ok()
        .unwrap();
    let source = tempfile::tempdir().unwrap();
    let first = archive(
        source.path(),
        "gate-pack",
        "1.0.0",
        Some(("MessageReceived", b"# check")),
    );
    confirm(&engine, &stage(&engine, &first).await).await;
    let uninstall = stage_uninstall(&engine).await;

    let second = archive(
        source.path(),
        "gate-pack",
        "1.0.0",
        Some(("ChangeProposed", b"# check")),
    );
    confirm(&engine, &stage(&engine, &second).await).await;
    let result = confirm_pending_uninstall(
        &engine,
        uninstall["uninstall_id"].as_str().unwrap(),
        true,
        None,
    )
    .await;
    assert!(matches!(result, Err(error) if error.contains("changed after review")));
    assert!(workspace.path().join("data/hooks/gate/hook.toml").exists());
    assert_eq!(
        armed_hooks(&pool).await.unwrap()[0].event_type,
        "ChangeProposed"
    );
    drop(engine);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn plugin_hooks_report_when_an_update_leaves_a_stale_arming() {
    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    engine
        .event_hook_catalog
        .set(std::sync::Arc::new(TestHooks))
        .ok()
        .unwrap();
    let source = tempfile::tempdir().unwrap();
    let bundle = archive(
        source.path(),
        "gate-pack",
        "1.0.0",
        Some(("MessageReceived", b"# shipped\n")),
    );
    confirm(&engine, &stage(&engine, &bundle).await).await;
    let script = workspace.path().join("data/hooks/gate/check.py");
    std::fs::write(&script, "# edited\n").unwrap();
    let rows = TestHooks.list_definitions(workspace.path()).unwrap();
    let edited_hash = rows[0].content_hash().unwrap();
    engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::EventHookArmed {
            slug: "gate".into(),
            event_type: "MessageReceived".into(),
            content_hash: edited_hash.into(),
            actor: None,
        }))
        .await
        .unwrap();

    let staged = stage(&engine, &bundle).await;
    let outcome = confirm_pending_install(
        &engine,
        staged["install_id"].as_str().unwrap(),
        false,
        true,
        None,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&script).unwrap(), "# shipped\n");
    assert_eq!(
        armed_hooks(&pool).await.unwrap()[0].content_hash,
        edited_hash
    );
    assert_eq!(outcome.hooks_needing_review, ["gate"]);

    let drop_hook = archive(source.path(), "gate-pack", "1.1.0", None);
    let update = stage(&engine, &drop_hook).await;
    assert_eq!(update["hook_confirmation_required"], true);
    confirm(&engine, &update).await;
    assert!(!script.exists());
    assert!(armed_hooks(&pool).await.unwrap().is_empty());
    assert!(event_hook_plugin_ids(&pool).await.unwrap().is_empty());
    drop(engine);
    teardown_test_db(&db).await;
}
