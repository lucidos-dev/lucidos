use super::*;

/// Install a plugin by source string (git URL, GitHub tree URL, or
/// `.lucidos-plugin` archive path). Detects the shape, fetches into a temp
/// dir, then delegates to `install_from_unpacked_with_bus`. Test-only entry
/// point — production install runs through `prepare_install_request` +
/// `confirm_pending_install`, which surfaces the user-confirm panel before
/// any bytes touch `data/`.
async fn install_from_source_with_bus(
    workspace_path: &Path,
    bus: &dyn EventBusEmitter,
    source_str: &str,
    overwrite: bool,
) -> Result<String, String> {
    let source = detect_source(source_str)?;
    let (_scratch, plugin_root, source_type) = fetch_source(workspace_path, &source)?;
    let (summary, _files) = install_from_unpacked_with_bus(
        workspace_path,
        bus,
        &plugin_root,
        source_type,
        overwrite,
        None,
    )
    .await?;
    Ok(summary)
}

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

#[test]
fn detect_source_github_tree_url_with_subpath() {
    let s =
        detect_source("https://github.com/lucidos-dev/plugins/tree/main/browser-learning").unwrap();
    match s {
        Source::Git {
            url,
            branch,
            subpath,
        } => {
            assert_eq!(url, "https://github.com/lucidos-dev/plugins.git");
            assert_eq!(branch.as_deref(), Some("main"));
            assert_eq!(subpath.as_deref(), Some("browser-learning"));
        }
        other => panic!("expected Git, got {:?}", other),
    }
}

#[test]
fn detect_source_github_tree_url_without_subpath() {
    let s = detect_source("https://github.com/lucidos-dev/plugin-x/tree/main").unwrap();
    match s {
        Source::Git {
            url,
            branch,
            subpath,
        } => {
            assert_eq!(url, "https://github.com/lucidos-dev/plugin-x.git");
            assert_eq!(branch.as_deref(), Some("main"));
            assert_eq!(subpath, None);
        }
        other => panic!("expected Git, got {:?}", other),
    }
}

#[test]
fn detect_source_plain_https_repo() {
    let s = detect_source("https://github.com/x/y.git").unwrap();
    match s {
        Source::Git {
            url,
            branch,
            subpath,
        } => {
            assert_eq!(url, "https://github.com/x/y.git");
            assert_eq!(branch, None);
            assert_eq!(subpath, None);
        }
        other => panic!("expected Git, got {:?}", other),
    }
}

#[test]
fn detect_source_ssh() {
    let s = detect_source("git@github.com:x/y.git").unwrap();
    assert!(matches!(s, Source::Git { .. }));
}

#[test]
fn detect_source_archive_missing_file() {
    let err = detect_source("/tmp/no-such-thing.lucidos-plugin").unwrap_err();
    assert!(err.contains("not found"));
}

#[test]
fn detect_source_unknown_shape() {
    let err = detect_source("just-a-name").unwrap_err();
    assert!(err.contains("could not infer"));
}

#[test]
fn short_source_strips_https_and_git_suffix() {
    assert_eq!(short_source("https://github.com/a/b.git"), "github.com/a/b");
    assert_eq!(short_source("https://github.com/a/b/"), "github.com/a/b");
}

#[test]
fn validate_archive_entry_path_is_used() {
    // Smoke: the public function in core::plugins still rejects ../.
    assert!(validate_archive_entry_path("a/../b").is_err());
}

// --- Integration test: full install / conflict / overwrite / uninstall ---
//
// Builds a `.lucidos-plugin` zip in a temp dir, extracts it via the same
// code path the live tool uses, and asserts the EventBus receives the
// expected `PluginInstalled` and `PluginUninstalled` frames. Uses the
// in-memory `MockEventBus` so no PgPool is needed.

use crate::engine::event_bus::MockEventBus;
use std::io::Write;

const FIXTURE_MANIFEST: &str = r#"
id = "fixture-plugin"
version = "0.1.0"
name = "Fixture Plugin"
description = "test"
source = "https://github.com/x/y"
"#;

fn build_archive(
    tmp: &Path,
    archive_name: &str,
    manifest: &str,
    files: &[(&str, &[u8])],
) -> PathBuf {
    let archive_path = tmp.join(archive_name);
    let file = std::fs::File::create(&archive_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("manifest.toml", opts).unwrap();
    zip.write_all(manifest.as_bytes()).unwrap();
    for (path, body) in files {
        zip.start_file(*path, opts).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
    archive_path
}

fn build_fixture_archive(tmp: &Path, knowhow_body: &str) -> PathBuf {
    build_archive(
        tmp,
        "fixture.lucidos-plugin",
        FIXTURE_MANIFEST,
        &[
            ("knowhow/fixture.md", knowhow_body.as_bytes()),
            (
                "triggers/fixture/fixture.md",
                b"---\nname: Fixture\n---\nrun me",
            ),
        ],
    )
}

fn extract_to(tmp: &Path, archive: &Path) -> PathBuf {
    let dest = tmp.join("unpacked");
    std::fs::create_dir_all(&dest).unwrap();
    super::extract_zip(archive, &dest).unwrap();
    dest
}

fn fresh_workspace() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("lucidos_plugins_int_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(p.join("data")).unwrap();
    p
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

use crate::engine::event_bus::EventBus;
use crate::test_support::{setup_test_db, teardown_test_db};

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

/// `uninstall_plugin` confirm flow: prepare_uninstall_plugin returns the
/// `[PLUGIN_UNINSTALL_REQUEST]` sentinel, the user confirms via the panel,
/// and uninstall_with_bus deletes the recorded files + emits an extended
/// PluginUninstalled with `files_deleted` populated. Asserts the symmetric
/// install-confirm shape: prepare doesn't touch disk, confirm does.
#[tokio::test]
async fn e2e_uninstall_plugin_deletes_files_after_confirm() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "delete me on uninstall");

    install_from_source_with_bus(&scratch, &bus, archive.to_str().unwrap(), false)
        .await
        .expect("install");
    let knowhow_path = scratch.join("data/knowhow/fixture.md");
    let trigger_path = scratch.join("data/triggers/fixture/fixture.md");
    let trigger_dir = scratch.join("data/triggers/fixture");
    assert!(knowhow_path.is_file());
    assert!(trigger_path.is_file());

    let pending_uninstalls: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let sentinel = prepare_uninstall_plugin(&scratch, &pool, &pending_uninstalls, "fixture-plugin")
        .await;
    assert!(
        sentinel.starts_with(PLUGIN_UNINSTALL_REQUEST_PREFIX),
        "expected uninstall sentinel, got: {}",
        sentinel
    );

    // Files MUST still be on disk before confirm — symmetric with install,
    // which doesn't write until the panel resolves.
    assert!(
        knowhow_path.is_file(),
        "prepare must not delete files before confirm"
    );

    // Pop the pending entry and run the delete-and-emit step directly
    // (mirrors what confirm_pending_uninstall does without an engine).
    let uninstall_id = pending_uninstalls
        .lock()
        .unwrap()
        .keys()
        .next()
        .cloned()
        .expect("one pending uninstall registered");
    let pending = pending_uninstalls
        .lock()
        .unwrap()
        .remove(&uninstall_id)
        .unwrap();
    let outcome = uninstall_with_bus(&scratch, &bus, &pending, None)
        .await
        .expect("uninstall_with_bus");

    assert!(
        outcome.summary.contains("Uninstalled Fixture Plugin v0.1.0"),
        "summary: {}",
        outcome.summary
    );
    assert!(
        outcome.summary.contains("2 files removed"),
        "summary should mention file count: {}",
        outcome.summary
    );
    let mut deleted_sorted = outcome.files_deleted.clone();
    deleted_sorted.sort();
    assert_eq!(
        deleted_sorted,
        vec![
            "knowhow/fixture.md".to_string(),
            "triggers/fixture/fixture.md".to_string(),
        ]
    );

    // Files actually gone now — and the empty `triggers/fixture/` dir was
    // pruned (but `triggers/` itself remains, since it's the content-dir
    // root the prune is forbidden to cross).
    assert!(!knowhow_path.exists(), "knowhow file must be deleted");
    assert!(!trigger_path.exists(), "trigger file must be deleted");
    assert!(
        !trigger_dir.exists(),
        "empty per-plugin trigger dir must be pruned"
    );
    assert!(
        scratch.join("data/triggers").is_dir(),
        "content-dir root must NOT be pruned"
    );

    // Event recorded with files_deleted populated.
    let events = read_events(&pool, "PluginUninstalled", "fixture-plugin").await;
    assert_eq!(events.len(), 1, "exactly one PluginUninstalled event");
    let payload = &events[0];
    assert_eq!(payload["data"]["id"], "fixture-plugin");
    assert_eq!(payload["data"]["version"], "0.1.0");
    let deleted: Vec<&str> = payload["data"]["files_deleted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(deleted.contains(&"knowhow/fixture.md"));
    assert!(deleted.contains(&"triggers/fixture/fixture.md"));

    // After uninstall, latest_install returns None for this id.
    let after = latest_install(&pool, "fixture-plugin").await.unwrap();
    assert!(
        after.is_none(),
        "uninstall must hide the install record from subsequent lookups"
    );

    let _ = std::fs::remove_dir_all(&scratch);
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn install_then_conflict_then_overwrite_then_uninstall() {
    let scratch = fresh_workspace();
    let workspace = scratch.clone();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");
    let unpacked = extract_to(&archive_dir, &archive);

    let bus = MockEventBus::new();

    // 1. Fresh install lands files and emits PluginInstalled.
    let (msg, _files) = install_from_unpacked_with_bus(
        &workspace,
        &bus,
        &unpacked,
        SourceType::Archive,
        false,
        None,
    )
    .await
    .expect("install should succeed on empty workspace");
    assert_eq!(msg, "Installed Fixture Plugin v0.1.0 (2 files).");

    let kn_path = workspace.join("data/knowhow/fixture.md");
    let trig_path = workspace.join("data/triggers/fixture/fixture.md");
    assert!(kn_path.is_file(), "knowhow/fixture.md missing");
    assert!(trig_path.is_file(), "triggers/fixture/fixture.md missing");
    assert_eq!(std::fs::read_to_string(&kn_path).unwrap(), "v1");

    let events = bus.emitted_events();
    assert_eq!(events.len(), 1, "exactly one event after first install");
    match &events[0] {
        BusEvent::System(SystemEvent::PluginInstalled {
            manifest,
            files,
            source_type,
            installed_at,
            ..
        }) => {
            assert_eq!(source_type, "archive");
            assert!(!installed_at.is_empty());
            let mut sorted = files.clone();
            sorted.sort();
            assert_eq!(
                sorted,
                vec![
                    "knowhow/fixture.md".to_string(),
                    "triggers/fixture/fixture.md".to_string()
                ]
            );
            let payload_id = manifest
                .get("manifest")
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str());
            assert_eq!(payload_id, Some("fixture-plugin"));
            let payload_summary = manifest
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert!(
                payload_summary.starts_with("Installed Fixture Plugin v0.1.0 from "),
                "summary was: {}",
                payload_summary
            );
        }
        other => panic!("expected PluginInstalled, got {:?}", other),
    }

    // 2. Re-install without overwrite returns the conflict error and emits nothing new.
    let v2_archive_dir = scratch.join("archive_v2");
    std::fs::create_dir_all(&v2_archive_dir).unwrap();
    let archive_v2 = build_fixture_archive(&v2_archive_dir, "v2");
    let unpacked_v2 = extract_to(&v2_archive_dir, &archive_v2);

    let err = install_from_unpacked_with_bus(
        &workspace,
        &bus,
        &unpacked_v2,
        SourceType::Archive,
        false,
        None,
    )
    .await
    .expect_err("second install must hit conflict");
    assert!(
        err.contains("would overwrite"),
        "conflict message was: {}",
        err
    );
    assert!(
        err.contains("knowhow/fixture.md"),
        "conflict message must list the file: {}",
        err
    );
    assert_eq!(
        bus.emitted_events().len(),
        1,
        "conflict path must not emit a second event"
    );
    // File on disk unchanged — still v1.
    assert_eq!(std::fs::read_to_string(&kn_path).unwrap(), "v1");

    // 3. Re-install with overwrite=true succeeds and the file content updates.
    let (msg2, _files2) = install_from_unpacked_with_bus(
        &workspace,
        &bus,
        &unpacked_v2,
        SourceType::Archive,
        true,
        None,
    )
    .await
    .expect("overwrite install should succeed");
    assert_eq!(msg2, "Installed Fixture Plugin v0.1.0 (2 files).");
    assert_eq!(
        std::fs::read_to_string(&kn_path).unwrap(),
        "v2",
        "overwrite must replace file content"
    );
    assert_eq!(
        bus.emitted_events().len(),
        2,
        "overwrite must emit a new PluginInstalled"
    );

    // 4. Uninstall via the new confirm flow: build a PendingUninstall directly
    //    from the recorded files and run uninstall_with_bus. Asserts that
    //    files now actually disappear (the v2 behavior — v1 was guide-only).
    let installed_files = match &bus.emitted_events()[1] {
        BusEvent::System(SystemEvent::PluginInstalled { files, .. }) => files.clone(),
        other => panic!("expected PluginInstalled at index 1, got {:?}", other),
    };
    let pending = PendingUninstall {
        plugin_id: "fixture-plugin".to_string(),
        plugin_version: "0.1.0".to_string(),
        plugin_name: "Fixture Plugin".to_string(),
        files_present: installed_files.clone(),
        files_missing: Vec::new(),
        created_at: chrono::Utc::now(),
    };
    let outcome = uninstall_with_bus(&workspace, &bus, &pending, None)
        .await
        .expect("uninstall should succeed");
    assert!(
        outcome.summary.contains("Uninstalled Fixture Plugin v0.1.0"),
        "uninstall summary: {}",
        outcome.summary
    );
    assert_eq!(outcome.files_deleted.len(), 2);
    // Files actually deleted now.
    assert!(!kn_path.exists(), "uninstall must delete the knowhow file");
    assert!(!trig_path.exists(), "uninstall must delete the trigger file");

    let final_events = bus.emitted_events();
    assert_eq!(final_events.len(), 3);
    match &final_events[2] {
        BusEvent::System(SystemEvent::PluginUninstalled {
            id,
            version,
            files,
            files_deleted,
            files_missing,
            ..
        }) => {
            assert_eq!(id, "fixture-plugin");
            assert_eq!(version, "0.1.0");
            let mut sorted = files.clone();
            sorted.sort();
            assert_eq!(
                sorted,
                vec![
                    "knowhow/fixture.md".to_string(),
                    "triggers/fixture/fixture.md".to_string()
                ]
            );
            assert_eq!(files_deleted.len(), 2);
            assert!(files_missing.is_empty(), "all files were present at confirm");
        }
        other => panic!("expected PluginUninstalled, got {:?}", other),
    }

    let _ = std::fs::remove_dir_all(&scratch);
}

// ---- uninstall confirm-flow unit tests ------------------------------

#[test]
fn is_safe_data_path_accepts_content_dir_files() {
    assert!(super::is_safe_data_path("apps/foo/index.html"));
    assert!(super::is_safe_data_path("knowhow/foo.md"));
    assert!(super::is_safe_data_path("triggers/foo/run.md"));
    assert!(super::is_safe_data_path("scripts/foo.py"));
    assert!(super::is_safe_data_path("auth-modules/foo.wasm"));
}

#[test]
fn is_safe_data_path_rejects_traversal_and_unknown_roots() {
    // Traversal attempts (delegated to `api::is_path_traversal`).
    assert!(!super::is_safe_data_path("../etc/passwd"));
    assert!(!super::is_safe_data_path("apps/../../etc/passwd"));
    assert!(!super::is_safe_data_path("/etc/passwd"));
    assert!(!super::is_safe_data_path("\\windows\\system32"));
    // Unknown content-dir roots — defense against tampered install records.
    assert!(!super::is_safe_data_path("artifacts/foo.txt"));
    assert!(!super::is_safe_data_path("postgres/data.bin"));
    assert!(!super::is_safe_data_path(".lucidos/secrets"));
    // Empty.
    assert!(!super::is_safe_data_path(""));
}

#[test]
fn prune_empty_parents_stops_at_content_dir_root() {
    let scratch = fresh_workspace();
    let data_dir = scratch.join("data");
    let nested = data_dir.join("apps/myapp/sub/leaf.html");
    std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
    std::fs::write(&nested, b"x").unwrap();

    // Simulate the file already removed; prune should walk up sub → myapp,
    // remove both, then stop at apps/ (the content-dir root).
    std::fs::remove_file(&nested).unwrap();
    super::prune_empty_parents(&data_dir, "apps/myapp/sub/leaf.html");

    assert!(!data_dir.join("apps/myapp/sub").exists(), "sub must be pruned");
    assert!(!data_dir.join("apps/myapp").exists(), "myapp must be pruned");
    assert!(
        data_dir.join("apps").is_dir(),
        "content-dir root must NOT be pruned even when empty"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn prune_empty_parents_leaves_non_empty_dir_alone() {
    let scratch = fresh_workspace();
    let data_dir = scratch.join("data");
    let leaf = data_dir.join("knowhow/sibling-file.md");
    let other = data_dir.join("knowhow/keep.md");
    std::fs::create_dir_all(leaf.parent().unwrap()).unwrap();
    std::fs::write(&leaf, b"x").unwrap();
    std::fs::write(&other, b"y").unwrap();

    std::fs::remove_file(&leaf).unwrap();
    super::prune_empty_parents(&data_dir, "knowhow/sibling-file.md");

    assert!(other.is_file(), "sibling file must survive prune");
    assert!(data_dir.join("knowhow").is_dir());

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn prepare_uninstall_request_partitions_present_and_missing() {
    let scratch = fresh_workspace();
    let data_dir = scratch.join("data");
    std::fs::create_dir_all(data_dir.join("apps/foo")).unwrap();
    std::fs::write(data_dir.join("apps/foo/index.html"), b"<html/>").unwrap();
    // knowhow/foo.md does NOT exist → expected in `files_missing`.

    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let result = super::prepare_uninstall_request(
        &scratch,
        &pending,
        "foo",
        "1.2.3",
        "Foo",
        vec![
            "apps/foo/index.html".to_string(),
            "knowhow/foo.md".to_string(),
            "../etc/passwd".to_string(), // unsafe — must land in `missing`
        ],
    );

    assert!(result.starts_with(PLUGIN_UNINSTALL_REQUEST_PREFIX));
    let json = &result[PLUGIN_UNINSTALL_REQUEST_PREFIX.len()..];
    let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(parsed["plugin_id"], "foo");
    assert_eq!(parsed["plugin_version"], "1.2.3");
    assert_eq!(parsed["plugin_name"], "Foo");

    let present: Vec<&str> = parsed["files_present"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let missing: Vec<&str> = parsed["files_missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    assert_eq!(present, vec!["apps/foo/index.html"]);
    assert!(missing.contains(&"knowhow/foo.md"));
    assert!(
        missing.contains(&"../etc/passwd"),
        "unsafe path must surface in files_missing, never in files_present"
    );

    // PendingUninstall registered under the returned uninstall_id.
    let uninstall_id = parsed["uninstall_id"].as_str().unwrap();
    assert!(pending.lock().unwrap().contains_key(uninstall_id));

    let _ = std::fs::remove_dir_all(&scratch);
}

#[tokio::test]
async fn cancel_pending_uninstall_emits_event_and_drops_entry() {
    let bus = MockEventBus::new();
    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let entry = PendingUninstall {
        plugin_id: "foo".to_string(),
        plugin_version: "1.0.0".to_string(),
        plugin_name: "Foo".to_string(),
        files_present: vec!["apps/foo/index.html".to_string()],
        files_missing: Vec::new(),
        created_at: chrono::Utc::now(),
    };
    pending
        .lock()
        .unwrap()
        .insert("uninstall-123".to_string(), entry);

    super::cancel_pending_uninstall_with_bus(&pending, &bus, "uninstall-123", None)
        .await
        .expect("cancel must succeed");

    assert!(
        pending.lock().unwrap().is_empty(),
        "pending entry must be removed on cancel"
    );

    let events = bus.emitted_events();
    assert_eq!(events.len(), 1);
    match &events[0] {
        BusEvent::System(SystemEvent::PluginUninstallCanceled { id, version, .. }) => {
            assert_eq!(id, "foo");
            assert_eq!(version, "1.0.0");
        }
        other => panic!("expected PluginUninstallCanceled, got {:?}", other),
    }
}

#[tokio::test]
async fn cancel_pending_uninstall_returns_error_for_unknown_id() {
    let bus = MockEventBus::new();
    let pending: std::sync::Arc<PendingUninstallsMap> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let err = super::cancel_pending_uninstall_with_bus(&pending, &bus, "missing-id", None)
        .await
        .unwrap_err();
    assert!(err.contains("no pending uninstall"), "err was: {}", err);
    assert!(bus.emitted_events().is_empty(), "no event on bad id");
}

#[tokio::test]
async fn uninstall_with_bus_handles_file_disappearing_between_prepare_and_confirm() {
    // Race: prepare snapshots the file as present, then the user (or another
    // process) deletes it manually before clicking Confirm. uninstall_with_bus
    // must NOT fail — fold it into files_missing and continue.
    let scratch = fresh_workspace();
    let data_dir = scratch.join("data");
    std::fs::create_dir_all(data_dir.join("knowhow")).unwrap();
    std::fs::write(data_dir.join("knowhow/stays.md"), b"x").unwrap();
    // The "raced" file is intentionally not created on disk, even though
    // we'll mark it as files_present in the pending entry.

    let pending = PendingUninstall {
        plugin_id: "race-fixture".to_string(),
        plugin_version: "0.1.0".to_string(),
        plugin_name: "Race Fixture".to_string(),
        files_present: vec![
            "knowhow/stays.md".to_string(),
            "knowhow/already-gone.md".to_string(),
        ],
        files_missing: Vec::new(),
        created_at: chrono::Utc::now(),
    };

    let bus = MockEventBus::new();
    let outcome = super::uninstall_with_bus(&scratch, &bus, &pending, None)
        .await
        .expect("race must not fail uninstall");

    assert_eq!(outcome.files_deleted, vec!["knowhow/stays.md"]);
    assert!(
        outcome.files_missing.contains(&"knowhow/already-gone.md".to_string()),
        "raced-missing file must end up in files_missing"
    );
    assert!(!data_dir.join("knowhow/stays.md").exists());

    let _ = std::fs::remove_dir_all(&scratch);
}

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
    use super::normalize_plugin_query as n;
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
