use std::path::PathBuf;

use super::helpers::*;
use super::*;
use crate::engine::pending_plugins::normalize_setup;

use crate::engine::event_bus::{EventBus, MockEventBus};
use crate::test_support::{setup_test_db, teardown_test_db};

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

    let remote = match fetch_remote_manifest(workspace_path, pool, &source).await {
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
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_sourceless_fixture(&archive_dir);
    let unpacked = extract_to(&archive_dir, &archive);

    let bus = MockEventBus::new();
    let write = install_from_unpacked_with_bus(
        &scratch,
        &bus,
        &unpacked,
        InstallContext::plain(SourceType::Archive, false),
    )
    .await
    .expect("install must succeed even with no source field");
    assert_eq!(
        write.summary,
        "Installed Sourceless Plugin v0.1.0 (1 files)."
    );

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
}

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
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "preview-body");
    let pending = fresh_pending_map();

    let result = prepare_install_request(
        &scratch,
        &pending,
        archive.to_str().unwrap(),
        &Ok(Default::default()),
        &GitCredentials::none(),
        &running(),
        None,
    );

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

    assert!(!scratch.join("data/knowhow/fixture.md").exists());
}

fn stage_with_engine(
    scratch: &Path,
    pending: &std::sync::Arc<PendingInstallsMap>,
    engine: &str,
    running: &Result<semver::Version, String>,
) -> String {
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let manifest = FIXTURE_MANIFEST
        .replace("Fixture Plugin", "Theme Studio")
        .replace("source =", &format!("engine = {engine}\nsource ="));
    let archive = build_archive(
        &archive_dir,
        "theme-studio.lucidos-plugin",
        &manifest,
        &[("knowhow/fixture.md", b"body")],
    );
    prepare_install_request(
        scratch,
        pending,
        archive.to_str().unwrap(),
        &Ok(Default::default()),
        &GitCredentials::none(),
        running,
        None,
    )
}

#[test]
fn an_unmet_engine_requirement_refuses_staging_and_writes_nothing() {
    let (_tmp, scratch) = fresh_workspace();
    let pending = fresh_pending_map();

    let result = stage_with_engine(
        &scratch,
        &pending,
        r#"">=0.46.1""#,
        &Ok(semver::Version::new(0, 46, 0)),
    );

    assert_eq!(
        result,
        "Error: Theme Studio 0.1.0 needs Lucidos 0.46.1 or later. This is Lucidos 0.46.0. \
         Update Lucidos first."
    );
    assert!(pending.lock().unwrap().is_empty(), "no confirm panel");
    assert!(!scratch.join("data/knowhow").exists(), "nothing in data/");
}

#[test]
fn an_invalid_engine_requirement_refuses_staging_with_the_value_quoted() {
    let (_tmp, scratch) = fresh_workspace();
    let pending = fresh_pending_map();

    let result = stage_with_engine(&scratch, &pending, r#""newest""#, &running());

    assert!(result.starts_with("Error: Theme Studio 0.1.0 "), "{result}");
    assert!(result.contains(r#"engine = "newest""#), "{result}");
    assert!(pending.lock().unwrap().is_empty());
    assert!(!scratch.join("data/knowhow").exists());
}

#[test]
fn a_met_engine_requirement_stages_as_usual() {
    let (_tmp, scratch) = fresh_workspace();
    let pending = fresh_pending_map();

    let result = stage_with_engine(
        &scratch,
        &pending,
        r#"">=0.46.1""#,
        &Ok(semver::Version::new(0, 46, 1)),
    );

    assert!(
        result.starts_with(PLUGIN_INSTALL_REQUEST_PREFIX),
        "got: {result}"
    );
    assert_eq!(pending.lock().unwrap().len(), 1);
    assert_eq!(
        parse_sentinel_payload(&result)["engine_requirement"],
        ">=0.46.1"
    );
}

#[test]
fn a_plugin_with_no_engine_stages_with_a_null_engine_requirement() {
    let (_tmp, scratch) = fresh_workspace();
    let pending = fresh_pending_map();
    let archive = build_fixture_archive(&scratch, "body");

    let result = prepare_install_request(
        &scratch,
        &pending,
        archive.to_str().unwrap(),
        &Ok(Default::default()),
        &GitCredentials::none(),
        &Ok(semver::Version::new(0, 1, 0)),
        None,
    );

    assert!(
        result.starts_with(PLUGIN_INSTALL_REQUEST_PREFIX),
        "got: {result}"
    );
    assert_eq!(pending.lock().unwrap().len(), 1);
    let payload = parse_sentinel_payload(&result);
    assert!(payload["manifest"].get("engine").is_none());
    assert!(payload["engine_requirement"].is_null(), "{payload}");
}

#[test]
fn an_unreadable_release_refuses_only_a_plugin_that_declares_engine() {
    let (_tmp, scratch) = fresh_workspace();
    let pending = fresh_pending_map();
    let unreadable = Err("RELEASE is not semver".to_string());

    let refused = stage_with_engine(&scratch, &pending, r#"">=0.46.1""#, &unreadable);
    assert!(refused.contains("RELEASE is not semver"), "{refused}");
    assert!(pending.lock().unwrap().is_empty());

    let archive = build_fixture_archive(&scratch, "body");
    let staged = prepare_install_request(
        &scratch,
        &pending,
        archive.to_str().unwrap(),
        &Ok(Default::default()),
        &GitCredentials::none(),
        &unreadable,
        None,
    );
    assert!(
        staged.starts_with(PLUGIN_INSTALL_REQUEST_PREFIX),
        "got: {staged}"
    );
}

#[test]
fn prepare_install_request_lists_overwrites_when_files_already_exist() {
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");
    std::fs::create_dir_all(scratch.join("data/knowhow")).unwrap();
    std::fs::write(scratch.join("data/knowhow/fixture.md"), "existing").unwrap();
    let pending = fresh_pending_map();

    let result = prepare_install_request(
        &scratch,
        &pending,
        archive.to_str().unwrap(),
        &Ok(Default::default()),
        &GitCredentials::none(),
        &running(),
        None,
    );
    let payload = parse_sentinel_payload(&result);
    let overwrites: Vec<&str> = payload["overwrites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(overwrites, vec!["knowhow/fixture.md"]);
    assert_eq!(
        std::fs::read_to_string(scratch.join("data/knowhow/fixture.md")).unwrap(),
        "existing"
    );
}

#[test]
fn a_collision_with_unrelated_files_stages_a_fresh_install() {
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");
    std::fs::create_dir_all(scratch.join("data/knowhow")).unwrap();
    std::fs::write(scratch.join("data/knowhow/fixture.md"), "existing").unwrap();

    let result = prepare_install_request(
        &scratch,
        &fresh_pending_map(),
        archive.to_str().unwrap(),
        &Ok(Default::default()),
        &GitCredentials::none(),
        &running(),
        None,
    );
    let payload = parse_sentinel_payload(&result);
    assert_eq!(payload["overwrites"].as_array().unwrap().len(), 1);
    assert_eq!(payload["is_update"], false);
}

#[test]
fn an_installed_plugin_stages_an_update_even_with_its_files_gone() {
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");
    let installed = InstalledPlugins {
        files: [("fixture-plugin".to_string(), Vec::new())].into(),
        ..Default::default()
    };

    let result = prepare_install_request(
        &scratch,
        &fresh_pending_map(),
        archive.to_str().unwrap(),
        &Ok(installed),
        &GitCredentials::none(),
        &running(),
        None,
    );
    let payload = parse_sentinel_payload(&result);
    assert!(payload["overwrites"].as_array().unwrap().is_empty());
    assert_eq!(payload["is_update"], true);
}

fn stage_with_setup(scratch: &Path, extra: &str, installed: InstalledPlugins) -> serde_json::Value {
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let manifest = format!("{FIXTURE_MANIFEST}{extra}");
    let archive = build_archive(
        &archive_dir,
        "fixture.lucidos-plugin",
        &manifest,
        &[("knowhow/fixture.md", b"body")],
    );
    let result = prepare_install_request(
        scratch,
        &fresh_pending_map(),
        archive.to_str().unwrap(),
        &Ok(installed),
        &GitCredentials::none(),
        &running(),
        None,
    );
    parse_sentinel_payload(&result)
}

fn installed_with_setup(prior: PriorSetup) -> InstalledPlugins {
    InstalledPlugins {
        files: [("fixture-plugin".to_string(), Vec::new())].into(),
        setups: [("fixture-plugin".to_string(), prior)].into(),
        ..Default::default()
    }
}

#[test]
fn the_preview_states_whether_an_update_runs_setup_again() {
    let (_tmp, scratch) = fresh_workspace();
    let setup = "setup = \"copy the skill\"\n";
    let prior = |text: &str, revision| {
        installed_with_setup(PriorSetup {
            text: Some(text.into()),
            revision,
            version: Some("0.0.9".into()),
        })
    };

    let raised = stage_with_setup(
        &scratch,
        &format!("{setup}setup_revision = 2\nsetup_reason = \"The skill changed.\"\n"),
        prior("copy the skill", 1),
    );
    assert_eq!(
        raised["setup_run"],
        serde_json::json!({
            "kind": "update",
            "from_version": "0.0.9",
            "cause": {
                "kind": "revision_raised",
                "revision": 2,
                "reason": "The skill changed.",
            },
        })
    );

    let carried = stage_with_setup(
        &scratch,
        &format!("{setup}setup_revision = 2\n"),
        prior("copy the skill", 2),
    );
    assert!(carried["setup_run"].is_null(), "{carried}");

    let changed = stage_with_setup(&scratch, setup, prior("an older text", 0));
    assert_eq!(
        changed["setup_run"]["cause"]["kind"],
        "instructions_changed"
    );

    let fresh = stage_with_setup(&scratch, setup, InstalledPlugins::default());
    assert_eq!(fresh["setup_run"]["kind"], "fresh_install");

    let none = stage_with_setup(&scratch, "", InstalledPlugins::default());
    assert!(none["setup_run"].is_null());
}

#[test]
fn staging_an_update_lists_the_files_it_will_remove() {
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v2");
    std::fs::create_dir_all(scratch.join("data/knowhow")).unwrap();
    std::fs::write(scratch.join("data/knowhow/fixture.md"), "v1").unwrap();
    std::fs::write(scratch.join("data/knowhow/old.md"), "dropped in v2").unwrap();
    let recorded = vec![
        "knowhow/fixture.md".to_string(),
        "knowhow/old.md".to_string(),
    ];
    let installed = InstalledPlugins {
        files: [("fixture-plugin".to_string(), recorded)].into(),
        ..Default::default()
    };

    let result = prepare_install_request(
        &scratch,
        &fresh_pending_map(),
        archive.to_str().unwrap(),
        &Ok(installed),
        &GitCredentials::none(),
        &running(),
        None,
    );
    let payload = parse_sentinel_payload(&result);
    assert_eq!(payload["removes"], serde_json::json!(["knowhow/old.md"]));
    assert_eq!(
        payload["local_changes"],
        serde_json::json!([{ "path": "knowhow/old.md", "outcome": "removed" }])
    );
    assert!(
        scratch.join("data/knowhow/old.md").exists(),
        "nothing is deleted before the user confirms"
    );
}

#[test]
fn staging_never_removes_a_path_another_plugin_records() {
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v2");
    std::fs::create_dir_all(scratch.join("data/knowhow")).unwrap();
    std::fs::write(scratch.join("data/knowhow/fixture.md"), "v1").unwrap();
    std::fs::write(scratch.join("data/knowhow/shared.md"), "both ship this").unwrap();
    let shared = "knowhow/shared.md".to_string();
    let installed = InstalledPlugins {
        files: [
            (
                "fixture-plugin".to_string(),
                vec!["knowhow/fixture.md".to_string(), shared.clone()],
            ),
            ("other-plugin".to_string(), vec![shared]),
        ]
        .into(),
        ..Default::default()
    };

    let result = prepare_install_request(
        &scratch,
        &fresh_pending_map(),
        archive.to_str().unwrap(),
        &Ok(installed),
        &GitCredentials::none(),
        &running(),
        None,
    );
    let payload = parse_sentinel_payload(&result);
    assert_eq!(payload["removes"], serde_json::json!([]));
    assert_eq!(payload["local_changes"], serde_json::json!([]));
}

fn records(entries: &[(&str, &[&str])]) -> InstalledPlugins {
    InstalledPlugins {
        files: entries
            .iter()
            .map(|(id, files)| {
                (
                    id.to_string(),
                    files.iter().map(|f| f.to_string()).collect(),
                )
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn a_confirm_refuses_when_the_records_no_longer_support_its_plan() {
    let staged = records(&[("demo", &["knowhow/a.md", "knowhow/old.md"])]).files;
    let removals = vec!["knowhow/old.md".to_string()];
    let check = |now: InstalledPluginsRead, removals: &[String]| {
        check_records_still_match("demo", Some(&staged), &now, removals)
    };

    assert!(check(
        Ok(records(&[("demo", &["knowhow/a.md", "knowhow/old.md"])])),
        &removals
    )
    .is_ok());
    assert!(
        check(
            Ok(records(&[
                ("demo", &["knowhow/a.md", "knowhow/old.md"]),
                ("other", &["knowhow/b.md"]),
            ])),
            &removals,
        )
        .is_ok(),
        "an unrelated install leaves the plan valid"
    );
    assert!(
        check(
            Ok(records(&[
                ("demo", &["knowhow/a.md", "knowhow/old.md"]),
                ("other", &["knowhow/old.md"]),
            ])),
            &removals,
        )
        .is_err(),
        "another plugin now records a path the plan removes"
    );
    assert!(
        check(
            Ok(records(&[("demo", &["knowhow/a.md", "knowhow/new.md"])])),
            &[]
        )
        .is_err(),
        "a newer version of this plugin confirmed first"
    );
    assert!(
        check(Err("db down".to_string()), &removals).is_err(),
        "an unknown never authorizes a delete"
    );
    assert!(check(Err("db down".to_string()), &[]).is_ok());
    assert!(
        check_records_still_match("demo", None, &Err("db down".to_string()), &[]).is_ok(),
        "staging could not read them either, so there is nothing to compare"
    );
}

#[test]
fn unreadable_baselines_refuse_a_staging_that_would_overwrite_files() {
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v2");
    std::fs::create_dir_all(scratch.join("data/knowhow")).unwrap();
    std::fs::write(scratch.join("data/knowhow/fixture.md"), "my local edit").unwrap();
    let pending = fresh_pending_map();

    let result = prepare_install_request(
        &scratch,
        &pending,
        archive.to_str().unwrap(),
        &Err("connection refused".to_string()),
        &GitCredentials::none(),
        &running(),
        None,
    );

    assert!(result.starts_with("Error:"), "got: {}", result);
    assert!(result.contains("connection refused"), "got: {}", result);
    assert!(
        pending.lock().unwrap().is_empty(),
        "an update that cannot see local edits must not reach the confirm panel"
    );
    assert_eq!(
        std::fs::read_to_string(scratch.join("data/knowhow/fixture.md")).unwrap(),
        "my local edit"
    );
}

#[test]
fn unreadable_baselines_still_stage_a_fresh_install() {
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");
    let pending = fresh_pending_map();

    let result = prepare_install_request(
        &scratch,
        &pending,
        archive.to_str().unwrap(),
        &Err("connection refused".to_string()),
        &GitCredentials::none(),
        &running(),
        None,
    );

    assert!(
        result.starts_with(PLUGIN_INSTALL_REQUEST_PREFIX),
        "a fresh install has nothing to lose, got: {}",
        result
    );
    assert_eq!(pending.lock().unwrap().len(), 1);
}

#[test]
fn prepare_install_request_returns_error_string_on_invalid_source() {
    let (_tmp, scratch) = fresh_workspace();
    let pending = fresh_pending_map();

    let result = prepare_install_request(
        &scratch,
        &pending,
        "not-a-real-source",
        &Ok(Default::default()),
        &GitCredentials::none(),
        &running(),
        None,
    );

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
}

#[tokio::test]
async fn cancel_pending_install_emits_event_and_drops_staging() {
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "to-cancel");
    let pending = fresh_pending_map();

    let result = prepare_install_request(
        &scratch,
        &pending,
        archive.to_str().unwrap(),
        &Ok(Default::default()),
        &GitCredentials::none(),
        &running(),
        None,
    );
    let payload = parse_sentinel_payload(&result);
    let install_id = payload["install_id"].as_str().unwrap().to_string();

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
async fn install_summary_stays_one_line_when_manifest_declares_setup() {
    const SETUP_MANIFEST: &str = r#"
id = "with-setup"
version = "0.1.0"
name = "With Setup"
description = "Plugin that needs post-install wiring"
setup = "Create a daily trigger that loads `knowhow/with-setup/run.md`. Suggested cron: `0 0 4 * * *`."
"#;
    let (_tmp, scratch) = fresh_workspace();
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
    let write = install_from_unpacked_with_bus(
        &scratch,
        &bus,
        &unpacked,
        InstallContext::plain(SourceType::Archive, false),
    )
    .await
    .expect("install must succeed");

    assert_eq!(
        write.summary, "Installed With Setup v0.1.0 (1 files).",
        "a manifest with setup must not widen the summary beyond the one line"
    );
}

#[test]
fn normalize_setup_trims_and_collapses_empty() {
    assert_eq!(normalize_setup(None), None);
    assert_eq!(normalize_setup(Some("")), None);
    assert_eq!(normalize_setup(Some("   \n\t ")), None);
    assert_eq!(normalize_setup(Some("  do x  ")), Some("do x".to_string()));
}

fn staged(text: Option<&str>, revision: u64, reason: Option<&str>) -> StagedSetup {
    StagedSetup {
        text: normalize_setup(text),
        revision,
        reason: reason.map(str::to_string),
    }
}

fn prior(text: Option<&str>, revision: u64, version: Option<&str>) -> PriorSetup {
    PriorSetup {
        text: normalize_setup(text),
        revision,
        version: version.map(str::to_string),
    }
}

fn update_from(from: Option<&str>, cause: PluginSetupCause) -> PluginSetupOccasion {
    PluginSetupOccasion::Update {
        from_version: from.map(str::to_string),
        cause,
    }
}

fn update(from: Option<&str>, cause: PluginSetupCause) -> Option<PluginSetupOccasion> {
    Some(update_from(from, cause))
}

fn revision_raised() -> PluginSetupCause {
    PluginSetupCause::RevisionRaised {
        revision: 2,
        reason: Some("The skill changed.".into()),
    }
}

fn both_causes() -> PluginSetupCause {
    PluginSetupCause::Both {
        revision: 2,
        reason: Some("The skill changed.".into()),
    }
}

#[test]
fn no_setup_in_the_staged_version_never_runs_setup() {
    let key = Some("wire up the key");
    assert_eq!(setup_occasion(&staged(None, 0, None), None), None);
    assert_eq!(
        setup_occasion(&staged(None, 0, None), Some(&prior(key, 0, Some("0.1.0")))),
        None
    );
    assert_eq!(
        setup_occasion(
            &staged(Some("   "), 0, None),
            Some(&prior(key, 0, Some("0.1.0")))
        ),
        None
    );
}

#[test]
fn a_fresh_install_with_setup_runs_it_whatever_the_revision() {
    let key = Some("wire up the key");
    let fresh = Some(PluginSetupOccasion::FreshInstall);
    assert_eq!(setup_occasion(&staged(key, 0, None), None), fresh);
    assert_eq!(setup_occasion(&staged(key, 5, None), None), fresh);
    assert_eq!(
        setup_occasion(&staged(key, 0, None), Some(&prior(None, 0, Some("0.1.0")))),
        update(Some("0.1.0"), PluginSetupCause::InstructionsChanged)
    );
}

#[test]
fn an_update_that_changed_the_setup_text_runs_setup_for_that_reason() {
    let old = prior(Some("wire up the key"), 0, Some("0.1.0"));
    assert_eq!(
        setup_occasion(
            &staged(Some("wire up the key AND the secret"), 0, None),
            Some(&old)
        ),
        update(Some("0.1.0"), PluginSetupCause::InstructionsChanged)
    );
}

#[test]
fn an_update_with_the_same_text_and_no_revision_change_runs_nothing() {
    let old = prior(Some("wire up the key"), 0, Some("0.1.0"));
    assert_eq!(
        setup_occasion(&staged(Some("  wire up the key  "), 0, None), Some(&old)),
        None
    );
    let old = prior(Some("wire up the key"), 3, Some("0.1.0"));
    assert_eq!(
        setup_occasion(
            &staged(Some("wire up the key"), 3, Some("Same.")),
            Some(&old)
        ),
        None
    );
}

#[test]
fn a_raised_revision_runs_setup_when_the_text_is_unchanged() {
    let old = prior(Some("copy the skill"), 0, Some("0.1.0"));
    assert_eq!(
        setup_occasion(
            &staged(Some("copy the skill"), 1, Some("The skill changed.")),
            Some(&old)
        ),
        update(
            Some("0.1.0"),
            PluginSetupCause::RevisionRaised {
                revision: 1,
                reason: Some("The skill changed.".into()),
            }
        )
    );
    assert_eq!(
        setup_occasion(&staged(Some("copy the skill"), 1, None), Some(&old)),
        update(
            Some("0.1.0"),
            PluginSetupCause::RevisionRaised {
                revision: 1,
                reason: None,
            }
        )
    );
}

#[test]
fn a_raised_revision_and_changed_text_run_setup_for_both_reasons() {
    let old = prior(Some("copy the skill"), 1, Some("0.1.0"));
    assert_eq!(
        setup_occasion(
            &staged(Some("copy the skill to the new folder"), 2, Some("Moved.")),
            Some(&old)
        ),
        update(
            Some("0.1.0"),
            PluginSetupCause::Both {
                revision: 2,
                reason: Some("Moved.".into()),
            }
        )
    );
}

#[test]
fn a_revision_carried_across_two_releases_runs_setup_once() {
    let text = Some("copy the skill");
    let raised = setup_occasion(
        &staged(text, 2, Some("Changed.")),
        Some(&prior(text, 1, Some("0.1.0"))),
    );
    assert!(raised.is_some(), "0.1.0 to 0.2.0 raises 1 to 2");
    let carried = setup_occasion(
        &staged(text, 2, Some("Changed.")),
        Some(&prior(text, 2, Some("0.2.0"))),
    );
    assert_eq!(carried, None, "0.2.0 to 0.3.0 keeps 2");
}

#[test]
fn a_lowered_or_removed_revision_runs_nothing() {
    let text = Some("copy the skill");
    let old = prior(text, 3, Some("0.3.0"));
    assert_eq!(setup_occasion(&staged(text, 2, None), Some(&old)), None);
    assert_eq!(setup_occasion(&staged(text, 0, None), Some(&old)), None);
}

fn super_slides() -> SeededPlugin {
    SeededPlugin {
        id: "super-slides".to_string(),
        name: "Super Slides".to_string(),
        version: "0.6.0".to_string(),
    }
}

fn setup_seed_origin(
    occasion: PluginSetupOccasion,
    actor: Option<&MessageOrigin>,
) -> Option<MessageOrigin> {
    match build_setup_thread_request(uuid::Uuid::new_v4(), &super_slides(), occasion, actor) {
        crate::engine::thread_queue::ThreadQueueRequest::SubThread { origin, .. } => origin,
        other => panic!("setup thread must be a SubThread, got: {other:?}"),
    }
}

#[test]
fn every_setup_occasion_seeds_a_typed_engine_origin() {
    let occasions = [
        PluginSetupOccasion::FreshInstall,
        update_from(Some("0.5.1"), PluginSetupCause::InstructionsChanged),
        update_from(None, PluginSetupCause::InstructionsChanged),
        update_from(Some("0.5.1"), revision_raised()),
        update_from(None, both_causes()),
    ];
    for occasion in occasions {
        let origin = setup_seed_origin(occasion.clone(), None);
        assert_eq!(
            origin,
            Some(MessageOrigin::engine(EngineReason::PluginSetup {
                plugin_id: "super-slides".to_string(),
                plugin_name: "Super Slides".to_string(),
                version: "0.6.0".to_string(),
                occasion,
                confirmed_on_device_id: None,
            }))
        );
        assert_eq!(
            origin.as_ref().map(MessageOrigin::mode),
            Some(crate::engine::thread_events::ActorMode::Engine)
        );
    }
}

#[test]
fn the_setup_seed_names_the_confirming_device_and_only_a_device() {
    let device = MessageOrigin::Device {
        device_id: "dev-1".to_string(),
    };
    let reason_device = |actor| match setup_seed_origin(PluginSetupOccasion::FreshInstall, actor) {
        Some(MessageOrigin::Engine {
            reason:
                EngineReason::PluginSetup {
                    confirmed_on_device_id,
                    ..
                },
        }) => confirmed_on_device_id,
        other => panic!("expected a plugin_setup engine origin, got: {other:?}"),
    };
    assert_eq!(reason_device(Some(&device)).as_deref(), Some("dev-1"));

    let api = MessageOrigin::Api {
        user_agent: Some("curl/8".to_string()),
        mode: crate::engine::thread_events::ActorMode::Human,
        source_thread_id: None,
        emitting_trigger_id: None,
    };
    assert_eq!(reason_device(Some(&api)), None);
    assert_eq!(reason_device(None), None);
}

#[test]
fn the_upstream_patch_seed_carries_a_typed_engine_origin() {
    let device = MessageOrigin::Device {
        device_id: "dev-1".to_string(),
    };
    let req = build_upstream_patch_thread_request(
        uuid::Uuid::new_v4(),
        &super_slides(),
        "artifacts/plugin-changes/super-slides/proposed-v0.6.0.patch",
        Some(&device),
    );
    assert!(req.is_attributed());
    match req {
        crate::engine::thread_queue::ThreadQueueRequest::SubThread { origin, .. } => {
            assert_eq!(
                origin,
                Some(MessageOrigin::engine(
                    EngineReason::PluginUpstreamProposal {
                        plugin_id: "super-slides".to_string(),
                        plugin_name: "Super Slides".to_string(),
                        version: "0.6.0".to_string(),
                        patch_path: "artifacts/plugin-changes/super-slides/proposed-v0.6.0.patch"
                            .to_string(),
                        confirmed_on_device_id: Some("dev-1".to_string()),
                    }
                ))
            );
        }
        other => panic!("upstream-patch thread must be a SubThread, got: {other:?}"),
    }
}

#[test]
fn setup_thread_request_is_a_subthread_bound_to_the_advertised_id() {
    let tid = uuid::Uuid::new_v4();
    let req = build_setup_thread_request(
        tid,
        &super_slides(),
        PluginSetupOccasion::FreshInstall,
        None,
    );
    match req {
        crate::engine::thread_queue::ThreadQueueRequest::SubThread {
            child_thread_id,
            title,
            prompt,
            pre_emitted_origin,
            ..
        } => {
            assert_eq!(
                child_thread_id, tid,
                "must bind the advertised setup_thread_id"
            );
            assert_eq!(title.as_deref(), Some("Set up Super Slides"));
            assert!(pre_emitted_origin.is_none());
            assert_eq!(prompt, "Set up the newly installed Super Slides plugin.");
            assert!(
                !prompt.contains("send_notification") && !prompt.contains("Walk the user"),
                "seed must not embed the agent meta-instructions, got: {prompt:?}"
            );
        }
        other => panic!("setup thread must be a SubThread, got: {other:?}"),
    }
}

#[test]
fn an_update_seeds_a_re_run_rather_than_a_first_install() {
    let tid = uuid::Uuid::new_v4();
    let req = build_setup_thread_request(
        tid,
        &super_slides(),
        update_from(Some("0.5.1"), PluginSetupCause::InstructionsChanged),
        None,
    );
    match req {
        crate::engine::thread_queue::ThreadQueueRequest::SubThread {
            child_thread_id,
            title,
            prompt,
            pre_emitted_origin,
            origin,
            ..
        } => {
            assert_eq!(child_thread_id, tid);
            assert!(pre_emitted_origin.is_none() && origin.is_some());
            assert_eq!(title.as_deref(), Some("Update Super Slides setup"));
            assert_eq!(
                prompt,
                "Set up Super Slides again: its setup instructions changed since \
                 version 0.5.1."
            );
            assert!(
                !prompt.contains("send_notification") && !prompt.contains("Walk the user"),
                "seed must not embed the agent meta-instructions, got: {prompt:?}"
            );
        }
        other => panic!("setup thread must be a SubThread, got: {other:?}"),
    }
}

#[test]
fn each_update_cause_seeds_its_own_wording() {
    let cases = [
        (
            update_from(Some("0.5.1"), revision_raised()),
            "Set up Super Slides again: version 0.6.0 asks to run its setup again \
             since version 0.5.1. Author's note: The skill changed.",
        ),
        (
            update_from(None, revision_raised()),
            "Set up Super Slides again: this update asks to run its setup again. \
             Author's note: The skill changed.",
        ),
        (
            update_from(
                Some("0.5.1"),
                PluginSetupCause::RevisionRaised {
                    revision: 2,
                    reason: None,
                },
            ),
            "Set up Super Slides again: version 0.6.0 asks to run its setup again \
             since version 0.5.1.",
        ),
        (
            update_from(Some("0.5.1"), both_causes()),
            "Set up Super Slides again: its setup instructions changed since version \
             0.5.1, and the author asks to run it again. Author's note: The skill changed.",
        ),
        (
            update_from(None, both_causes()),
            "Set up Super Slides again: this update changed its setup instructions, \
             and the author asks to run it again. Author's note: The skill changed.",
        ),
    ];
    for (occasion, expected) in cases {
        match build_setup_thread_request(uuid::Uuid::new_v4(), &super_slides(), occasion, None) {
            crate::engine::thread_queue::ThreadQueueRequest::SubThread {
                title,
                prompt,
                origin,
                ..
            } => {
                assert_eq!(prompt, expected);
                assert_eq!(title.as_deref(), Some("Update Super Slides setup"));
                assert!(origin.is_some());
            }
            other => panic!("setup thread must be a SubThread, got: {other:?}"),
        }
    }
}

#[test]
fn an_update_from_a_versionless_record_still_seeds_a_re_run() {
    let tid = uuid::Uuid::new_v4();
    let req = build_setup_thread_request(
        tid,
        &super_slides(),
        update_from(None, PluginSetupCause::InstructionsChanged),
        None,
    );
    match req {
        crate::engine::thread_queue::ThreadQueueRequest::SubThread { title, prompt, .. } => {
            assert_eq!(title.as_deref(), Some("Update Super Slides setup"));
            assert_eq!(
                prompt,
                "Set up Super Slides again: this update changed its setup instructions."
            );
            assert!(
                !prompt.contains("newly installed"),
                "an update must never seed the first-install line: {prompt:?}"
            );
        }
        other => panic!("setup thread must be a SubThread, got: {other:?}"),
    }
}

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

#[test]
fn a_git_subpath_that_escapes_the_clone_is_refused() {
    let (_tmp, scratch) = fresh_workspace();
    let (bare, _work) = make_local_git_plugin(&scratch, "escape", "escape-plugin", "0.1.0", "body");
    for subpath in ["../..", "/etc", "\\windows"] {
        let source = super::source::Source::Git {
            url: format!("file://{}", bare.display()),
            branch: None,
            subpath: Some(subpath.to_string()),
        };
        let refused = match fetch_source(&scratch, &source, &GitCredentials::none()) {
            Ok(_) => panic!("subpath {subpath:?} must be refused"),
            Err(e) => e,
        };
        assert!(
            refused.contains("rejected plugin subpath"),
            "{subpath:?}: {refused}"
        );
    }
}

#[tokio::test]
async fn latest_install_round_trips_id_version_source_and_files() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1");
    let unpacked = extract_to(&archive_dir, &archive);

    install_from_unpacked_with_bus(
        &scratch,
        &bus,
        &unpacked,
        InstallContext::plain(SourceType::Archive, false),
    )
    .await
    .expect("install must succeed");

    let installed = latest_install(&pool, "fixture-plugin")
        .await
        .expect("query must succeed")
        .expect("install record must be findable by manifest id, not 'unknown'");

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

    teardown_test_db(&db_name).await;
}

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

#[tokio::test]
async fn e2e_install_from_local_archive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_fixture_archive(&archive_dir, "v1-from-archive");

    let msg = install_from_source_with_bus(&scratch, &bus, archive.to_str().unwrap(), false)
        .await
        .expect("install from archive must succeed");
    assert_eq!(msg, "Installed Fixture Plugin v0.1.0 (2 files).");

    assert_eq!(
        std::fs::read_to_string(scratch.join("data/knowhow/fixture.md")).unwrap(),
        "v1-from-archive"
    );
    assert!(scratch.join("data/triggers/fixture/fixture.md").is_file());

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

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn e2e_install_from_local_git_source_records_source_url() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let (_tmp, scratch) = fresh_workspace();
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

    let installed = latest_install(&pool, "git-fixture-plugin")
        .await
        .unwrap()
        .expect("install record must exist");
    assert_eq!(installed.source(), Some(source_url.as_str()));
    assert_eq!(installed.version(), Some("0.1.0"));

    let events = read_events(&pool, "PluginInstalled", "git-fixture-plugin").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["data"]["source_type"], "git");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn e2e_check_plugin_updates_returns_real_data_after_install() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let (_tmp, scratch) = fresh_workspace();
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

    teardown_test_db(&db_name).await;
}

#[cfg(unix)]
#[tokio::test]
async fn e2e_check_plugin_updates_never_reads_a_symlinked_manifest() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    let repos_dir = scratch.join("repos");
    std::fs::create_dir_all(&repos_dir).unwrap();
    let (bare, work) = make_local_git_plugin(&repos_dir, "linked", "linked", "0.1.0", "b");
    let source_url = format!("file://{}", bare.display());
    install_from_source_with_bus(&scratch, &bus, &source_url, false)
        .await
        .expect("install");

    let secret = scratch.join("host-secret.txt");
    std::fs::write(&secret, "password: canary-secret-value-7f3a\n").unwrap();
    std::fs::remove_file(work.join("manifest.toml")).unwrap();
    std::os::unix::fs::symlink(&secret, work.join("manifest.toml")).unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-m", "link manifest"]);
    git(&work, &["push", "origin", "main"]);

    let report = check_plugin_updates_impl(&scratch, &pool, Some("linked".into())).await;
    assert!(!report.contains("canary-secret-value"), "{report}");
    let report: Vec<serde_json::Value> = serde_json::from_str(&report).unwrap();
    let error = report[0]["error"].as_str().unwrap_or_default();
    assert!(
        error.contains(&crate::core::plugins::ValidationError::MissingManifest.to_string()),
        "{}",
        report[0]
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn e2e_engine_requirement_follows_the_manifest_on_installed_and_check_updates() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let (_tmp, scratch) = fresh_workspace();
    let repos_dir = scratch.join("repos");
    std::fs::create_dir_all(&repos_dir).unwrap();
    let (bare, work) = make_local_git_plugin(&repos_dir, "floorless", "floorless", "0.1.0", "b");
    let source_url = format!("file://{}", bare.display());
    let id = Some("floorless".to_string());

    install_from_source_with_bus(&scratch, &bus, &source_url, false)
        .await
        .expect("install");
    let installed = installed_plugin_summaries(&pool, &scratch).await.unwrap();
    assert_eq!(installed[0].engine_requirement, None);
    let wire = serde_json::to_value(&installed[0]).unwrap();
    assert!(wire.get("engine_requirement").is_none(), "{wire}");
    let report = check_plugin_updates_impl(&scratch, &pool, id.clone()).await;
    let report: Vec<serde_json::Value> = serde_json::from_str(&report).unwrap();
    assert!(report[0]["engine_requirement"].is_null(), "{}", report[0]);
    assert_eq!(report[0]["engine_compatible"], true);

    let manifest = std::fs::read_to_string(work.join("manifest.toml")).unwrap();
    let declared = manifest.replace("source =", "engine = \">=0.1.0\"\nsource =");
    std::fs::write(work.join("manifest.toml"), declared).unwrap();
    git(&work, &["commit", "-am", "declare engine"]);
    git(&work, &["push", "origin", "main"]);
    let report = check_plugin_updates_impl(&scratch, &pool, id).await;
    let report: Vec<serde_json::Value> = serde_json::from_str(&report).unwrap();
    assert_eq!(report[0]["engine_requirement"], ">=0.1.0");

    install_from_source_with_bus(&scratch, &bus, &source_url, true)
        .await
        .expect("reinstall");
    let installed = installed_plugin_summaries(&pool, &scratch).await.unwrap();
    assert_eq!(installed[0].engine_requirement.as_deref(), Some(">=0.1.0"));

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn e2e_update_plugin_re_fetches_when_version_bumps() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let (_tmp, scratch) = fresh_workspace();
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

    install_from_source_with_bus(&scratch, &bus, &source_url, false)
        .await
        .expect("install v0.1.0");
    let knowhow_path = scratch.join("data/knowhow/update-fixture-plugin.md");
    assert_eq!(std::fs::read_to_string(&knowhow_path).unwrap(), "v1 body");

    bump_local_git_plugin(&work, "update-fixture-plugin", "0.1.0", "0.1.1", "v2 body");

    let msg = update_plugin_impl(&scratch, &bus, &pool, "update-fixture-plugin").await;
    assert!(
        msg.starts_with("Installed Local Git Plugin v0.1.1"),
        "update message: {}",
        msg
    );

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

    let again = update_plugin_impl(&scratch, &bus, &pool, "update-fixture-plugin").await;
    assert_eq!(again, "Already at latest (v0.1.1)");
    assert_eq!(
        read_events(&pool, "PluginInstalled", "update-fixture-plugin")
            .await
            .len(),
        2,
        "no-op update must not emit a third PluginInstalled"
    );

    teardown_test_db(&db_name).await;
}

const GIT_TRACK_MANIFEST: &str = r#"
id = "git-track-plugin"
version = "0.3.1"
name = "Git Track Plugin"
description = "test"
source = "https://github.com/x/git-track"
"#;

#[tokio::test]
async fn install_commits_written_files_so_working_tree_is_clean() {
    let (_tmp, scratch) = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_archive(
        &archive_dir,
        "git-track.lucidos-plugin",
        GIT_TRACK_MANIFEST,
        &[
            ("apps/git-track-plugin/index.html", b"<h1>hi</h1>"),
            (
                "apps/git-track-plugin/manifest.json",
                b"{\"name\":\"Git Track\"}",
            ),
            ("knowhow/git-track.md", b"# how"),
        ],
    );
    let unpacked = extract_to(&archive_dir, &archive);

    let bus = MockEventBus::new();
    let write = install_from_unpacked_with_bus(
        &scratch,
        &bus,
        &unpacked,
        InstallContext::plain(SourceType::Archive, false),
    )
    .await
    .expect("install must succeed");

    let repo = git2::Repository::open(&scratch).expect("workspace must be a git repo");

    for rel in &write.installed_files {
        let repo_path = format!("data/{}", rel);
        let status = repo
            .status_file(std::path::Path::new(&repo_path))
            .unwrap_or_else(|e| panic!("status for {}: {}", repo_path, e));
        assert!(
            status.is_empty(),
            "installed file {} must be tracked & clean after install, got status {:?}",
            repo_path,
            status
        );
    }

    let head_commit = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .expect("install must have created a commit (HEAD)");
    assert_eq!(
        head_commit.message().unwrap_or(""),
        "Install plugin: git-track-plugin v0.3.1",
        "install commit must name the plugin and version"
    );

    let tree = head_commit.tree().expect("commit tree");
    for rel in &write.installed_files {
        let repo_path = format!("data/{}", rel);
        assert!(
            tree.get_path(std::path::Path::new(&repo_path)).is_ok(),
            "install commit tree must contain {}",
            repo_path
        );
    }
    assert!(
        write
            .installed_files
            .contains(&"apps/git-track-plugin/index.html".to_string())
            && write
                .installed_files
                .contains(&"knowhow/git-track.md".to_string()),
        "fixture must span apps/ and knowhow/ to prove a single multi-dir commit"
    );
}

mod guard_tests {
    use super::*;

    #[test]
    fn both_setup_seeds_route_to_the_plugin_setup_knowhow() {
        use crate::engine::chat::process::PLUGIN_SETUP_RULE;

        let seed = |occasion| match build_setup_thread_request(
            uuid::Uuid::new_v4(),
            &super_slides(),
            occasion,
            None,
        ) {
            crate::engine::thread_queue::ThreadQueueRequest::SubThread { prompt, .. } => {
                prompt.to_lowercase()
            }
            other => panic!("setup thread must be a SubThread, got: {other:?}"),
        };

        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        let knowhow_path = repo.join("system-knowhow/plugin-setup.md");
        let knowhow = std::fs::read_to_string(&knowhow_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", knowhow_path.display()))
            .to_lowercase();
        let description = knowhow
            .split("\n---")
            .next()
            .expect("split always yields at least one element")
            .to_string();
        let route = PLUGIN_SETUP_RULE.to_lowercase();

        for word in ["set up", "install", "update"] {
            assert!(
                route.contains(word),
                "the system-prompt route drops {word:?}, so a setup seed may not \
             reach the knowhow:\n{PLUGIN_SETUP_RULE}"
            );
            assert!(
                description.contains(word),
                "system-knowhow/plugin-setup.md's description drops {word:?}, so \
             retrieval may not reach it:\n{description}"
            );
        }

        let occasions = [
            PluginSetupOccasion::FreshInstall,
            update_from(Some("0.5.1"), PluginSetupCause::InstructionsChanged),
            update_from(None, PluginSetupCause::InstructionsChanged),
            update_from(Some("0.5.1"), revision_raised()),
            update_from(None, revision_raised()),
            update_from(Some("0.5.1"), both_causes()),
            update_from(None, both_causes()),
        ];
        for occasion in occasions {
            let again = matches!(occasion, PluginSetupOccasion::Update { .. });
            let prompt = seed(occasion);
            assert!(
                !again || prompt.contains("again"),
                "an update seed drops the word the knowhow keys on: {prompt:?}"
            );
            assert!(
                prompt.contains("set up"),
                "the seed drops the verb every route keys on: {prompt:?}"
            );
        }
        assert!(
            seed(PluginSetupOccasion::FreshInstall).contains("newly installed"),
            "the fresh-install seed must keep the phrase the route names"
        );

        assert!(
            route.contains("`system-knowhow/plugin-setup`")
                || route.contains("'system-knowhow/plugin-setup'"),
            "the route must name the knowhow id it loads:\n{PLUGIN_SETUP_RULE}"
        );
    }
}
