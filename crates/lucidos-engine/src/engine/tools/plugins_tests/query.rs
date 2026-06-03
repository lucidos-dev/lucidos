//! Plugin-query resolution, file-ownership guard, normalization, and
//! check-plugin-updates survey tests.


use super::helpers::*;
use super::*;


use crate::engine::event_bus::EventBus;
use crate::test_support::{setup_test_db, teardown_test_db};

// ---- Plugin-query resolution + delete-guard tests -------------------
//
// Regression for the bug where an LLM picked the app *folder*
// (`anti-sycophancy-critique`) as the `uninstall_plugin` id, the lookup
// failed because the registered plugin id was `no-role-playing`, and the
// agent then bypassed the panel by calling `delete_file` on each path
// individually. Two behaviors must hold afterwards:
//   1. `resolve_plugin_query` accepts the id, the manifest name, OR an
//      app folder name (case-insensitive, dash-normalized) and returns
//      the canonical id.
//   2. `find_plugin_owning_file` returns `Some(id)` for any path that a
//      currently-installed plugin owns, so `delete_file` can refuse and
//      route the agent back through the confirm panel.

const APP_FIXTURE_MANIFEST: &str = r#"
id = "no-role-playing"
version = "0.1.2"
name = "No role playing"
description = "test"
"#;

/// Build a fixture archive whose manifest id (`no-role-playing`) is
/// deliberately different from its app folder (`anti-sycophancy-critique`)
/// — the exact mismatch from the original bug report.
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

/// Install the app-fixture into `scratch` and return the workspace root.
async fn install_app_fixture(scratch: &Path, bus: &dyn EventBusEmitter) {
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_app_fixture_archive(&archive_dir);
    let unpacked = extract_to(&archive_dir, &archive);
    install_from_unpacked_with_bus(scratch, bus, &unpacked, SourceType::Archive, false, None)
        .await
        .expect("install app fixture must succeed");
}

#[tokio::test]
async fn resolve_plugin_query_matches_canonical_id() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let scratch = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let id = super::resolve_plugin_query(&pool, "no-role-playing")
        .await
        .expect("exact id must resolve");
    assert_eq!(id, "no-role-playing");

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_plugin_query_matches_manifest_name_case_insensitive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let scratch = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    // Manifest name is "No role playing" — same words, mixed casing,
    // and a trailing space all resolve to the canonical id.
    for q in ["No role playing", "no role playing", "NO ROLE PLAYING ", "no-role-playing"] {
        let id = super::resolve_plugin_query(&pool, q)
            .await
            .unwrap_or_else(|e| panic!("query {q:?} must resolve: {e:?}"));
        assert_eq!(id, "no-role-playing", "query {q:?}");
    }

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_plugin_query_matches_app_folder_name() {
    // The original bug: agent passed `anti-sycophancy-critique` (the app
    // folder) instead of `no-role-playing` (the plugin id). The fix must
    // accept either.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let scratch = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let id = super::resolve_plugin_query(&pool, "anti-sycophancy-critique")
        .await
        .expect("app folder name must resolve");
    assert_eq!(id, "no-role-playing");

    // Same with a free-form re-spacing.
    let id = super::resolve_plugin_query(&pool, "Anti Sycophancy Critique")
        .await
        .expect("re-spaced app folder must resolve");
    assert_eq!(id, "no-role-playing");

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_plugin_query_unknown_returns_not_installed_error() {
    let (pool, db_name) = setup_test_db().await;
    let scratch = fresh_workspace();

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

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_plugin_query_ambiguous_lists_candidates() {
    // Two installed plugins share the manifest name "Demo" — a plain
    // `Demo` query matches both, must error and list both ids so the
    // agent can pick one.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let scratch = fresh_workspace();

    for id in ["demo-one", "demo-two"] {
        let archive_dir = scratch.join(format!("archive-{}", id));
        std::fs::create_dir_all(&archive_dir).unwrap();
        let manifest = format!(
            "id = \"{id}\"\nversion = \"0.1.0\"\nname = \"Demo\"\ndescription = \"x\"\n"
        );
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
            SourceType::Archive,
            false,
            None,
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

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_plugin_query_ignores_uninstalled_plugin() {
    // An uninstall hides the install record from `latest_install`; the
    // resolver must respect that and report not-installed for both id
    // and folder queries.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let scratch = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let sentinel = super::prepare_uninstall_plugin(&scratch, &pool, &pending, "no-role-playing")
        .await;
    assert!(sentinel.starts_with(PLUGIN_UNINSTALL_REQUEST_PREFIX), "sentinel: {sentinel}");
    let uninstall_id = pending.lock().unwrap().keys().next().cloned().unwrap();
    let entry = pending.lock().unwrap().remove(&uninstall_id).unwrap();
    super::uninstall_with_bus(&scratch, &bus, &entry, None)
        .await
        .expect("uninstall must succeed");

    for q in ["no-role-playing", "anti-sycophancy-critique", "No role playing"] {
        let err = super::resolve_plugin_query(&pool, q)
            .await
            .expect_err("uninstalled plugin must not resolve");
        assert!(
            err.to_string().contains("not currently installed"),
            "query {q:?} after uninstall: {err:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn prepare_uninstall_plugin_resolves_via_app_folder() {
    // End-to-end version of the bug: the original install registered
    // `no-role-playing` but the agent called `prepare_uninstall_plugin`
    // with `anti-sycophancy-critique`. With the resolver in place, the
    // sentinel must come back pointing at the canonical id and listing
    // the recorded files.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let scratch = fresh_workspace();
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

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn find_plugin_owning_file_returns_owner_for_installed_path() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let scratch = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let owner = super::find_plugin_owning_file(&pool, "apps/anti-sycophancy-critique/index.html")
        .await
        .expect("query must succeed")
        .expect("owner must be found for an installed file");
    assert_eq!(owner.plugin_id, "no-role-playing");
    assert_eq!(owner.plugin_name, "No role playing");

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn find_plugin_owning_file_returns_none_for_unrelated_paths() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let scratch = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    // A user-authored file the plugin doesn't own — must not be guarded.
    let owner = super::find_plugin_owning_file(&pool, "artifacts/my-notes.md")
        .await
        .expect("query must succeed");
    assert!(owner.is_none(), "unrelated paths must not resolve to an owner");

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn find_plugin_owning_file_returns_none_after_uninstall() {
    // Once the plugin is uninstalled, its old paths are no longer guarded
    // — the user (or recovery agent) is free to delete leftover empty
    // directories etc. without the engine refusing.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let scratch = fresh_workspace();
    install_app_fixture(&scratch, &bus).await;

    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let _ = super::prepare_uninstall_plugin(&scratch, &pool, &pending, "no-role-playing").await;
    let uninstall_id = pending.lock().unwrap().keys().next().cloned().unwrap();
    let entry = pending.lock().unwrap().remove(&uninstall_id).unwrap();
    super::uninstall_with_bus(&scratch, &bus, &entry, None)
        .await
        .expect("uninstall must succeed");

    let owner =
        super::find_plugin_owning_file(&pool, "apps/anti-sycophancy-critique/index.html")
            .await
            .expect("query must succeed");
    assert!(
        owner.is_none(),
        "uninstalled plugin's files must no longer be guarded"
    );

    let _ = std::fs::remove_dir_all(&scratch);
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

/// Regression: `check_plugin_updates(None)` must not surface a plugin once
/// it's been uninstalled. Drives the same flow as the LLM tool — install,
/// uninstall, then survey — and asserts the survey is empty.
#[tokio::test]
async fn check_plugin_updates_skips_uninstalled_plugin() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");

    install_from_source_with_bus(&scratch, &bus, archive.to_str().unwrap(), false)
        .await
        .expect("install must succeed");

    // Uninstall via the production code path (prepare → confirm).
    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let _ = prepare_uninstall_plugin(&scratch, &pool, &pending, "fixture-plugin").await;
    let uninstall_id = pending.lock().unwrap().keys().next().cloned().unwrap();
    let entry = pending.lock().unwrap().remove(&uninstall_id).unwrap();
    uninstall_with_bus(&scratch, &bus, &entry, None)
        .await
        .expect("uninstall must succeed");

    // Survey: no installed plugins → empty report.
    let report_json = check_plugin_updates_impl(&scratch, &pool, None).await;
    let report: Vec<serde_json::Value> =
        serde_json::from_str(&report_json).expect("report parses as JSON");
    assert!(
        report.is_empty(),
        "after uninstall, check_plugin_updates(None) must not list the plugin; got: {}",
        report_json
    );

    // Single-id check: explicit not-installed shape, not a stale install record.
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

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

/// Regression: legacy `PluginInstalled` events from before the
/// `aggregate_id` projection was fixed have `aggregate_id = 'unknown'` even
/// though the payload's manifest carries the real id. A subsequent
/// `PluginUninstalled` (correctly stamped with the real id) must still be
/// recognized as superseding the install — projection has to compare on the
/// canonical id from the payload, not on `aggregate_id`. Reproduces the
/// 2026-05-13 "browser-learning v0.1.0 still showing as installed" bug.
#[tokio::test]
async fn check_plugin_updates_ignores_install_with_legacy_unknown_aggregate_id() {
    let (pool, db_name) = setup_test_db().await;

    // Legacy install: aggregate_id stamped 'unknown', payload manifest has real id.
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

    // Correct uninstall: aggregate_id matches the canonical id.
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

    let scratch = fresh_workspace();

    // Survey: legacy 'unknown' install + correct uninstall → projection
    // must match by canonical id and report nothing installed.
    let report_json = check_plugin_updates_impl(&scratch, &pool, None).await;
    let report: Vec<serde_json::Value> =
        serde_json::from_str(&report_json).expect("report parses as JSON");
    assert!(
        report.is_empty(),
        "legacy 'unknown' install superseded by canonical-id uninstall must not appear in survey; got: {}",
        report_json
    );

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

/// Defensive cross-check: even if the registry projection drifted and
/// reported a plugin as installed, surveys must skip plugins whose recorded
/// files are all gone from disk. Catches future bugs where an uninstall
/// path forgets to emit the event.
#[tokio::test]
async fn check_plugin_updates_skips_plugin_when_files_are_all_missing_from_disk() {
    let (pool, db_name) = setup_test_db().await;

    // Synthesize an install event but never write the files to disk.
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

    let scratch = fresh_workspace();

    // Survey: registry says installed, but no files on disk → skip.
    let report_json = check_plugin_updates_impl(&scratch, &pool, None).await;
    let report: Vec<serde_json::Value> =
        serde_json::from_str(&report_json).expect("report parses as JSON");
    assert!(
        report.is_empty(),
        "plugin with all recorded files missing from disk must be skipped in survey; got: {}",
        report_json
    );

    // Single-id mode still honors the explicit ask (defensive skip is
    // survey-only — a direct check should still surface the registry state).
    let single_json =
        check_plugin_updates_impl(&scratch, &pool, Some("ghost-plugin".to_string())).await;
    let single: Vec<serde_json::Value> = serde_json::from_str(&single_json).unwrap();
    assert_eq!(single.len(), 1);
    assert_eq!(single[0]["id"], "ghost-plugin");

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}
