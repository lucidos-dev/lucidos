//! Plugin widgets through install and uninstall (ADR 0414).

use super::helpers::*;
use super::*;
use crate::engine::event_bus::MockEventBus;

const MANIFEST: &str = r#"
id = "habit-tracker"
version = "1.0.0"
name = "Habit Tracker"
description = "test"
"#;

const WIDGET: &[u8] = br#"{"name": "Board", "kind": "widget", "reusable": true, "origin_plugin_id": "habit-tracker"}"#;

/// Invariant I14: install lands a widget its plugin owns, and uninstall takes
/// it away with the rest of the plugin.
#[tokio::test]
async fn uninstall_removes_the_plugins_widget() {
    let scratch = fresh_workspace();
    let archive_dir = scratch.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_archive(
        &archive_dir,
        "habit-tracker.lucidos-plugin",
        MANIFEST,
        &[
            ("apps/board/manifest.json", WIDGET),
            ("apps/board/index.html", b"<h1>board</h1>"),
        ],
    );
    let unpacked = extract_to(&archive_dir, &archive);
    let write = install_from_unpacked_with_bus(
        &scratch,
        &MockEventBus::new(),
        &unpacked,
        InstallContext::plain(SourceType::Archive, false),
    )
    .await
    .expect("install");

    let manager = crate::core::AppManager::new(&scratch).unwrap();
    let board = manager.get_app("board").unwrap();
    assert_eq!(board.origin_plugin(), Some("habit-tracker"));
    assert_eq!(manager.list_reusable_widgets().unwrap().len(), 1);

    let pending = PendingUninstall {
        plugin_id: "habit-tracker".into(),
        plugin_version: "1.0.0".into(),
        plugin_name: "Habit Tracker".into(),
        files_present: write.installed_files,
        files_missing: Vec::new(),
        created_at: chrono::Utc::now(),
    };
    uninstall_with_bus(&scratch, &MockEventBus::new(), &pending, None)
        .await
        .expect("uninstall");
    assert!(!scratch.join("data/apps/board").exists());
    let _ = std::fs::remove_dir_all(&scratch);
}
