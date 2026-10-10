//! Plugin uninstall flow tests: confirm-flow delete, conflict/overwrite/
//! uninstall round-trip, path-safety + empty-parent pruning, cancel paths.

use super::helpers::*;
use super::*;

use crate::engine::event_bus::{EventBus, MockEventBus};
use crate::test_support::{setup_test_db, teardown_test_db};

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
    let sentinel =
        prepare_uninstall_plugin(&scratch, &pool, &pending_uninstalls, "fixture-plugin").await;
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
        outcome
            .summary
            .contains("Uninstalled Fixture Plugin v0.1.0"),
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
    let write = install_from_unpacked_with_bus(
        &workspace,
        &bus,
        &unpacked,
        InstallContext::plain(SourceType::Archive, false),
    )
    .await
    .expect("install should succeed on empty workspace");
    assert_eq!(write.summary, "Installed Fixture Plugin v0.1.0 (2 files).");

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
        InstallContext::plain(SourceType::Archive, false),
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
    let write2 = install_from_unpacked_with_bus(
        &workspace,
        &bus,
        &unpacked_v2,
        InstallContext::plain(SourceType::Archive, true),
    )
    .await
    .expect("overwrite install should succeed");
    assert_eq!(write2.summary, "Installed Fixture Plugin v0.1.0 (2 files).");
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
    let recorded_files = match &bus.emitted_events()[1] {
        BusEvent::System(SystemEvent::PluginInstalled { files, .. }) => files.clone(),
        other => panic!("expected PluginInstalled at index 1, got {:?}", other),
    };
    let pending = PendingUninstall {
        plugin_id: "fixture-plugin".to_string(),
        plugin_version: "0.1.0".to_string(),
        plugin_name: "Fixture Plugin".to_string(),
        files_present: recorded_files.clone(),
        files_missing: Vec::new(),
        created_at: chrono::Utc::now(),
    };
    let outcome = uninstall_with_bus(&workspace, &bus, &pending, None)
        .await
        .expect("uninstall should succeed");
    assert!(
        outcome
            .summary
            .contains("Uninstalled Fixture Plugin v0.1.0"),
        "uninstall summary: {}",
        outcome.summary
    );
    assert_eq!(outcome.files_deleted.len(), 2);
    // Files actually deleted now.
    assert!(!kn_path.exists(), "uninstall must delete the knowhow file");
    assert!(
        !trig_path.exists(),
        "uninstall must delete the trigger file"
    );

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
            assert!(
                files_missing.is_empty(),
                "all files were present at confirm"
            );
        }
        other => panic!("expected PluginUninstalled, got {:?}", other),
    }

    let _ = std::fs::remove_dir_all(&scratch);
}

/// Regression test for the uninstall half of the plugin git-tracking bug:
/// uninstall deleted the plugin's files from `data/` but never committed the
/// deletion, leaving the removal unrecorded in git history. This is the
/// symmetric counterpart of `install_commits_written_files_so_working_tree_is_clean`.
///
/// Installs (which now commits the files), then uninstalls and asserts the
/// removal lands in ONE commit, the files leave the HEAD tree, and the working
/// tree is left clean (no uncommitted deletions / ghosts).
#[tokio::test]
async fn uninstall_commits_deletions_so_working_tree_is_clean() {
    const MANIFEST: &str = r#"
id = "uninstall-track-plugin"
version = "1.2.0"
name = "Uninstall Track Plugin"
description = "test"
source = "https://github.com/x/uninstall-track"
"#;
    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_archive(
        &archive_dir,
        "uninstall-track.lucidos-plugin",
        MANIFEST,
        &[
            ("apps/uninstall-track-plugin/index.html", b"<h1>hi</h1>"),
            ("knowhow/uninstall-track.md", b"# how"),
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
    .expect("install");

    // Sanity: install committed the files (covered in depth by the install
    // regression test) — so the uninstall has a tracked deletion to make.
    let repo = git2::Repository::open(&scratch).unwrap();
    for rel in &write.installed_files {
        assert!(
            repo.status_file(std::path::Path::new(&format!("data/{}", rel)))
                .unwrap()
                .is_empty(),
            "precondition: install must leave {} tracked & clean",
            rel
        );
    }

    let pending = PendingUninstall {
        plugin_id: "uninstall-track-plugin".to_string(),
        plugin_version: "1.2.0".to_string(),
        plugin_name: "Uninstall Track Plugin".to_string(),
        files_present: write.installed_files.clone(),
        files_missing: Vec::new(),
        created_at: chrono::Utc::now(),
    };
    let outcome = uninstall_with_bus(&scratch, &bus, &pending, None)
        .await
        .expect("uninstall");
    assert_eq!(outcome.files_deleted.len(), write.installed_files.len());

    // Files gone from disk.
    for rel in &write.installed_files {
        assert!(
            !scratch.join("data").join(rel).exists(),
            "uninstall must delete {}",
            rel
        );
    }

    // HEAD now records the uninstall, and the deleted files are gone from its
    // tree — the deletion is version-controlled, not just a dirty working tree.
    let head = repo.head().and_then(|h| h.peel_to_commit()).unwrap();
    assert_eq!(
        head.message().unwrap_or(""),
        "Uninstall plugin: uninstall-track-plugin v1.2.0",
        "uninstall commit must name the plugin and version"
    );
    let tree = head.tree().unwrap();
    for rel in &write.installed_files {
        assert!(
            tree.get_path(std::path::Path::new(&format!("data/{}", rel)))
                .is_err(),
            "uninstall commit tree must NOT contain {}",
            rel
        );
    }

    // Working tree is clean: no staged-or-unstaged deletions left dangling.
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(false);
    let dirty: Vec<String> = repo
        .statuses(Some(&mut opts))
        .unwrap()
        .iter()
        .filter_map(|e| e.path().map(str::to_string))
        .collect();
    assert!(
        dirty.is_empty(),
        "working tree must be clean after uninstall commit, dirty: {:?}",
        dirty
    );

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

    assert!(
        !data_dir.join("apps/myapp/sub").exists(),
        "sub must be pruned"
    );
    assert!(
        !data_dir.join("apps/myapp").exists(),
        "myapp must be pruned"
    );
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
        outcome
            .files_missing
            .contains(&"knowhow/already-gone.md".to_string()),
        "raced-missing file must end up in files_missing"
    );
    assert!(!data_dir.join("knowhow/stays.md").exists());

    let _ = std::fs::remove_dir_all(&scratch);
}
