//! Plugin media through install, update and uninstall (ADR 0414).

use super::helpers::*;
use super::*;
use crate::core::plugin_media::{self, installed_dir, listed_file, read_index};
use crate::engine::event_bus::MockEventBus;

const MANIFEST: &str = r#"
id = "media-plugin"
version = "1.0.0"
name = "Media Plugin"
description = "test"
icon = "media/icon.svg"
screenshots = ["media/one.png", "media/huge.png"]
"#;

async fn install(scratch: &Path, manifest: &str, files: &[(&str, &[u8])]) -> InstallWrite {
    let archive_dir = scratch.join(format!("archive-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_archive(&archive_dir, "p.lucidos-plugin", manifest, files);
    let unpacked = extract_to(&archive_dir, &archive);
    install_from_unpacked_with_bus(
        scratch,
        &MockEventBus::new(),
        &unpacked,
        InstallContext::plain(SourceType::Archive, true),
    )
    .await
    .expect("install")
}

fn huge() -> Vec<u8> {
    vec![0; plugin_media::MAX_SCREENSHOT_BYTES as usize + 1]
}

/// Invariants I8 and I9: media lands in its own copy, never in the file list
/// or the install commit, and an oversize screenshot is dropped, not fatal.
#[tokio::test]
async fn install_keeps_media_outside_the_workspace_content() {
    let scratch = fresh_workspace();
    crate::core::ensure_workspace_gitignore_entries(&scratch).unwrap();
    let huge = huge();
    let write = install(
        &scratch,
        MANIFEST,
        &[
            ("knowhow/media.md", b"# how"),
            ("media/icon.svg", b"<svg/>"),
            ("media/one.png", b"png"),
            ("media/huge.png", &huge),
        ],
    )
    .await;

    assert_eq!(write.installed_files, vec!["knowhow/media.md"]);
    let copy = installed_dir(&scratch, "media-plugin");
    let index = read_index(&copy).expect("an installed media copy");
    assert_eq!(index.icon.as_deref(), Some("media/icon.svg"));
    assert_eq!(index.screenshots, vec!["media/one.png"]);
    assert_eq!(index.problems.len(), 1);
    assert_eq!(index.problems[0].path, "media/huge.png");

    let repo = git2::Repository::open(&scratch).unwrap();
    let tree = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap();
    assert!(tree.get_path(Path::new("data/plugin-media")).is_err());
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true);
    let untracked: Vec<String> = repo
        .statuses(Some(&mut opts))
        .unwrap()
        .iter()
        .filter_map(|e| e.path().map(str::to_string))
        .filter(|p| p.starts_with("data/"))
        .collect();
    assert!(
        untracked.is_empty(),
        "media must be gitignored: {untracked:?}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Invariant I11: an update swaps the copy whole and uninstall deletes it.
#[tokio::test]
async fn update_replaces_media_and_uninstall_removes_it() {
    let scratch = fresh_workspace();
    install(
        &scratch,
        MANIFEST,
        &[
            ("knowhow/media.md", b"# how"),
            ("media/icon.svg", b"<svg/>"),
            ("media/one.png", b"png"),
        ],
    )
    .await;
    let copy = installed_dir(&scratch, "media-plugin");
    assert!(listed_file(&copy, "media/one.png").is_some());

    let next = MANIFEST.replace("1.0.0", "1.1.0").replace(
        "[\"media/one.png\", \"media/huge.png\"]",
        "[\"media/two.png\"]",
    );
    let write = install(
        &scratch,
        &next,
        &[("knowhow/media.md", b"# how"), ("media/two.png", b"png")],
    )
    .await;
    assert!(
        !copy.join("files/media/one.png").exists(),
        "no stale screenshot"
    );
    assert!(listed_file(&copy, "media/two.png").is_some());

    let pending = PendingUninstall {
        plugin_id: "media-plugin".into(),
        plugin_version: "1.1.0".into(),
        plugin_name: "Media Plugin".into(),
        files_present: write.installed_files,
        files_missing: Vec::new(),
        created_at: chrono::Utc::now(),
    };
    uninstall_with_bus(&scratch, &MockEventBus::new(), &pending, None)
        .await
        .expect("uninstall");
    assert!(!copy.exists());
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Invariant I10: installed media does not depend on the marketplace's cache.
#[tokio::test]
async fn installed_media_outlives_the_marketplace() {
    let scratch = fresh_workspace();
    install(
        &scratch,
        MANIFEST,
        &[
            ("knowhow/media.md", b"# how"),
            ("media/icon.svg", b"<svg/>"),
        ],
    )
    .await;
    let cached = plugin_media::cache_dir(&scratch, "gone-market", "media-plugin", "1.0.0");
    std::fs::create_dir_all(&cached).unwrap();

    plugin_media::prune_cache(
        &scratch,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    );

    assert!(!cached.exists(), "the removed marketplace's cache goes");
    let copy = installed_dir(&scratch, "media-plugin");
    assert!(listed_file(&copy, "media/icon.svg").is_some());
    let _ = std::fs::remove_dir_all(&scratch);
}
