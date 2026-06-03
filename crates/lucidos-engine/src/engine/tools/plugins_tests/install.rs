//! Plugin install flow tests: source install, prepare/cancel confirm flow,
//! setup-text handling, and end-to-end install / update / check via local
//! git + archive fixtures.


use super::helpers::*;
use super::*;


use crate::engine::event_bus::{EventBus, MockEventBus};
use crate::test_support::{setup_test_db, teardown_test_db};

/// `update_plugin(id)` core. Re-fetches the recorded `source`, compares
/// semver, and re-runs the install with `overwrite=true` when the remote
/// version is strictly greater. No-ops with a friendly message when already
/// at latest (including when the remote is older — intentional downgrades
/// aren't supported by `update_plugin`).
///
/// Test-only — production `update_plugin` routes through the dispatcher's
/// `prepare_install_request` so the user confirms in the install panel
/// before files are written.
async fn update_plugin_impl(
    workspace_path: &Path,
    bus: &dyn EventBusEmitter,
    pool: &sqlx::PgPool,
    id: &str,
) -> String {
    let installed = match latest_install(pool, id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            return format!(
                "Error: plugin '{}' is not currently installed (no PluginInstalled event, or already uninstalled)",
                id
            );
        }
        Err(e) => return format!("Error: read install record: {}", e),
    };

    let installed_version = installed.version().unwrap_or("unknown").to_string();
    let source = match installed.source() {
        Some(s) => s.to_string(),
        None => {
            return format!(
                "Error: installed manifest for '{}' is missing 'source' — cannot fetch latest",
                id
            );
        }
    };

    let remote = match fetch_remote_manifest(workspace_path, &source).await {
        Ok(m) => m,
        Err(e) => return format!("Error: fetch latest manifest: {}", e),
    };

    if compare_versions(&installed_version, &remote.version) == UpdateDecision::AlreadyLatest {
        return format!("Already at latest (v{})", installed_version);
    }

    match install_from_source_with_bus(workspace_path, bus, &source, true).await {
        Ok(msg) => msg,
        Err(e) => format!("Error: {}", e),
    }
}

fn build_sourceless_fixture(tmp: &Path) -> PathBuf {
    const SOURCELESS_MANIFEST: &str = r#"
id = "sourceless-plugin"
version = "0.1.0"
name = "Sourceless Plugin"
description = "test"
"#;
    build_archive(
        tmp,
        "sourceless.lucidos-plugin",
        SOURCELESS_MANIFEST,
        &[("knowhow/sourceless.md", b"---\nname: S\n---\nx")],
    )
}

#[tokio::test]
async fn install_without_source_succeeds_and_omits_from_in_summary() {
    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_sourceless_fixture(&archive_dir);
    let unpacked = extract_to(&archive_dir, &archive);

    let bus = MockEventBus::new();
    let (msg, _files) =
        install_from_unpacked_with_bus(&scratch, &bus, &unpacked, SourceType::Archive, false, None)
            .await
            .expect("install must succeed even with no source field");
    assert_eq!(msg, "Installed Sourceless Plugin v0.1.0 (1 files).");

    let events = bus.emitted_events();
    assert_eq!(events.len(), 1);
    match &events[0] {
        BusEvent::System(SystemEvent::PluginInstalled { manifest, .. }) => {
            let summary = manifest
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert_eq!(
                summary, "Installed Sourceless Plugin v0.1.0",
                "summary must not include 'from <source>' when no source is set"
            );
            let payload_source = manifest.get("manifest").and_then(|m| m.get("source"));
            assert!(
                payload_source.is_none(),
                "raw manifest in event payload must not contain a `source` key when the manifest omitted it (got: {:?})",
                payload_source
            );
        }
        other => panic!("expected PluginInstalled, got {:?}", other),
    }

    let _ = std::fs::remove_dir_all(&scratch);
}

// --- Confirm-flow: prepare_install_request + cancel_pending_install_with_bus ---

fn fresh_pending_map() -> std::sync::Arc<PendingInstallsMap> {
    std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn parse_sentinel_payload(s: &str) -> serde_json::Value {
    let json = s
        .strip_prefix(PLUGIN_INSTALL_REQUEST_PREFIX)
        .expect("must start with sentinel prefix");
    serde_json::from_str(json).expect("payload must be valid JSON")
}

#[test]
fn prepare_install_request_returns_sentinel_and_registers_pending() {
    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "preview-body");
    let pending = fresh_pending_map();

    let result = prepare_install_request(&scratch, &pending, archive.to_str().unwrap());

    assert!(
        result.starts_with(PLUGIN_INSTALL_REQUEST_PREFIX),
        "result must start with sentinel, got: {}",
        result
    );
    let payload = parse_sentinel_payload(&result);
    assert_eq!(payload["plugin_id"], "fixture-plugin");
    assert_eq!(payload["plugin_version"], "0.1.0");
    assert_eq!(payload["source_type"], "archive");
    assert_eq!(payload["source"], archive.to_string_lossy().as_ref());
    let files: Vec<&str> = payload["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(files.contains(&"knowhow/fixture.md"));
    assert!(files.contains(&"triggers/fixture/fixture.md"));
    assert!(payload["overwrites"].as_array().unwrap().is_empty());
    let install_id = payload["install_id"].as_str().unwrap();
    assert_eq!(pending.lock().unwrap().len(), 1);
    assert!(pending.lock().unwrap().contains_key(install_id));

    // Critically: nothing in data/ yet — the panel hasn't been confirmed.
    assert!(!scratch.join("data/knowhow/fixture.md").exists());

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn prepare_install_request_lists_overwrites_when_files_already_exist() {
    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");
    std::fs::create_dir_all(scratch.join("data/knowhow")).unwrap();
    std::fs::write(scratch.join("data/knowhow/fixture.md"), "existing").unwrap();
    let pending = fresh_pending_map();

    let result = prepare_install_request(&scratch, &pending, archive.to_str().unwrap());
    let payload = parse_sentinel_payload(&result);
    let overwrites: Vec<&str> = payload["overwrites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(overwrites, vec!["knowhow/fixture.md"]);
    // Existing file untouched until user clicks Confirm.
    assert_eq!(
        std::fs::read_to_string(scratch.join("data/knowhow/fixture.md")).unwrap(),
        "existing"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn prepare_install_request_returns_error_string_on_invalid_source() {
    let scratch = fresh_workspace();
    let pending = fresh_pending_map();

    let result = prepare_install_request(&scratch, &pending, "not-a-real-source");

    assert!(result.starts_with("Error:"), "got: {}", result);
    assert!(
        !result.contains(PLUGIN_INSTALL_REQUEST_PREFIX),
        "no sentinel on failure: {}",
        result
    );
    assert!(
        pending.lock().unwrap().is_empty(),
        "failed prepare must not register a pending entry"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

#[tokio::test]
async fn cancel_pending_install_emits_event_and_drops_staging() {
    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "to-cancel");
    let pending = fresh_pending_map();

    let result = prepare_install_request(&scratch, &pending, archive.to_str().unwrap());
    let payload = parse_sentinel_payload(&result);
    let install_id = payload["install_id"].as_str().unwrap().to_string();

    // Capture the staged plugin_root so we can confirm cleanup on cancel.
    let staged_root = pending
        .lock()
        .unwrap()
        .get(&install_id)
        .map(|e| e.plugin_root.clone())
        .unwrap();
    assert!(staged_root.is_dir(), "staged dir must exist before cancel");

    let bus = MockEventBus::new();
    cancel_pending_install_with_bus(&pending, &bus, &install_id, None)
        .await
        .expect("cancel must succeed for a known install_id");

    assert!(
        pending.lock().unwrap().is_empty(),
        "pending entry must be removed on cancel"
    );
    assert!(
        !staged_root.exists(),
        "staged dir must be cleaned up on cancel"
    );
    assert!(
        !scratch.join("data/knowhow/fixture.md").exists(),
        "cancel must NOT write any files to data/"
    );

    let events = bus.emitted_events();
    assert_eq!(events.len(), 1, "exactly one PluginInstallCanceled event");
    match &events[0] {
        BusEvent::System(SystemEvent::PluginInstallCanceled {
            id,
            version,
            source,
            source_type,
            ..
        }) => {
            assert_eq!(id, "fixture-plugin");
            assert_eq!(version, "0.1.0");
            assert_eq!(source, archive.to_str().unwrap());
            assert_eq!(source_type, "archive");
        }
        other => panic!("expected PluginInstallCanceled, got {:?}", other),
    }

    let _ = std::fs::remove_dir_all(&scratch);
}

#[tokio::test]
async fn cancel_pending_install_returns_error_for_unknown_id() {
    let pending = fresh_pending_map();
    let bus = MockEventBus::new();
    let err = cancel_pending_install_with_bus(&pending, &bus, "no-such-id", None)
        .await
        .expect_err("cancel must fail for unknown id");
    assert!(err.contains("no pending install"), "got: {}", err);
    assert!(bus.emitted_events().is_empty(), "no event on failed cancel");
}

#[tokio::test]
async fn install_appends_setup_text_when_manifest_declares_it() {
    const SETUP_MANIFEST: &str = r#"
id = "with-setup"
version = "0.1.0"
name = "With Setup"
description = "Plugin that needs post-install wiring"
setup = "Create a daily trigger that loads `knowhow/with-setup/run.md`. Suggested cron: `0 0 4 * * *`."
"#;
    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_archive(
        &archive_dir,
        "withsetup.lucidos-plugin",
        SETUP_MANIFEST,
        &[("knowhow/with-setup/run.md", b"---\nname: Run\n---\nx")],
    );
    let unpacked = extract_to(&archive_dir, &archive);

    let bus = MockEventBus::new();
    let (msg, _files) =
        install_from_unpacked_with_bus(&scratch, &bus, &unpacked, SourceType::Archive, false, None)
            .await
            .expect("install must succeed");

    assert!(
        msg.starts_with("Installed With Setup v0.1.0 (1 files)."),
        "install summary line must come first, got: {:?}",
        msg
    );
    assert!(
        msg.contains("Setup:"),
        "tool result must label the setup section so the LLM acts on it, got: {:?}",
        msg
    );
    assert!(
        msg.contains("Create a daily trigger that loads `knowhow/with-setup/run.md`. Suggested cron: `0 0 4 * * *`."),
        "tool result must include the verbatim setup text from the manifest, got: {:?}",
        msg
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

#[tokio::test]
async fn install_omits_setup_section_when_manifest_has_no_setup() {
    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "---\nname: F\n---\nx");
    let unpacked = extract_to(&archive_dir, &archive);

    let bus = MockEventBus::new();
    let (msg, _files) =
        install_from_unpacked_with_bus(&scratch, &bus, &unpacked, SourceType::Archive, false, None)
            .await
            .expect("install must succeed");

    assert_eq!(msg, "Installed Fixture Plugin v0.1.0 (2 files).");

    let _ = std::fs::remove_dir_all(&scratch);
}

// ---- DB-backed regression + lifecycle tests --------------------------
//
// These exercise the live `EventBus` + `latest_install` path that the four
// plugin tools (install / check_plugin_updates / update_plugin /
// uninstall_plugin) actually run through. The MockEventBus tests above
// assert event shape; these assert the round-trip from emit → DB →
// `InstalledRecord` works, which is where the "missing source" regression
// hid for so long.

/// Run a git command in `dir`, panicking on any failure. Used by the
/// `file://` git-source tests where we stand up a bare repo locally so
/// `update_plugin` has somewhere to re-fetch from without depending on
/// the live `lucidos-dev/plugins` GitHub repo.
fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap_or_else(|e| panic!("git {} failed: {}", args.join(" "), e));
    assert!(
        out.status.success(),
        "git {} in {:?} failed: {}",
        args.join(" "),
        dir,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Stand up a bare git repo at `<scratch>/<name>.git` plus a working
/// clone at `<scratch>/<name>-work/`, then commit `manifest.toml` and a
/// `knowhow/<id>.md` file referencing `version`. Returns the bare repo
/// path and the work tree so the caller can bump the version later.
fn make_local_git_plugin(
    scratch: &Path,
    name: &str,
    id: &str,
    version: &str,
    knowhow_body: &str,
) -> (PathBuf, PathBuf) {
    let bare = scratch.join(format!("{}.git", name));
    let work = scratch.join(format!("{}-work", name));
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    git(&bare, &["init", "--bare", "--initial-branch=main"]);

    let bare_url = format!("file://{}", bare.display());
    let manifest = format!(
        r#"id = "{id}"
version = "{version}"
name = "Local Git Plugin"
description = "test"
source = "{bare_url}"
"#,
        id = id,
        version = version,
        bare_url = bare_url,
    );
    std::fs::write(work.join("manifest.toml"), manifest).unwrap();
    std::fs::create_dir_all(work.join("knowhow")).unwrap();
    std::fs::write(work.join(format!("knowhow/{}.md", id)), knowhow_body).unwrap();

    git(&work, &["init", "--initial-branch=main"]);
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "initial"]);
    git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&work, &["push", "origin", "main"]);
    (bare, work)
}

/// Replace the manifest version + knowhow body in an existing work tree
/// and push to the bare repo. Used by the `update_plugin` test.
fn bump_local_git_plugin(
    work: &Path,
    id: &str,
    old_version: &str,
    new_version: &str,
    new_body: &str,
) {
    let manifest = std::fs::read_to_string(work.join("manifest.toml")).unwrap();
    let updated = manifest.replace(
        &format!("version = \"{}\"", old_version),
        &format!("version = \"{}\"", new_version),
    );
    std::fs::write(work.join("manifest.toml"), updated).unwrap();
    std::fs::write(work.join(format!("knowhow/{}.md", id)), new_body).unwrap();
    git(work, &["add", "."]);
    git(work, &["commit", "-m", "bump"]);
    git(work, &["push", "origin", "main"]);
}

/// Regression test for the "installed manifest is missing 'source'" bug.
///
/// The PluginInstalled event payload nests the manifest inside the
/// SystemEvent's `manifest` field (see `install_from_unpacked_with_bus`),
/// and the persisted JSONB column wraps everything in serde's
/// `{type, data}` envelope. Earlier versions of `InstalledRecord` read
/// `payload.manifest.source`, which silently returned None and bubbled
/// up as the misleading "installed manifest is missing 'source'" error
/// from `check_plugin_updates`. This test installs a plugin via the same
/// code path the LLM tool uses, then asserts the install record is
/// findable by id and that source / version / files round-trip.
#[tokio::test]
async fn latest_install_round_trips_id_version_source_and_files() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");
    let unpacked = extract_to(&archive_dir, &archive);

    install_from_unpacked_with_bus(&scratch, &bus, &unpacked, SourceType::Archive, false, None)
        .await
        .expect("install must succeed");

    // Install record must be findable by the manifest id (not "unknown").
    let installed = latest_install(&pool, "fixture-plugin")
        .await
        .expect("query must succeed")
        .expect("install record must be findable by manifest id, not 'unknown'");

    // Without the fix, all three of these return None / empty.
    assert_eq!(installed.version(), Some("0.1.0"), "version round-trip");
    assert_eq!(
        installed.source(),
        Some("https://github.com/x/y"),
        "source URL must be retrievable from install record — \
         this is the regression that caused 'missing source' errors in check_plugin_updates"
    );
    let mut files = installed.files();
    files.sort();
    assert_eq!(
        files,
        vec![
            "knowhow/fixture.md".to_string(),
            "triggers/fixture/fixture.md".to_string()
        ],
        "files list must round-trip from PluginInstalled payload"
    );

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

/// Fetch the persisted payload(s) for an aggregate id, oldest first, so
/// tests can index by emit order.
async fn read_events(pool: &sqlx::PgPool, event_type: &str, id: &str) -> Vec<serde_json::Value> {
    let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
        r#"SELECT payload FROM events
           WHERE event_type = $1 AND aggregate_id = $2
           ORDER BY sequence ASC"#,
    )
    .bind(event_type)
    .bind(id)
    .fetch_all(pool)
    .await
    .expect("query events");
    rows.into_iter().map(|(p,)| p).collect()
}

// ---- e2e test 1 -------------------------------------------------------

/// Install from a local `.lucidos-plugin` archive via the same code path
/// the LLM tool uses (`install_from_source_with_bus`). Verifies files
/// land under `data/<dir>/...` and that a PluginInstalled event is
/// persisted with the manifest payload.
#[tokio::test]
async fn e2e_install_from_local_archive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1-from-archive");

    let msg = install_from_source_with_bus(&scratch, &bus, archive.to_str().unwrap(), false)
        .await
        .expect("install from archive must succeed");
    assert_eq!(msg, "Installed Fixture Plugin v0.1.0 (2 files).");

    // Files landed under data/.
    assert_eq!(
        std::fs::read_to_string(scratch.join("data/knowhow/fixture.md")).unwrap(),
        "v1-from-archive"
    );
    assert!(scratch.join("data/triggers/fixture/fixture.md").is_file());

    // Exactly one PluginInstalled event, payload reflects the manifest
    // and the archive source type.
    let events = read_events(&pool, "PluginInstalled", "fixture-plugin").await;
    assert_eq!(events.len(), 1, "exactly one PluginInstalled event");
    let raw_manifest = events[0]
        .pointer("/data/manifest/manifest")
        .expect("raw manifest must be nested at /data/manifest/manifest");
    assert_eq!(raw_manifest["id"], "fixture-plugin");
    assert_eq!(raw_manifest["version"], "0.1.0");
    assert_eq!(events[0]["data"]["source_type"], "archive");
    let files: Vec<&str> = events[0]["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(files.contains(&"knowhow/fixture.md"));
    assert!(files.contains(&"triggers/fixture/fixture.md"));

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

// ---- e2e test 2 -------------------------------------------------------

/// Install from a local bare git repo via `file://...git` URL — the same
/// `install_from_source_with_bus` path that handles GitHub URLs in
/// production. Verifies the source URL is retrievable from the install
/// record so `check_plugin_updates` and `update_plugin` can re-fetch.
#[tokio::test]
async fn e2e_install_from_local_git_source_records_source_url() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let scratch = fresh_workspace();
    let repos_dir = scratch.join("repos");
    std::fs::create_dir_all(&repos_dir).unwrap();
    let (bare, _work) = make_local_git_plugin(
        &repos_dir,
        "fixture-git",
        "git-fixture-plugin",
        "0.1.0",
        "v1 body",
    );
    let source_url = format!("file://{}", bare.display());

    let msg = install_from_source_with_bus(&scratch, &bus, &source_url, false)
        .await
        .expect("git install must succeed");
    assert_eq!(msg, "Installed Local Git Plugin v0.1.0 (1 files).");
    assert_eq!(
        std::fs::read_to_string(scratch.join("data/knowhow/git-fixture-plugin.md")).unwrap(),
        "v1 body"
    );

    // Source URL must round-trip — without it, update_plugin can't re-fetch.
    let installed = latest_install(&pool, "git-fixture-plugin")
        .await
        .unwrap()
        .expect("install record must exist");
    assert_eq!(installed.source(), Some(source_url.as_str()));
    assert_eq!(installed.version(), Some("0.1.0"));

    let events = read_events(&pool, "PluginInstalled", "git-fixture-plugin").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["data"]["source_type"], "git");

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

// ---- e2e test 3 -------------------------------------------------------

/// Regression test for the "installed manifest is missing 'source'" bug
/// at the tool-handler level (the version 1 test exercises only
/// `InstalledRecord`). Drives the full `check_plugin_updates_impl` path
/// the way the LLM dispatcher calls it, and asserts the JSON report
/// contains real version + source data instead of the misleading error.
#[tokio::test]
async fn e2e_check_plugin_updates_returns_real_data_after_install() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let scratch = fresh_workspace();
    let repos_dir = scratch.join("repos");
    std::fs::create_dir_all(&repos_dir).unwrap();
    let (bare, _work) = make_local_git_plugin(
        &repos_dir,
        "fixture-check",
        "check-fixture-plugin",
        "0.1.0",
        "stable body",
    );
    let source_url = format!("file://{}", bare.display());

    install_from_source_with_bus(&scratch, &bus, &source_url, false)
        .await
        .expect("install");

    let report_json =
        check_plugin_updates_impl(&scratch, &pool, Some("check-fixture-plugin".to_string())).await;
    let report: Vec<serde_json::Value> =
        serde_json::from_str(&report_json).expect("report parses as JSON");
    assert_eq!(report.len(), 1, "single id → single report entry");
    let entry = &report[0];

    assert_eq!(entry["id"], "check-fixture-plugin");
    assert!(
        entry.get("error").is_none(),
        "must NOT report 'missing source' — got: {}",
        entry
    );
    assert_eq!(entry["installed_version"], "0.1.0");
    assert_eq!(entry["latest_version"], "0.1.0");
    assert_eq!(entry["changed"], false);
    assert_eq!(entry["source"], source_url);

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

// ---- e2e test 4 -------------------------------------------------------

/// `update_plugin` re-fetches when the upstream version bumps. Installs
/// v0.1.0, pushes a v0.1.1 commit to the same bare repo, then invokes
/// `update_plugin_impl`. Asserts the new content lands on disk and a
/// second PluginInstalled event is recorded (updates reuse that variant
/// per the documented contract — there is no separate PluginUpdated).
#[tokio::test]
async fn e2e_update_plugin_re_fetches_when_version_bumps() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let scratch = fresh_workspace();
    let repos_dir = scratch.join("repos");
    std::fs::create_dir_all(&repos_dir).unwrap();
    let (bare, work) = make_local_git_plugin(
        &repos_dir,
        "fixture-update",
        "update-fixture-plugin",
        "0.1.0",
        "v1 body",
    );
    let source_url = format!("file://{}", bare.display());

    // 1. Install v0.1.0.
    install_from_source_with_bus(&scratch, &bus, &source_url, false)
        .await
        .expect("install v0.1.0");
    let knowhow_path = scratch.join("data/knowhow/update-fixture-plugin.md");
    assert_eq!(std::fs::read_to_string(&knowhow_path).unwrap(), "v1 body");

    // 2. Bump upstream to v0.1.1.
    bump_local_git_plugin(&work, "update-fixture-plugin", "0.1.0", "0.1.1", "v2 body");

    // 3. update_plugin re-fetches and re-installs.
    let msg = update_plugin_impl(&scratch, &bus, &pool, "update-fixture-plugin").await;
    assert!(
        msg.starts_with("Installed Local Git Plugin v0.1.1"),
        "update message: {}",
        msg
    );

    // 4. New content is on disk and a second PluginInstalled was recorded.
    assert_eq!(std::fs::read_to_string(&knowhow_path).unwrap(), "v2 body");
    let events = read_events(&pool, "PluginInstalled", "update-fixture-plugin").await;
    assert_eq!(
        events.len(),
        2,
        "install + update = 2 PluginInstalled events"
    );
    assert_eq!(
        events[0]
            .pointer("/data/manifest/manifest/version")
            .unwrap(),
        "0.1.0"
    );
    assert_eq!(
        events[1]
            .pointer("/data/manifest/manifest/version")
            .unwrap(),
        "0.1.1"
    );

    // 5. Re-running update with no upstream change is a no-op (no third event).
    let again = update_plugin_impl(&scratch, &bus, &pool, "update-fixture-plugin").await;
    assert_eq!(again, "Already at latest (v0.1.1)");
    assert_eq!(
        read_events(&pool, "PluginInstalled", "update-fixture-plugin")
            .await
            .len(),
        2,
        "no-op update must not emit a third PluginInstalled"
    );

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

// ---- e2e test 5 -------------------------------------------------------

