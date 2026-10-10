use super::*;

fn declaration(toml_text: &str) -> MediaDeclaration {
    let table: toml::Table = toml::from_str(toml_text).unwrap();
    MediaDeclaration::from_manifest(&table)
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn reasons(index: &MediaIndex) -> Vec<(String, String)> {
    index
        .problems
        .iter()
        .map(|p| (p.path.clone(), p.reason.clone()))
        .collect()
}

fn problem_for<'a>(index: &'a MediaIndex, path: &str) -> &'a str {
    &index
        .problems
        .iter()
        .find(|p| p.path == path)
        .unwrap_or_else(|| panic!("no problem for {path}: {:?}", reasons(index)))
        .reason
}

/// A value of the wrong type is named and ignored, never a parse failure.
#[test]
fn a_wrong_type_in_the_manifest_is_a_problem_not_a_failure() {
    let declared =
        declaration("icon = 3\nscreenshots = \"media/a.png\"\nvideos = [\"media/v.mp4\", 4]");
    assert_eq!(declared.icon, None);
    assert!(declared.screenshots.is_empty());
    assert_eq!(declared.videos, vec!["media/v.mp4"]);
    let keys: Vec<_> = declared.problems.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(keys, vec!["icon", "screenshots", "videos"]);
}

/// Invariant I9: a missing, oversize, wrong-type or misplaced entry is dropped
/// and named, and the rest of the media is still kept.
#[test]
fn a_bad_entry_is_dropped_with_its_reason() {
    let tree = tempfile::tempdir().unwrap();
    let root = tree.path();
    write(root, "media/icon.svg", b"<svg/>");
    write(root, "media/ok.png", b"png");
    write(root, "media/anim.gif", b"GIF89a");
    write(root, "media/huge.png", &vec![0; 5 * 1024 * 1024]);
    write(root, "apps/a/outside.png", b"png");
    let declared = declaration(
        r#"
icon = "media/icon.svg"
screenshots = ["media/ok.png", "media/anim.gif", "media/huge.png", "media/gone.png",
               "media/./ok.png", "media//ok.png", "apps/a/outside.png", "media/../apps/a/outside.png", "/etc/hosts"]
"#,
    );
    let media = resolve(root, &declared);
    assert_eq!(media.index.icon.as_deref(), Some("media/icon.svg"));
    assert_eq!(media.index.screenshots, vec!["media/ok.png"]);
    assert!(problem_for(&media.index, "media/anim.gif").contains("png, jpg, jpeg, webp"));
    assert_eq!(
        problem_for(&media.index, "media/huge.png"),
        "is 5.0 MB, over the 4.0 MB screenshot limit"
    );
    assert_eq!(problem_for(&media.index, "media/gone.png"), "is missing");
    for outside in [
        "media/./ok.png",
        "media//ok.png",
        "apps/a/outside.png",
        "media/../apps/a/outside.png",
        "/etc/hosts",
    ] {
        assert_eq!(
            problem_for(&media.index, outside),
            "must name a file inside media/"
        );
    }
}

/// Invariant I12: the count and total caps hold, and say which limit.
#[test]
fn the_count_and_total_caps_hold() {
    let tree = tempfile::tempdir().unwrap();
    let root = tree.path();
    let mut listed = Vec::new();
    for n in 0..=MAX_SCREENSHOTS {
        write(root, &format!("media/s{n}.png"), b"png");
        listed.push(format!("\"media/s{n}.png\""));
    }
    let big = vec![0; MAX_VIDEO_BYTES as usize];
    for n in 0..MAX_VIDEOS {
        write(root, &format!("media/v{n}.mp4"), &big);
    }
    let declared = declaration(&format!(
        "screenshots = [{}]\nvideos = [\"media/v0.mp4\", \"media/v1.mp4\", \"media/v2.mp4\"]",
        listed.join(", ")
    ));
    let media = resolve(root, &declared);
    assert_eq!(media.index.screenshots.len(), MAX_SCREENSHOTS);
    let past = format!("media/s{MAX_SCREENSHOTS}.png");
    assert_eq!(
        problem_for(&media.index, &past),
        format!("is past the {MAX_SCREENSHOTS} screenshot limit")
    );
    assert_eq!(media.index.videos, vec!["media/v0.mp4", "media/v1.mp4"]);
    assert!(problem_for(&media.index, "media/v2.mp4").contains("total limit"));
}

#[cfg(unix)]
#[test]
fn a_symlink_is_never_media() {
    let tree = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "secret.png", b"png");
    std::fs::create_dir_all(tree.path().join("media")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.png"),
        tree.path().join("media/a.png"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path(), tree.path().join("media/dir")).unwrap();
    let declared = declaration("screenshots = [\"media/a.png\", \"media/dir/secret.png\"]");
    let media = resolve(tree.path(), &declared);
    assert!(media.index.screenshots.is_empty());
    assert!(problem_for(&media.index, "media/a.png").contains("symlink"));
    assert!(problem_for(&media.index, "media/dir/secret.png").contains("symlink"));
}

fn write_app(root: &Path, id: &str, kind: Option<&str>, icon: bool) {
    let mut manifest = serde_json::json!({ "name": id });
    if let Some(kind) = kind {
        manifest["kind"] = kind.into();
    }
    if icon {
        manifest["icon"] = "assets/icon.svg".into();
        write(root, &format!("apps/{id}/assets/icon.svg"), b"<svg/>");
    }
    write(
        root,
        &format!("apps/{id}/manifest.json"),
        manifest.to_string().as_bytes(),
    );
}

/// With no plugin icon, the icon of the plugin's single app stands in. Its
/// widgets do not count as apps.
#[test]
fn the_single_app_icon_stands_in() {
    let tree = tempfile::tempdir().unwrap();
    write_app(tree.path(), "board", None, true);
    write_app(tree.path(), "board-chip", Some("widget"), true);
    let media = resolve(tree.path(), &MediaDeclaration::default());
    assert_eq!(
        media.index.icon.as_deref(),
        Some("apps/board/assets/icon.svg")
    );

    write_app(tree.path(), "second", None, true);
    let media = resolve(tree.path(), &MediaDeclaration::default());
    assert_eq!(media.index.icon, None, "two apps have no single icon");
}

/// Invariant I13: a copy serves only what its index lists.
#[test]
fn a_copy_serves_only_listed_files() {
    let tree = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    write(tree.path(), "media/a.png", b"png");
    write(tree.path(), "media/README.md", b"# Hello");
    let media = resolve(tree.path(), &declaration("screenshots = [\"media/a.png\"]"));
    let copy = installed_dir(workspace.path(), "p");
    write_copy(&media, &copy).unwrap();
    std::fs::write(copy.join("files/media/unlisted.html"), "<script>").unwrap();

    assert!(listed_file(&copy, "media/a.png").is_some());
    assert!(listed_file(&copy, "media/README.md").is_some());
    assert_eq!(listed_file(&copy, "media/unlisted.html"), None);
    assert_eq!(listed_file(&copy, "index.json"), None);
    assert_eq!(listed_file(&copy, "../p/index.json"), None);
}

/// Invariant I11: a new copy replaces the old one whole, so no stale
/// screenshot survives an update, and no media at all leaves no copy.
#[test]
fn a_new_copy_replaces_the_old_one_whole() {
    let tree = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let copy = installed_dir(workspace.path(), "p");
    write(tree.path(), "media/old.png", b"old");
    write_copy(
        &resolve(
            tree.path(),
            &declaration("screenshots = [\"media/old.png\"]"),
        ),
        &copy,
    )
    .unwrap();
    assert!(copy.join("files/media/old.png").exists());

    write(tree.path(), "media/new.png", b"new");
    let next = resolve(
        tree.path(),
        &declaration("screenshots = [\"media/new.png\"]"),
    );
    write_copy(&next, &copy).unwrap();
    assert!(!copy.join("files/media/old.png").exists());
    assert_eq!(read_index(&copy), Some(next.index));

    write_copy(&resolve(tree.path(), &MediaDeclaration::default()), &copy).unwrap();
    assert!(!copy.exists());
}

/// A rescan of unchanged media leaves the copy untouched.
#[test]
fn unchanged_media_is_not_rewritten() {
    let tree = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    write(tree.path(), "media/a.png", b"png");
    let declared = declaration("screenshots = [\"media/a.png\"]");
    let copy = cache_dir(workspace.path(), "m", "p", "1.0.0");
    write_copy(&resolve(tree.path(), &declared), &copy).unwrap();
    let marker = copy.join("files/marker");
    std::fs::write(&marker, "kept").unwrap();
    write_copy(&resolve(tree.path(), &declared), &copy).unwrap();
    assert!(
        marker.exists(),
        "an equal index means nothing was rewritten"
    );

    write(tree.path(), "media/a.png", b"changed");
    write_copy(&resolve(tree.path(), &declared), &copy).unwrap();
    assert!(!marker.exists(), "changed bytes change the digest");

    std::fs::remove_file(copy.join("files/media/a.png")).unwrap();
    write_copy(&resolve(tree.path(), &declared), &copy).unwrap();
    assert!(
        listed_file(&copy, "media/a.png").unwrap().is_file(),
        "a copy that lost a listed file is rewritten"
    );
}

/// Invariant I19: the cache keeps what the scan needs, drops the rest, and
/// leaves a failed marketplace alone.
#[test]
fn pruning_keeps_what_the_scan_needs() {
    let workspace = tempfile::tempdir().unwrap();
    for (mp, plugin, version) in [
        ("m1", "p", "1.0.0"),
        ("m1", "p", "0.9.0"),
        ("m2", "q", "1.0.0"),
        ("gone", "r", "1.0.0"),
    ] {
        std::fs::create_dir_all(cache_dir(workspace.path(), mp, plugin, version)).unwrap();
    }
    let set = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
    let keep = BTreeSet::from([("m1".into(), "p".into(), "1.0.0".into())]);
    prune_cache(workspace.path(), &set(&["m1", "m2"]), &set(&["m1"]), &keep);

    assert!(cache_dir(workspace.path(), "m1", "p", "1.0.0").exists());
    assert!(!cache_dir(workspace.path(), "m1", "p", "0.9.0").exists());
    assert!(
        cache_dir(workspace.path(), "m2", "q", "1.0.0").exists(),
        "m2 failed this scan"
    );
    assert!(!workspace.path().join(CACHE_DIR).join("gone").exists());
}

#[test]
fn urls_name_the_copy_and_encode_each_segment() {
    let index = MediaIndex {
        icon: Some("media/my icon.svg".into()),
        screenshots: vec!["media/a.png".into()],
        readme: Some(README_PATH.into()),
        ..MediaIndex::default()
    };
    let catalog = MediaSource::Catalog {
        marketplace_id: "m-1",
        plugin_id: "p",
        version: "1.0.0+build",
    }
    .media(&index);
    assert_eq!(
        catalog.icon_url.as_deref(),
        Some("/api/v1/plugins/media/catalog/m-1/p/1.0.0%2Bbuild/media/my%20icon.svg")
    );
    let installed = MediaSource::Installed { plugin_id: "p" }.media(&index);
    assert_eq!(
        installed.readme_url.as_deref(),
        Some("/api/v1/plugins/media/installed/p/media/README.md")
    );
    assert_eq!(
        installed.screenshots,
        vec!["/api/v1/plugins/media/installed/p/media/a.png"]
    );
}

/// Invariant I8: installed media stays out of the workspace's git history.
#[test]
fn installed_media_is_gitignored() {
    let entry = format!("{}/{}/", crate::core::DATA_DIR, INSTALLED_DIR);
    assert!(crate::core::WORKSPACE_GITIGNORE_ENTRIES.contains(&entry.as_str()));
}

#[test]
fn removing_a_marketplace_cache_takes_only_that_marketplace() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cache_dir(workspace.path(), "m1", "p", "1.0.0")).unwrap();
    std::fs::create_dir_all(cache_dir(workspace.path(), "m2", "q", "1.0.0")).unwrap();
    remove_marketplace_cache(workspace.path(), "m1").unwrap();
    remove_marketplace_cache(workspace.path(), "..").unwrap();
    assert!(!workspace.path().join(CACHE_DIR).join("m1").exists());
    assert!(cache_dir(workspace.path(), "m2", "q", "1.0.0").exists());
}

/// `plugins.md` states the limits to authors. It restates these constants, so
/// this pins the copy (`.claude/rules/one-definition-per-value.md`).
#[test]
fn the_plugins_knowhow_states_the_limits() {
    let doc = include_str!("../../../../system-knowhow/plugins.md");
    let mb = |b: u64| b / (1024 * 1024);
    let kb = |b: u64| b / 1024;
    for expected in [
        format!("{} KB, the app icon's limit", kb(APP_ICON_MAX_BYTES)),
        format!(
            "{} MB, at most {}",
            mb(MAX_SCREENSHOT_BYTES),
            MAX_SCREENSHOTS
        ),
        format!("{} MB, at most {}", mb(MAX_VIDEO_BYTES), MAX_VIDEOS),
        format!("| {} KB |", kb(MAX_README_BYTES)),
        format!("| {} MB |", mb(MAX_TOTAL_BYTES)),
    ] {
        assert!(doc.contains(&expected), "plugins.md lost {expected:?}");
    }
}
