use std::path::PathBuf;

use super::helpers::*;
use super::*;

use crate::engine::event_bus::EventBus;
use crate::test_support::{setup_test_db, teardown_test_db};

const APP_FIXTURE_MANIFEST: &str = r#"
id = "no-role-playing"
version = "0.1.2"
name = "No role playing"
description = "test"
"#;

fn build_app_fixture_archive(tmp: &Path) -> PathBuf {
    build_archive(
        tmp,
        "no-role-playing.lucidos-plugin",
        APP_FIXTURE_MANIFEST,
        &[
            (
                "apps/anti-sycophancy-critique/index.html",
                b"<html>app</html>",
            ),
            (
                "apps/anti-sycophancy-critique/manifest.json",
                br#"{"id":"anti-sycophancy-critique","name":"No role playing"}"#,
            ),
        ],
    )
}

async fn install_app_fixture(scratch: &Path, bus: &dyn EventBusEmitter) {
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_app_fixture_archive(&archive_dir);
    let unpacked = extract_to(&archive_dir, &archive);
    install_from_unpacked_with_bus(
        scratch,
        bus,
        &unpacked,
        InstallContext::plain(SourceType::Archive, false),
    )
    .await
    .expect("install app fixture must succeed");
}

#[tokio::test]
async fn resolve_plugin_query_matches_canonical_id() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let id = super::resolve_plugin_query(&pool, "no-role-playing")
        .await
        .expect("exact id must resolve");
    assert_eq!(id, "no-role-playing");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_plugin_query_matches_manifest_name_case_insensitive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    for q in [
        "No role playing",
        "no role playing",
        "NO ROLE PLAYING ",
        "no-role-playing",
    ] {
        let id = super::resolve_plugin_query(&pool, q)
            .await
            .unwrap_or_else(|e| panic!("query {q:?} must resolve: {e:?}"));
        assert_eq!(id, "no-role-playing", "query {q:?}");
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_plugin_query_matches_app_folder_name() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let id = super::resolve_plugin_query(&pool, "anti-sycophancy-critique")
        .await
        .expect("app folder name must resolve");
    assert_eq!(id, "no-role-playing");

    let id = super::resolve_plugin_query(&pool, "Anti Sycophancy Critique")
        .await
        .expect("re-spaced app folder must resolve");
    assert_eq!(id, "no-role-playing");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_plugin_query_unknown_returns_not_installed_error() {
    let (pool, db_name) = setup_test_db().await;

    let err = super::resolve_plugin_query(&pool, "no-such-plugin")
        .await
        .expect_err("unknown query must error");
    let msg = err.to_string();
    assert!(
        msg.contains("not currently installed"),
        "unknown query error must mention not-installed, got: {msg}"
    );
    assert!(
        msg.contains("no-such-plugin"),
        "error must echo the query, got: {msg}"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_plugin_query_ambiguous_lists_candidates() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();

    for id in ["demo-one", "demo-two"] {
        let archive_dir = scratch.join(format!("archive-{}", id));
        std::fs::create_dir_all(&archive_dir).unwrap();
        let manifest =
            format!("id = \"{id}\"\nversion = \"0.1.0\"\nname = \"Demo\"\ndescription = \"x\"\n");
        let archive = build_archive(
            &archive_dir,
            &format!("{}.lucidos-plugin", id),
            &manifest,
            &[(&format!("knowhow/{}.md", id), b"x")],
        );
        let unpacked = extract_to(&archive_dir, &archive);
        install_from_unpacked_with_bus(
            &scratch,
            &bus,
            &unpacked,
            InstallContext::plain(SourceType::Archive, false),
        )
        .await
        .expect("install must succeed");
    }

    let err = super::resolve_plugin_query(&pool, "Demo")
        .await
        .expect_err("ambiguous query must error");
    let msg = err.to_string();
    assert!(
        msg.contains("multiple plugins") || msg.contains("ambiguous"),
        "ambiguous error must say so, got: {msg}"
    );
    assert!(msg.contains("demo-one"), "error must list demo-one: {msg}");
    assert!(msg.contains("demo-two"), "error must list demo-two: {msg}");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_plugin_query_ignores_uninstalled_plugin() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let sentinel =
        super::prepare_uninstall_plugin(&scratch, &pool, &pending, "no-role-playing").await;
    assert!(
        sentinel.starts_with(PLUGIN_UNINSTALL_REQUEST_PREFIX),
        "sentinel: {sentinel}"
    );
    let uninstall_id = pending.lock().unwrap().keys().next().cloned().unwrap();
    let entry = pending.lock().unwrap().remove(&uninstall_id).unwrap();
    super::uninstall_with_bus(&scratch, &bus, &entry, &[], None, vec![])
        .await
        .expect("uninstall must succeed");

    for q in [
        "no-role-playing",
        "anti-sycophancy-critique",
        "No role playing",
    ] {
        let err = super::resolve_plugin_query(&pool, q)
            .await
            .expect_err("uninstalled plugin must not resolve");
        assert!(
            err.to_string().contains("not currently installed"),
            "query {q:?} after uninstall: {err:?}"
        );
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn prepare_uninstall_plugin_resolves_via_app_folder() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let result =
        super::prepare_uninstall_plugin(&scratch, &pool, &pending, "anti-sycophancy-critique")
            .await;
    assert!(
        result.starts_with(PLUGIN_UNINSTALL_REQUEST_PREFIX),
        "expected sentinel, got: {result}"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&result[PLUGIN_UNINSTALL_REQUEST_PREFIX.len()..]).unwrap();
    assert_eq!(payload["plugin_id"], "no-role-playing");
    assert_eq!(payload["plugin_name"], "No role playing");
    let present: Vec<&str> = payload["files_present"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(present.contains(&"apps/anti-sycophancy-critique/index.html"));
    assert!(present.contains(&"apps/anti-sycophancy-critique/manifest.json"));

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn find_plugin_owning_file_returns_owner_for_installed_path() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let owner = super::find_plugin_owning_file(&pool, "apps/anti-sycophancy-critique/index.html")
        .await
        .expect("query must succeed")
        .expect("owner must be found for an installed file");
    assert_eq!(owner.plugin_id, "no-role-playing");
    assert_eq!(owner.plugin_name, "No role playing");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn find_plugin_owning_file_returns_none_for_unrelated_paths() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let owner = super::find_plugin_owning_file(&pool, "artifacts/my-notes.md")
        .await
        .expect("query must succeed");
    assert!(
        owner.is_none(),
        "unrelated paths must not resolve to an owner"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn find_plugin_owning_file_returns_none_after_uninstall() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let _ = super::prepare_uninstall_plugin(&scratch, &pool, &pending, "no-role-playing").await;
    let uninstall_id = pending.lock().unwrap().keys().next().cloned().unwrap();
    let entry = pending.lock().unwrap().remove(&uninstall_id).unwrap();
    super::uninstall_with_bus(&scratch, &bus, &entry, &[], None, vec![])
        .await
        .expect("uninstall must succeed");

    let owner = super::find_plugin_owning_file(&pool, "apps/anti-sycophancy-critique/index.html")
        .await
        .expect("query must succeed");
    assert!(
        owner.is_none(),
        "uninstalled plugin's files must no longer be guarded"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn find_plugin_owning_app_returns_owner_for_installed_app() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let owner = super::find_plugin_owning_app(&pool, "anti-sycophancy-critique")
        .await
        .expect("query must succeed")
        .expect("owner must be found for an installed app");
    assert_eq!(owner.plugin_id, "no-role-playing");
    assert_eq!(owner.plugin_name, "No role playing");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn find_plugin_owning_app_returns_none_for_standalone_app() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let owner = super::find_plugin_owning_app(&pool, "some-standalone-app")
        .await
        .expect("query must succeed");
    assert!(
        owner.is_none(),
        "standalone apps must not resolve to an owner"
    );

    let owner = super::find_plugin_owning_app(&pool, "anti-sycophancy")
        .await
        .expect("query must succeed");
    assert!(
        owner.is_none(),
        "a strict prefix of an owned app folder must not resolve to an owner"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn find_plugin_owning_app_returns_none_after_uninstall() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let _ = super::prepare_uninstall_plugin(&scratch, &pool, &pending, "no-role-playing").await;
    let uninstall_id = pending.lock().unwrap().keys().next().cloned().unwrap();
    let entry = pending.lock().unwrap().remove(&uninstall_id).unwrap();
    super::uninstall_with_bus(&scratch, &bus, &entry, &[], None, vec![])
        .await
        .expect("uninstall must succeed");

    let owner = super::find_plugin_owning_app(&pool, "anti-sycophancy-critique")
        .await
        .expect("query must succeed");
    assert!(
        owner.is_none(),
        "uninstalled plugin's app must no longer be guarded"
    );

    teardown_test_db(&db_name).await;
}

#[test]
fn normalize_plugin_query_collapses_case_whitespace_and_dashes() {
    use super::registry::normalize_plugin_query as n;
    assert_eq!(n("no-role-playing"), "no-role-playing");
    assert_eq!(n("No role playing"), "no-role-playing");
    assert_eq!(n("  No   Role   Playing  "), "no-role-playing");
    assert_eq!(n("anti_sycophancy_critique"), "anti-sycophancy-critique");
    assert_eq!(n("--FOO  BAR--"), "foo-bar");
    assert_eq!(n(""), "");
}

#[tokio::test]
async fn check_plugin_updates_skips_uninstalled_plugin() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");

    install_from_source_with_bus(&scratch, &bus, archive.to_str().unwrap(), false)
        .await
        .expect("install must succeed");

    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let _ = prepare_uninstall_plugin(&scratch, &pool, &pending, "fixture-plugin").await;
    let uninstall_id = pending.lock().unwrap().keys().next().cloned().unwrap();
    let entry = pending.lock().unwrap().remove(&uninstall_id).unwrap();
    uninstall_with_bus(&scratch, &bus, &entry, &[], None, vec![])
        .await
        .expect("uninstall must succeed");

    let report_json = check_plugin_updates_impl(&scratch, &pool, None).await;
    let report: Vec<serde_json::Value> =
        serde_json::from_str(&report_json).expect("report parses as JSON");
    assert!(
        report.is_empty(),
        "after uninstall, check_plugin_updates(None) must not list the plugin; got: {}",
        report_json
    );

    let single_json =
        check_plugin_updates_impl(&scratch, &pool, Some("fixture-plugin".to_string())).await;
    let single: Vec<serde_json::Value> = serde_json::from_str(&single_json).unwrap();
    assert_eq!(single.len(), 1);
    assert!(
        single[0]
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("not installed"),
        "single-id check must report not-installed; got: {}",
        single_json
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn check_plugin_updates_ignores_install_with_legacy_unknown_aggregate_id() {
    let (pool, db_name) = setup_test_db().await;

    let legacy_install_payload = serde_json::json!({
        "type": "PluginInstalled",
        "data": {
            "files": ["knowhow/browser-skills.md"],
            "manifest": {
                "files": ["knowhow/browser-skills.md"],
                "summary": "Installed Browser Learning v0.1.0",
                "manifest": {
                    "id": "browser-learning",
                    "name": "Browser Learning",
                    "version": "0.1.0",
                    "source": "https://github.com/lucidos-dev/plugins/tree/main/browser-learning",
                    "description": "test"
                },
                "source_type": "git",
                "installed_at": "2026-04-30T07:10:46Z"
            },
            "source_type": "git",
            "installed_at": "2026-04-30T07:10:46Z"
        }
    });
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
             VALUES ($1, 'PluginInstalled', $2, 'plugin', 'unknown')",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(&legacy_install_payload)
    .execute(&pool)
    .await
    .expect("insert legacy install");

    let uninstall_payload = serde_json::json!({
        "type": "PluginUninstalled",
        "data": {
            "id": "browser-learning",
            "version": "0.1.0",
            "files": ["knowhow/browser-skills.md"],
            "files_deleted": ["knowhow/browser-skills.md"],
            "files_missing": []
        }
    });
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
             VALUES ($1, 'PluginUninstalled', $2, 'plugin', 'browser-learning')",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(&uninstall_payload)
    .execute(&pool)
    .await
    .expect("insert uninstall");

    let (_tmp, scratch) = fresh_workspace();

    let report_json = check_plugin_updates_impl(&scratch, &pool, None).await;
    let report: Vec<serde_json::Value> =
        serde_json::from_str(&report_json).expect("report parses as JSON");
    assert!(
        report.is_empty(),
        "legacy 'unknown' install superseded by canonical-id uninstall must not appear in survey; got: {}",
        report_json
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn check_plugin_updates_skips_plugin_when_files_are_all_missing_from_disk() {
    let (pool, db_name) = setup_test_db().await;

    let install_payload = serde_json::json!({
        "type": "PluginInstalled",
        "data": {
            "files": ["knowhow/ghost.md", "triggers/ghost/ghost.md"],
            "manifest": {
                "files": ["knowhow/ghost.md", "triggers/ghost/ghost.md"],
                "summary": "Installed Ghost Plugin v0.1.0",
                "manifest": {
                    "id": "ghost-plugin",
                    "name": "Ghost Plugin",
                    "version": "0.1.0",
                    "source": "https://github.com/x/y",
                    "description": "test"
                },
                "source_type": "git",
                "installed_at": "2026-05-01T00:00:00Z"
            },
            "source_type": "git",
            "installed_at": "2026-05-01T00:00:00Z"
        }
    });
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
             VALUES ($1, 'PluginInstalled', $2, 'plugin', 'ghost-plugin')",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(&install_payload)
    .execute(&pool)
    .await
    .expect("insert install");

    let (_tmp, scratch) = fresh_workspace();

    let report_json = check_plugin_updates_impl(&scratch, &pool, None).await;
    let report: Vec<serde_json::Value> =
        serde_json::from_str(&report_json).expect("report parses as JSON");
    assert!(
        report.is_empty(),
        "plugin with all recorded files missing from disk must be skipped in survey; got: {}",
        report_json
    );

    let single_json =
        check_plugin_updates_impl(&scratch, &pool, Some("ghost-plugin".to_string())).await;
    let single: Vec<serde_json::Value> = serde_json::from_str(&single_json).unwrap();
    assert_eq!(single.len(), 1);
    assert_eq!(single[0]["id"], "ghost-plugin");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn installed_summary_surfaces_setup_thread_and_app_id() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();

    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_app_fixture_archive(&archive_dir);
    let unpacked = extract_to(&archive_dir, &archive);
    let setup_tid = uuid::Uuid::new_v4();
    install_from_unpacked_with_bus(
        &scratch,
        &bus,
        &unpacked,
        InstallContext {
            setup_thread_id: Some(setup_tid),
            ..InstallContext::plain(SourceType::Archive, false)
        },
    )
    .await
    .expect("install with a setup thread must succeed");

    let summaries = super::installed_plugin_summaries(&pool, &scratch)
        .await
        .expect("read installed summaries");
    let plugin = summaries
        .iter()
        .find(|p| p.id == "no-role-playing")
        .expect("installed plugin present in summaries");
    assert_eq!(
        plugin.setup_thread_id.as_deref(),
        Some(setup_tid.to_string().as_str()),
        "setup_thread_id must round-trip from the PluginInstalled payload"
    );
    assert_eq!(plugin.app_id.as_deref(), Some("anti-sycophancy-critique"));

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn installed_summary_omits_setup_and_app_when_absent() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");
    install_from_source_with_bus(&scratch, &bus, archive.to_str().unwrap(), false)
        .await
        .expect("install must succeed");

    let summaries = super::installed_plugin_summaries(&pool, &scratch)
        .await
        .expect("read installed summaries");
    let plugin = summaries
        .iter()
        .find(|p| p.id == "fixture-plugin")
        .expect("installed plugin present");
    assert_eq!(plugin.setup_thread_id, None);
    assert_eq!(
        plugin.app_id, None,
        "knowhow-only plugin has no app to open"
    );

    teardown_test_db(&db_name).await;
}

#[test]
fn primary_app_id_prefers_plugin_id_then_first_app() {
    use super::registry::primary_app_id as p;
    assert_eq!(
        p(&["knowhow/x.md".into(), "triggers/t/run.md".into()], "plug"),
        None
    );
    assert_eq!(
        p(&["apps/dashboard/index.html".into()], "plug"),
        Some("dashboard".to_string())
    );
    assert_eq!(
        p(
            &["apps/zeta/index.html".into(), "apps/plug/index.html".into(),],
            "plug"
        ),
        Some("plug".to_string())
    );
    assert_eq!(
        p(
            &[
                "apps/zeta/index.html".into(),
                "apps/alpha/index.html".into(),
            ],
            "plug"
        ),
        Some("alpha".to_string())
    );
}

#[tokio::test]
async fn a_legacy_unknown_aggregate_install_is_keyed_by_its_payload_id_and_uninstallable() {
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, scratch) = fresh_workspace();

    let install_payload = serde_json::json!({
        "type": "PluginInstalled",
        "data": {
            "files": ["knowhow/browser-skills.md"],
            "manifest": {
                "manifest": {
                    "id": "browser-learning",
                    "name": "Browser Learning",
                    "version": "0.1.0"
                }
            }
        }
    });
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
         VALUES ($1, 'PluginInstalled', $2, 'plugin', 'unknown')",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(&install_payload)
    .execute(&pool)
    .await
    .expect("insert legacy install");

    let listed: Vec<String> = installed_plugin_summaries(&pool, &scratch)
        .await
        .expect("summaries")
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(listed, vec!["browser-learning".to_string()]);

    let uninstall_payload = serde_json::json!({
        "type": "PluginUninstalled",
        "data": { "id": "browser-learning", "version": "0.1.0" }
    });
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
         VALUES ($1, 'PluginUninstalled', $2, 'plugin', 'browser-learning')",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(&uninstall_payload)
    .execute(&pool)
    .await
    .expect("insert uninstall");

    let after = installed_plugin_summaries(&pool, &scratch)
        .await
        .expect("summaries");
    assert!(
        after.is_empty(),
        "the uninstall must supersede the legacy row"
    );

    teardown_test_db(&db_name).await;
}

#[test]
fn plugin_owned_deletes_stay_guarded_at_both_call_sites() {
    let files = include_str!("../files.rs");
    let apps = include_str!("../../../api/apps.rs");
    assert!(
        files.contains("find_plugin_owning_file(&self.pool, path)"),
        "delete_file must refuse plugin-owned paths"
    );
    assert!(
        apps.contains("find_plugin_owning_app(&state.pool, &query.id)"),
        "delete_app must refuse plugin-installed apps"
    );
}
