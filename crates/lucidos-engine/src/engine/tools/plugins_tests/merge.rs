//! Keeping a user's local edits across a plugin update, and removing the files
//! the new version dropped.
//!
//! Each test drives the real staging-then-confirm sequence: plan the update
//! against the recorded baseline, resolve the panel's keep control, save aside
//! whatever will be discarded, then run the writer. Nothing here shortcuts to
//! the merge function alone, because the invariants that matter are about what
//! reaches disk and what reaches the install commit.

use super::super::helpers::{build_archive, extract_to, fresh_workspace};
use super::super::{install_from_unpacked_with_bus, InstallContext, LocalChangeWrites, SourceType};
use super::*;

use crate::core::plugins::validate_tree;
use crate::engine::event_bus::{BusEvent, MockEventBus, SystemEvent};
use crate::engine::tools::plugins::registry::PluginBaseline;
use crate::triggers::config::TriggerRun;
use crate::triggers::definition::TriggerDefinition;
use std::path::PathBuf;

/// The planner as staging calls it for a plugin that shares no path with
/// another one.
fn plan_for(ws: &Path, baseline: &PluginBaseline, planned: &[PlannedFile]) -> UpdatePlan {
    super::plan_update(
        ws,
        &baseline.files,
        Some(baseline),
        planned,
        &BTreeSet::new(),
    )
}

const MANIFEST_V1: &str = r#"
id = "demo"
version = "1.0.0"
name = "Demo"
description = "test"
source = "https://github.com/example-org/example-repo"
"#;

const MANIFEST_V2: &str = r#"
id = "demo"
version = "2.0.0"
name = "Demo"
description = "test"
source = "https://github.com/example-org/example-repo"
"#;

const MANIFEST_V3: &str = r#"
id = "demo"
version = "3.0.0"
name = "Demo"
description = "test"
source = "https://github.com/example-org/example-repo"
"#;

const NOTES: &str = "# Notes\nintro\nalpha\nbravo\ncharlie\ndelta\necho\nfoxtrot\n";

/// Upstream's v2: rewrites the intro, far from where the user works.
const NOTES_V2: &str = "# Notes\nintro (v2)\nalpha\nbravo\ncharlie\ndelta\necho\nfoxtrot\n";

/// Upstream's v3: rewrites the intro again, still far from the user's hunk.
const NOTES_V3: &str = "# Notes\nintro (v3)\nalpha\nbravo\ncharlie\ndelta\necho\nfoxtrot\n";

/// The user's patch: a section appended at the end.
const USER_SECTION: &str = "\n## Outlook snapshot\npeek-safe, EXAMINE only\n";

const TRIGGER_TOML: &str = "name = \"Watcher\"\n\n[[on]]\nevent_type = \"FooHappened\"\n\n[run]\ntype = \"intent\"\nintent = \"do the thing\"\n";

fn notes_with_user_section(body: &str) -> String {
    format!("{}{}", body, USER_SECTION)
}

fn data_path(ws: &Path, rel: &str) -> PathBuf {
    ws.join(crate::core::DATA_DIR).join(rel)
}

fn read_data(ws: &Path, rel: &str) -> String {
    std::fs::read_to_string(data_path(ws, rel)).expect("read data file")
}

fn write_data(ws: &Path, rel: &str, body: &str) {
    let p = data_path(ws, rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Unpack a version's manifest + files into a staged plugin root, exactly as
/// `fetch_source` would leave it.
fn stage(ws: &Path, tag: &str, manifest: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = ws.join("staging").join(tag);
    std::fs::create_dir_all(&dir).unwrap();
    let archive = build_archive(&dir, "v.lucidos-plugin", manifest, files);
    extract_to(&dir, &archive)
}

/// The install commit the writer recorded, read back off the emitted
/// `PluginInstalled` exactly as `InstalledRecord::commit` does in production.
fn recorded_install_commit(bus: &MockEventBus) -> String {
    bus.emitted_events()
        .iter()
        .rev()
        .find_map(|e| match e {
            BusEvent::System(SystemEvent::PluginInstalled { manifest, .. }) => manifest
                .get("commit")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            _ => None,
        })
        .expect("install must record its commit")
}

fn baseline(bus: &MockEventBus, files: &[&str]) -> PluginBaseline {
    PluginBaseline {
        commit: recorded_install_commit(bus),
        files: files.iter().map(|s| s.to_string()).collect(),
    }
}

/// The bytes the install commit recorded for `rel`. The pristine-baseline
/// invariant is about this, not about what is on disk.
fn committed_blob(ws: &Path, commit: &str, rel: &str) -> String {
    let repo = git2::Repository::open(ws).unwrap();
    let tree = repo
        .revparse_single(commit)
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap();
    let entry = tree
        .get_path(Path::new(&format!("data/{}", rel)))
        .unwrap_or_else(|e| panic!("{} must be in the install commit: {}", rel, e));
    let blob = repo.find_blob(entry.id()).unwrap();
    String::from_utf8(blob.content().to_vec()).unwrap()
}

/// Whether the install commit's tree holds `rel` at all.
fn in_commit(ws: &Path, commit: &str, rel: &str) -> bool {
    let repo = git2::Repository::open(ws).unwrap();
    let tree = repo
        .revparse_single(commit)
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap();
    tree.get_path(Path::new(&format!("data/{}", rel))).is_ok()
}

/// What a confirmed update did to the user's edits, for assertions.
#[derive(Debug)]
struct UpdateOutcome {
    merged: Vec<String>,
    conflicted: Vec<String>,
    replaced: Vec<String>,
    restored: Vec<String>,
    removed: Vec<String>,
    saved_paths: Vec<String>,
    /// Every path the staged preview said the confirm would delete.
    removes: Vec<String>,
    /// Every path the writer deleted.
    removed_files: Vec<String>,
}

/// Fresh install: no baseline, so no merge plan and no local changes.
async fn install_first(ws: &Path, bus: &MockEventBus, files: &[(&str, &[u8])]) {
    let root = stage(ws, "v1", MANIFEST_V1, files);
    install_from_unpacked_with_bus(
        ws,
        bus,
        &root,
        InstallContext::plain(SourceType::Archive, false),
    )
    .await
    .expect("first install must succeed");
}

/// The confirm sequence for an update, step for step as
/// `confirm_pending_install` runs it.
async fn update(
    ws: &Path,
    bus: &MockEventBus,
    manifest: &str,
    files: &[(&str, &[u8])],
    baseline: &PluginBaseline,
    keep_local_changes: bool,
) -> UpdateOutcome {
    update_over(
        ws,
        bus,
        manifest,
        files,
        &baseline.files,
        Some(baseline),
        keep_local_changes,
    )
    .await
}

/// [`update`] over any previous record, a legacy one with no baseline included.
async fn update_over(
    ws: &Path,
    bus: &MockEventBus,
    manifest: &str,
    files: &[(&str, &[u8])],
    recorded: &[String],
    baseline: Option<&PluginBaseline>,
    keep_local_changes: bool,
) -> UpdateOutcome {
    // The version doubles as the staging directory name, so a fixture names it
    // once and the two cannot drift.
    let version = crate::core::plugins::parse_manifest(manifest)
        .expect("fixture manifest must parse")
        .version;
    let root = stage(ws, &version, manifest, files);
    let (_, planned) = validate_tree(&root).expect("staged tree must validate");
    let plan = super::plan_update(ws, recorded, baseline, &planned, &BTreeSet::new());
    assert!(
        plan.detect_drift(ws).is_none(),
        "the plan was just built, so nothing can have drifted yet"
    );
    let resolved = plan.resolve(keep_local_changes);
    let saved_paths = super::save_discarded(ws, "demo", &version, &resolved).expect("save aside");
    let writes = LocalChangeWrites::from_resolved(&resolved, saved_paths.clone());
    let merged = writes.merged_paths();
    let conflicted = writes.conflicted.clone();
    let replaced = writes.replaced.clone();
    let restored = writes.restored.clone();
    let removed = writes.removed.clone();
    let removes = plan.removes();
    let write = install_from_unpacked_with_bus(
        ws,
        bus,
        &root,
        InstallContext {
            local: writes,
            removals: removes.clone(),
            ..InstallContext::plain(SourceType::Archive, true)
        },
    )
    .await
    .expect("update must succeed");
    UpdateOutcome {
        merged,
        conflicted,
        replaced,
        restored,
        removed,
        saved_paths,
        removes,
        removed_files: write.removed_files,
    }
}

/// The whole point: upstream changes the top of the file, the user owns the
/// bottom, and both survive.
#[tokio::test]
async fn clean_merge_keeps_the_user_hunk_and_takes_upstreams() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;

    write_data(&ws, "knowhow/notes.md", &notes_with_user_section(NOTES));
    let base = baseline(&bus, &["knowhow/notes.md"]);

    let outcome = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
        &base,
        true,
    )
    .await;

    assert_eq!(outcome.merged, vec!["knowhow/notes.md".to_string()]);
    assert!(outcome.conflicted.is_empty() && outcome.replaced.is_empty());
    let on_disk = read_data(&ws, "knowhow/notes.md");
    assert!(
        on_disk.contains("intro (v2)"),
        "upstream's change must land: {on_disk}"
    );
    assert!(
        on_disk.contains("## Outlook snapshot"),
        "the user's section must survive: {on_disk}"
    );
    assert!(
        !on_disk.contains("<<<<<<<"),
        "a clean merge must not write markers: {on_disk}"
    );
    assert!(
        outcome.saved_paths.is_empty(),
        "a kept edit needs no copy aside"
    );
}

/// The install commit is the next update's merge base, so it must hold
/// UPSTREAM's bytes even though the working tree holds the merge. Recording
/// the merge instead is what silently drops the patch one update later.
#[tokio::test]
async fn install_commit_records_upstream_not_merged() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    write_data(&ws, "knowhow/notes.md", &notes_with_user_section(NOTES));
    let base = baseline(&bus, &["knowhow/notes.md"]);

    update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
        &base,
        true,
    )
    .await;

    let commit = recorded_install_commit(&bus);
    assert_eq!(
        committed_blob(&ws, &commit, "knowhow/notes.md"),
        NOTES_V2,
        "the install commit must be a byte-exact copy of what v2 shipped"
    );
    assert!(
        read_data(&ws, "knowhow/notes.md").contains("## Outlook snapshot"),
        "while the working tree keeps the merge"
    );
}

/// The regression this design exists for. Update two must not eat the patch
/// that update one kept.
#[tokio::test]
async fn patch_carries_forward_across_two_updates() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    write_data(&ws, "knowhow/notes.md", &notes_with_user_section(NOTES));

    let base_v1 = baseline(&bus, &["knowhow/notes.md"]);
    let first = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
        &base_v1,
        true,
    )
    .await;
    assert_eq!(first.merged, vec!["knowhow/notes.md".to_string()]);

    let base_v2 = baseline(&bus, &["knowhow/notes.md"]);
    let second = update(
        &ws,
        &bus,
        MANIFEST_V3,
        &[("knowhow/notes.md", NOTES_V3.as_bytes())],
        &base_v2,
        true,
    )
    .await;

    assert_eq!(
        second.merged,
        vec!["knowhow/notes.md".to_string()],
        "the second update must still see a local patch to merge"
    );
    let on_disk = read_data(&ws, "knowhow/notes.md");
    assert!(
        on_disk.contains("intro (v3)"),
        "v3's change must land: {on_disk}"
    );
    assert!(
        on_disk.contains("## Outlook snapshot"),
        "the patch must survive a SECOND update: {on_disk}"
    );
}

/// Both sides rewrote the same line. Upstream wins on disk: a file carrying
/// conflict markers is an instruction the engine would act on. The user's
/// version is kept as a file plus a re-appliable patch.
#[tokio::test]
async fn conflict_writes_upstream_and_saves_ours_aside() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;

    let ours = NOTES.replace("charlie", "charlie, my way");
    write_data(&ws, "knowhow/notes.md", &ours);
    let theirs = NOTES.replace("charlie", "charlie, upstream's way");
    let base = baseline(&bus, &["knowhow/notes.md"]);

    let outcome = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", theirs.as_bytes())],
        &base,
        true,
    )
    .await;

    assert_eq!(outcome.conflicted, vec!["knowhow/notes.md".to_string()]);
    assert!(outcome.merged.is_empty());
    let on_disk = read_data(&ws, "knowhow/notes.md");
    assert_eq!(on_disk, theirs, "upstream must land verbatim on a conflict");
    assert!(!on_disk.contains("<<<<<<<"), "never write conflict markers");

    let saved = "artifacts/plugin-local-changes/demo/v2.0.0/knowhow/notes.md".to_string();
    assert!(
        outcome.saved_paths.contains(&saved),
        "the user's file must be saved aside: {:?}",
        outcome.saved_paths
    );
    assert_eq!(
        read_data(&ws, &saved),
        ours,
        "the saved copy must be their version verbatim"
    );
    let patch = read_data(&ws, &format!("{saved}.patch"));
    assert!(
        patch.contains("charlie, my way"),
        "the patch must carry their edit: {patch}"
    );
    assert!(
        outcome
            .saved_paths
            .iter()
            .any(|p| p.ends_with("v2.0.0/README.md")),
        "the folder must explain itself"
    );
}

/// `trigger.toml` is a re-serialized projection the engine rewrites after every
/// install, so its bytes never match what the plugin shipped. Text-merging it
/// would report a conflict for every plugin trigger on every update.
#[tokio::test]
async fn trigger_projection_is_never_merged() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    let files: &[(&str, &[u8])] = &[
        ("knowhow/notes.md", NOTES.as_bytes()),
        ("triggers/watch/trigger.toml", TRIGGER_TOML.as_bytes()),
    ];
    install_first(&ws, &bus, files).await;

    // Re-serialized and registration-stamped, exactly as the scheduler leaves
    // it: different bytes, same definition.
    let mut def = TriggerDefinition::from_toml(TRIGGER_TOML).unwrap();
    def.slug = "watch".to_string();
    def.plugin_id = Some("demo".to_string());
    let reserialized = def.to_toml().unwrap();
    assert_ne!(reserialized, TRIGGER_TOML, "test premise: bytes differ");
    write_data(&ws, "triggers/watch/trigger.toml", &reserialized);

    let base = baseline(&bus, &["knowhow/notes.md", "triggers/watch/trigger.toml"]);
    let outcome = update(&ws, &bus, MANIFEST_V2, files, &base, true).await;
    assert!(
        outcome.conflicted.is_empty() && outcome.merged.is_empty(),
        "a re-serialized projection is not a local edit: {outcome:?}"
    );

    // Now a REAL definition change. Still never a conflict, just replaced.
    let mut edited = TriggerDefinition::from_toml(TRIGGER_TOML).unwrap();
    edited.slug = "watch".to_string();
    edited.run = TriggerRun::Intent {
        intent: "do something else".to_string(),
    };
    write_data(
        &ws,
        "triggers/watch/trigger.toml",
        &edited.to_toml().unwrap(),
    );
    let base = baseline(&bus, &["knowhow/notes.md", "triggers/watch/trigger.toml"]);
    let outcome = update(&ws, &bus, MANIFEST_V3, files, &base, true).await;
    assert_eq!(
        outcome.replaced,
        vec!["triggers/watch/trigger.toml".to_string()]
    );
    assert!(outcome.conflicted.is_empty(), "never a conflict");
}

/// The common path must be untouched: no merge attempted, nothing saved aside,
/// and `data/artifacts/` left clean.
#[tokio::test]
async fn unmodified_file_is_plainly_replaced() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    let base = baseline(&bus, &["knowhow/notes.md"]);

    let outcome = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
        &base,
        true,
    )
    .await;

    assert!(outcome.merged.is_empty());
    assert!(outcome.conflicted.is_empty());
    assert!(outcome.replaced.is_empty());
    assert!(outcome.saved_paths.is_empty());
    assert_eq!(read_data(&ws, "knowhow/notes.md"), NOTES_V2);
    assert!(
        !data_path(&ws, "artifacts/plugin-local-changes").exists(),
        "a normal update must not create a saved-changes folder"
    );
}

/// The repro: the user edits files, publishes those edits upstream, then
/// updates back onto their own work. Every path they sent is byte-identical to
/// what the new version ships, so only the one they held back is a decision.
/// The panel counts what this plan holds, so an extra row is an extra question
/// about a file nothing can happen to.
#[tokio::test]
async fn a_file_already_equal_to_the_new_version_is_not_a_local_change() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    let files: &[(&str, &[u8])] = &[
        ("knowhow/notes.md", NOTES.as_bytes()),
        ("apps/demo/index.html", b"<h1>v1</h1>\n"),
    ];
    install_first(&ws, &bus, files).await;

    // Published upstream: their edit IS v2's content.
    let published = notes_with_user_section(NOTES);
    write_data(&ws, "knowhow/notes.md", &published);
    // Held back: still only theirs.
    write_data(&ws, "apps/demo/index.html", "<h1>mine</h1>\n");
    let base = baseline(&bus, &["knowhow/notes.md", "apps/demo/index.html"]);

    let root = stage(
        &ws,
        "2.0.0-preview",
        MANIFEST_V2,
        &[
            ("knowhow/notes.md", published.as_bytes()),
            ("apps/demo/index.html", b"<h1>v2</h1>\n"),
        ],
    );
    let (_, planned) = validate_tree(&root).unwrap();
    let plan = plan_for(&ws, &base, &planned);

    let listed: Vec<String> = plan
        .preview_json()
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        listed,
        vec!["apps/demo/index.html".to_string()],
        "only the file that really differs from upstream is a decision"
    );

    // And clearing the keep control acts on that one file alone: the published
    // edit is not "discarded", so nothing is copied aside for it.
    let outcome = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[
            ("knowhow/notes.md", published.as_bytes()),
            ("apps/demo/index.html", b"<h1>v2</h1>\n"),
        ],
        &base,
        false,
    )
    .await;
    assert_eq!(outcome.replaced, vec!["apps/demo/index.html".to_string()]);
    assert!(outcome.merged.is_empty() && outcome.conflicted.is_empty());
    assert_eq!(
        outcome.saved_paths,
        vec![
            "artifacts/plugin-local-changes/demo/v2.0.0/apps/demo/index.html".to_string(),
            "artifacts/plugin-local-changes/demo/v2.0.0/apps/demo/index.html.patch".to_string(),
            "artifacts/plugin-local-changes/demo/v2.0.0/README.md".to_string(),
        ],
        "a file already equal to upstream has nothing to save aside"
    );
    assert_eq!(
        read_data(&ws, "knowhow/notes.md"),
        published,
        "and it lands as the new version ships it, which is what it already was"
    );
}

/// A path the panel says nothing about is still watched. The user edited it
/// once, so they can edit it again while the panel sits open, and that edit is
/// real work. Silent in the list must not mean unguarded at confirm.
#[tokio::test]
async fn a_file_dropped_for_matching_upstream_is_still_drift_checked() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    let published = notes_with_user_section(NOTES);
    write_data(&ws, "knowhow/notes.md", &published);
    let base = baseline(&bus, &["knowhow/notes.md"]);

    let root = stage(
        &ws,
        "v2",
        MANIFEST_V2,
        &[("knowhow/notes.md", published.as_bytes())],
    );
    let (_, planned) = validate_tree(&root).unwrap();
    let plan = plan_for(&ws, &base, &planned);
    assert!(
        plan.preview_json().as_array().unwrap().is_empty(),
        "nothing to decide, so nothing to list"
    );
    assert!(
        plan.detect_drift(&ws).is_none(),
        "clean right after staging"
    );

    write_data(&ws, "knowhow/notes.md", "something else entirely\n");
    assert_eq!(
        plan.detect_drift(&ws).as_deref(),
        Some("knowhow/notes.md"),
        "an edit inside the staging window must still stop the confirm"
    );
}

/// The panel's keep control switched off: clean upstream everywhere, but the
/// discarded edit is still preserved rather than deleted.
#[tokio::test]
async fn keep_local_changes_false_replaces_and_saves_aside() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    let ours = notes_with_user_section(NOTES);
    write_data(&ws, "knowhow/notes.md", &ours);
    let base = baseline(&bus, &["knowhow/notes.md"]);

    let outcome = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
        &base,
        false,
    )
    .await;

    assert_eq!(outcome.replaced, vec!["knowhow/notes.md".to_string()]);
    assert!(outcome.merged.is_empty());
    assert_eq!(
        read_data(&ws, "knowhow/notes.md"),
        NOTES_V2,
        "opting out takes upstream verbatim"
    );
    assert_eq!(
        read_data(
            &ws,
            "artifacts/plugin-local-changes/demo/v2.0.0/knowhow/notes.md"
        ),
        ours,
        "and still keeps what it discarded"
    );
}

/// A binary file has no meaningful three-way merge whatever its size.
#[tokio::test]
async fn binary_file_is_replaced_not_merged() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("apps/demo/logo.bin", b"\x00\x01binary v1")]).await;
    std::fs::write(data_path(&ws, "apps/demo/logo.bin"), b"\x00\x01edited").unwrap();
    let base = baseline(&bus, &["apps/demo/logo.bin"]);

    let outcome = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("apps/demo/logo.bin", b"\x00\x01binary v2")],
        &base,
        true,
    )
    .await;

    assert_eq!(outcome.replaced, vec!["apps/demo/logo.bin".to_string()]);
    assert!(outcome.merged.is_empty() && outcome.conflicted.is_empty());
    // Saved aside by copying the live file, since a binary is never held in
    // the pending entry and has no patch to write beside it.
    let saved = data_path(
        &ws,
        "artifacts/plugin-local-changes/demo/v2.0.0/apps/demo/logo.bin",
    );
    assert_eq!(std::fs::read(&saved).unwrap(), b"\x00\x01edited");
    assert!(
        !saved.with_extension("bin.patch").exists(),
        "a binary has no unified diff to file beside it"
    );
}

/// A file the user deleted comes back, and reports as its own outcome. It must
/// NOT read as `replaced`: nothing is saved aside for a deletion, so the panel
/// would be promising a copy that does not exist.
#[tokio::test]
async fn a_locally_deleted_file_is_restored_not_replaced() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    std::fs::remove_file(data_path(&ws, "knowhow/notes.md")).unwrap();
    let base = baseline(&bus, &["knowhow/notes.md"]);

    let outcome = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
        &base,
        true,
    )
    .await;

    assert_eq!(outcome.restored, vec!["knowhow/notes.md".to_string()]);
    assert!(outcome.replaced.is_empty(), "a deletion is not a replace");
    assert!(
        outcome.saved_paths.is_empty(),
        "a deletion has no content to save aside"
    );
    assert_eq!(read_data(&ws, "knowhow/notes.md"), NOTES_V2);
    assert!(
        !data_path(&ws, "artifacts/plugin-local-changes").exists(),
        "and so leaves no saved-changes folder behind"
    );
}

/// Installing the same version twice must not write the second batch of
/// discarded edits over the first. That copy is the only place the first batch
/// still exists in the working tree, because the confirm has just overwritten
/// the live file.
#[tokio::test]
async fn a_second_save_of_one_version_never_overwrites_the_first() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;

    // Two reinstalls of the SAME version, each discarding a different edit.
    let first_edit = NOTES.replace("charlie", "charlie, my first way");
    write_data(&ws, "knowhow/notes.md", &first_edit);
    let theirs = NOTES.replace("charlie", "charlie, upstream's way");
    let base = baseline(&bus, &["knowhow/notes.md"]);
    let first = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", theirs.as_bytes())],
        &base,
        true,
    )
    .await;
    assert_eq!(first.conflicted, vec!["knowhow/notes.md".to_string()]);

    let second_edit = theirs.replace("delta", "delta, my second way");
    write_data(&ws, "knowhow/notes.md", &second_edit);
    let base = baseline(&bus, &["knowhow/notes.md"]);
    let second = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
        &base,
        false,
    )
    .await;
    assert_eq!(second.replaced, vec!["knowhow/notes.md".to_string()]);

    assert_eq!(
        read_data(
            &ws,
            "artifacts/plugin-local-changes/demo/v2.0.0/knowhow/notes.md"
        ),
        first_edit,
        "the first save must survive the second"
    );
    assert_eq!(
        read_data(
            &ws,
            "artifacts/plugin-local-changes/demo/v2.0.0-2/knowhow/notes.md"
        ),
        second_edit,
        "the second save goes to its own folder"
    );
}

/// A merged file must end up with the mode the shipped file carries, on disk
/// and in the install commit. `copy_atomic` gets that free from
/// `std::fs::copy`, so only the merged path can lose it. A plugin script
/// silently losing its executable bit on update would be hard to trace.
#[cfg(unix)]
#[tokio::test]
async fn a_merged_file_keeps_the_shipped_executable_bit() {
    use std::os::unix::fs::PermissionsExt;

    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("scripts/run.sh", NOTES.as_bytes())]).await;
    write_data(&ws, "scripts/run.sh", &notes_with_user_section(NOTES));
    let base = baseline(&bus, &["scripts/run.sh"]);

    // Upstream ships it executable. `build_archive` stores plain entries, so
    // mark the staged file after extraction, which is what a git clone of an
    // executable script leaves behind.
    let root = stage(
        &ws,
        "2.0.0",
        MANIFEST_V2,
        &[("scripts/run.sh", NOTES_V2.as_bytes())],
    );
    std::fs::set_permissions(
        root.join("scripts/run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let (_, planned) = validate_tree(&root).unwrap();
    let plan = plan_for(&ws, &base, &planned);
    let resolved = plan.resolve(true);
    let saved = super::save_discarded(&ws, "demo", "2.0.0", &resolved).unwrap();
    let writes = LocalChangeWrites::from_resolved(&resolved, saved);
    assert_eq!(writes.merged_paths(), vec!["scripts/run.sh".to_string()]);
    install_from_unpacked_with_bus(
        &ws,
        &bus,
        &root,
        InstallContext {
            local: writes,
            ..InstallContext::plain(SourceType::Archive, true)
        },
    )
    .await
    .expect("update must succeed");

    let mode = std::fs::metadata(data_path(&ws, "scripts/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert!(
        mode & 0o100 != 0,
        "a merged script must stay executable, got {:o}",
        mode
    );

    let repo = git2::Repository::open(&ws).unwrap();
    let entry = repo
        .revparse_single(&recorded_install_commit(&bus))
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap()
        .get_path(Path::new("data/scripts/run.sh"))
        .unwrap();
    assert_eq!(
        entry.filemode(),
        0o100755,
        "the install commit must record the shipped mode, not the buffer default"
    );
}

/// The size arm of the same rule, checked directly: building a real
/// multi-megabyte fixture would cost far more than it proves.
#[test]
fn oversized_content_is_not_mergeable() {
    assert!(super::is_mergeable_text(b"ordinary text"));
    assert!(!super::is_mergeable_text(b"has a \0 byte"));
    assert!(!super::is_mergeable_text(&vec![
        b'a';
        super::MAX_MERGE_BYTES + 1
    ]));
    assert!(super::is_mergeable_text(&vec![
        b'a';
        super::MAX_MERGE_BYTES
    ]));
}

/// After a merge the user still carries a patch, so the badge must still say
/// so. This is also what proves the merge base stayed pristine.
#[tokio::test]
async fn badge_still_modified_after_a_clean_merge() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    write_data(&ws, "knowhow/notes.md", &notes_with_user_section(NOTES));
    let base = baseline(&bus, &["knowhow/notes.md"]);

    update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
        &base,
        true,
    )
    .await;

    let status = super::modification_status_for(
        &ws,
        &recorded_install_commit(&bus),
        &["knowhow/notes.md".to_string()],
    );
    assert!(
        status.modified,
        "a kept patch is still a local modification"
    );
    assert_eq!(status.modified_paths, vec!["knowhow/notes.md".to_string()]);
}

/// The staging window is an hour long. An edit inside it means the panel
/// described content that is now gone. The confirm must refuse rather than
/// write a merge against the old bytes.
#[tokio::test]
async fn confirm_refuses_when_local_content_changed_since_staging() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    write_data(&ws, "knowhow/notes.md", &notes_with_user_section(NOTES));
    let base = baseline(&bus, &["knowhow/notes.md"]);

    let root = stage(
        &ws,
        "v2",
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
    );
    let (_, planned) = validate_tree(&root).unwrap();
    let plan = plan_for(&ws, &base, &planned);
    assert!(
        plan.detect_drift(&ws).is_none(),
        "clean right after staging"
    );

    // The user edits again while the panel sits open.
    write_data(&ws, "knowhow/notes.md", "something else entirely\n");
    assert_eq!(
        plan.detect_drift(&ws).as_deref(),
        Some("knowhow/notes.md"),
        "the confirm must be able to see the drift"
    );
}

/// The patch offered upstream is derived on read, so it is always against the
/// version the user is actually on. It must also really apply, which is the
/// only thing that makes it worth handing to a plugin author.
#[tokio::test]
async fn proposed_patch_applies_to_the_installed_version() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    let ours = notes_with_user_section(NOTES);
    write_data(&ws, "knowhow/notes.md", &ours);
    let base = baseline(&bus, &["knowhow/notes.md"]);

    let patch = super::local_patch(&ws, &base).expect("an edited plugin must yield a patch");
    assert!(
        patch.contains("data/knowhow/notes.md"),
        "paths must be repo-root relative so git apply lands them: {patch}"
    );

    // Apply it to a clean checkout of the INSTALLED version and check we get
    // the user's file back.
    let tmp = tempfile::tempdir().unwrap();
    let scratch = tmp.path().to_path_buf();
    std::fs::create_dir_all(scratch.join("data/knowhow")).unwrap();
    git2::Repository::init(&scratch).unwrap();
    std::fs::write(scratch.join("data/knowhow/notes.md"), NOTES).unwrap();
    let patch_file = scratch.join("local.patch");
    std::fs::write(&patch_file, &patch).unwrap();

    let out = std::process::Command::new("git")
        .args(["apply", "local.patch"])
        .current_dir(&scratch)
        .output()
        .expect("run git apply");
    assert!(
        out.status.success(),
        "git apply must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(scratch.join("data/knowhow/notes.md")).unwrap(),
        ours,
        "applying the patch must reproduce the user's file"
    );
}

/// An untouched plugin has nothing to propose, and a trigger projection is not
/// a local edit anybody wrote.
#[tokio::test]
async fn nothing_to_propose_without_a_real_local_edit() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    let files: &[(&str, &[u8])] = &[
        ("knowhow/notes.md", NOTES.as_bytes()),
        ("triggers/watch/trigger.toml", TRIGGER_TOML.as_bytes()),
    ];
    install_first(&ws, &bus, files).await;
    let base = baseline(&bus, &["knowhow/notes.md", "triggers/watch/trigger.toml"]);
    assert!(
        super::local_patch(&ws, &base).is_none(),
        "a clean install has no patch to offer"
    );

    // Only the engine-generated trigger definition differs.
    let mut edited = TriggerDefinition::from_toml(TRIGGER_TOML).unwrap();
    edited.slug = "watch".to_string();
    edited.run = TriggerRun::Intent {
        intent: "do something else".to_string(),
    };
    write_data(
        &ws,
        "triggers/watch/trigger.toml",
        &edited.to_toml().unwrap(),
    );
    assert!(
        super::local_patch(&ws, &base).is_none(),
        "a trigger projection diff describes the serializer, not the user"
    );
}

/// A path reaches `modified_paths` on a mode change alone, and identical
/// content has no diff. The proposal must then offer nothing, rather than a
/// patch header with no hunks under it.
#[cfg(unix)]
#[tokio::test]
async fn a_mode_only_change_proposes_no_patch() {
    use std::os::unix::fs::PermissionsExt;

    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("scripts/run.sh", NOTES.as_bytes())]).await;
    let base = baseline(&bus, &["scripts/run.sh"]);

    std::fs::set_permissions(
        data_path(&ws, "scripts/run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(
        super::modification_status_for(&ws, &base.commit, &base.files).modified,
        "test premise: a chmod alone already reads as modified"
    );

    assert!(
        super::local_patch(&ws, &base).is_none(),
        "unchanged content has nothing to propose"
    );
}

/// A user-added file inside a plugin's app dir that upstream now ships too.
/// There is no baseline blob, so the merge base is empty and the two sides
/// conflict. That routes to the same save-aside rule as any other conflict.
#[tokio::test]
async fn user_added_app_file_upstream_now_ships_is_a_conflict() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("apps/demo/index.html", b"<h1>v1</h1>\n")]).await;
    write_data(&ws, "apps/demo/extra.js", "console.log('mine')\n");
    let base = baseline(&bus, &["apps/demo/index.html"]);

    let outcome = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[
            ("apps/demo/index.html", b"<h1>v2</h1>\n"),
            ("apps/demo/extra.js", b"console.log('upstream')\n"),
        ],
        &base,
        true,
    )
    .await;

    assert_eq!(outcome.conflicted, vec!["apps/demo/extra.js".to_string()]);
    assert_eq!(
        read_data(&ws, "apps/demo/extra.js"),
        "console.log('upstream')\n"
    );
    assert_eq!(
        read_data(
            &ws,
            "artifacts/plugin-local-changes/demo/v2.0.0/apps/demo/extra.js"
        ),
        "console.log('mine')\n"
    );
}

const TOPIC: &str = "# Topic\nhow to do the thing\n";

/// The reported case: v2 moved a knowhow file out of its app folder. Both
/// copies used to survive, and the stale one kept loading. Only the new path
/// may remain, on disk and in the install commit.
#[tokio::test]
async fn a_moved_file_ends_up_only_at_its_new_path() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(
        &ws,
        &bus,
        &[
            ("apps/demo/index.html", b"<h1>v1</h1>\n"),
            ("apps/demo/knowhow/topic.md", TOPIC.as_bytes()),
        ],
    )
    .await;
    let base = baseline(
        &bus,
        &["apps/demo/index.html", "apps/demo/knowhow/topic.md"],
    );

    let outcome = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[
            ("apps/demo/index.html", b"<h1>v2</h1>\n"),
            ("knowhow/topic.md", TOPIC.as_bytes()),
        ],
        &base,
        true,
    )
    .await;

    let old = "apps/demo/knowhow/topic.md".to_string();
    assert_eq!(outcome.removes, vec![old.clone()]);
    assert_eq!(outcome.removed_files, vec![old.clone()]);
    assert!(
        outcome.removed.is_empty() && outcome.saved_paths.is_empty(),
        "an unedited file needs no copy: {outcome:?}"
    );
    assert!(!data_path(&ws, &old).exists(), "the old copy must be gone");
    assert!(
        !data_path(&ws, "apps/demo/knowhow").exists(),
        "the folder it leaves empty is pruned"
    );
    assert_eq!(read_data(&ws, "knowhow/topic.md"), TOPIC);

    let commit = recorded_install_commit(&bus);
    assert!(
        !in_commit(&ws, &commit, &old),
        "the install commit holds only what v2 ships"
    );
    assert_eq!(committed_blob(&ws, &commit, "knowhow/topic.md"), TOPIC);
    let repo = git2::Repository::open(&ws).unwrap();
    let status = repo.status_file(Path::new(&format!("data/{old}"))).ok();
    assert!(
        status.is_none_or(|s| s.is_empty()),
        "the deletion is committed, not left in the working tree: {status:?}"
    );

    let recorded_removal = bus.emitted_events().iter().rev().find_map(|e| match e {
        BusEvent::System(SystemEvent::PluginInstalled { files_removed, .. }) => {
            Some(files_removed.clone())
        }
        _ => None,
    });
    assert_eq!(recorded_removal, Some(vec![old]), "the event records it");
}

/// Only recorded paths are candidates. A file the user added beside a dropped
/// one is theirs, so it stays, and so does its folder.
#[tokio::test]
async fn a_user_file_beside_a_dropped_one_survives() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(
        &ws,
        &bus,
        &[
            ("apps/demo/index.html", b"<h1>v1</h1>\n"),
            ("apps/demo/knowhow/topic.md", TOPIC.as_bytes()),
        ],
    )
    .await;
    write_data(&ws, "apps/demo/knowhow/mine.md", "my own notes\n");
    let base = baseline(
        &bus,
        &["apps/demo/index.html", "apps/demo/knowhow/topic.md"],
    );

    let outcome = update(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("apps/demo/index.html", b"<h1>v2</h1>\n")],
        &base,
        true,
    )
    .await;

    assert_eq!(outcome.removed_files, vec!["apps/demo/knowhow/topic.md"]);
    assert!(!data_path(&ws, "apps/demo/knowhow/topic.md").exists());
    assert_eq!(
        read_data(&ws, "apps/demo/knowhow/mine.md"),
        "my own notes\n"
    );
}

/// An edited file the new version drops is saved aside, whole and as a patch,
/// then removed. The keep control cannot keep it: a dropped file left in place
/// is exactly the stale copy this exists to prevent.
#[tokio::test]
async fn an_edited_dropped_file_is_saved_aside_then_removed() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(
        &ws,
        &bus,
        &[
            ("knowhow/notes.md", NOTES.as_bytes()),
            ("knowhow/old.md", NOTES.as_bytes()),
        ],
    )
    .await;
    let ours = notes_with_user_section(NOTES);
    write_data(&ws, "knowhow/old.md", &ours);
    let base = baseline(&bus, &["knowhow/notes.md", "knowhow/old.md"]);
    let v2: &[(&str, &[u8])] = &[("knowhow/notes.md", NOTES_V2.as_bytes())];

    let root = stage(&ws, "preview", MANIFEST_V2, v2);
    let (_, planned) = validate_tree(&root).unwrap();
    let staged = plan_for(&ws, &base, &planned);
    assert_eq!(
        staged.preview_json(),
        serde_json::json!([{ "path": "knowhow/old.md", "outcome": "removed" }]),
        "the panel names it before confirm"
    );
    assert!(
        staged
            .resolve(false)
            .iter()
            .all(|c| c.outcome == LocalChangeOutcome::Removed),
        "switching the keep control off does not change a removal"
    );

    let outcome = update(&ws, &bus, MANIFEST_V2, v2, &base, true).await;

    assert_eq!(outcome.removed, vec!["knowhow/old.md".to_string()]);
    assert_eq!(outcome.removed_files, vec!["knowhow/old.md".to_string()]);
    assert!(!data_path(&ws, "knowhow/old.md").exists());
    let saved = "artifacts/plugin-local-changes/demo/v2.0.0/knowhow/old.md";
    assert_eq!(read_data(&ws, saved), ours, "the whole file is kept");
    let patch = read_data(&ws, &format!("{saved}.patch"));
    assert!(
        patch.contains("## Outlook snapshot"),
        "and the edit as a patch: {patch}"
    );
}

/// A legacy record has no install commit, so no edit can be ruled out. Every
/// dropped file still on disk is saved aside whole, then removed.
#[tokio::test]
async fn a_legacy_record_saves_every_dropped_file_aside() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(
        &ws,
        &bus,
        &[
            ("knowhow/notes.md", NOTES.as_bytes()),
            ("knowhow/old.md", TOPIC.as_bytes()),
        ],
    )
    .await;
    let recorded = ["knowhow/notes.md".to_string(), "knowhow/old.md".to_string()];

    let outcome = update_over(
        &ws,
        &bus,
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
        &recorded,
        None,
        true,
    )
    .await;

    assert_eq!(outcome.removed, vec!["knowhow/old.md".to_string()]);
    assert!(!data_path(&ws, "knowhow/old.md").exists());
    let saved = "artifacts/plugin-local-changes/demo/v2.0.0/knowhow/old.md";
    assert_eq!(read_data(&ws, saved), TOPIC);
    assert!(
        !data_path(&ws, &format!("{saved}.patch")).exists(),
        "with no baseline there is nothing to diff against"
    );
    assert_eq!(
        read_data(&ws, "knowhow/notes.md"),
        NOTES_V2,
        "a shipped file is still plainly replaced"
    );
}

/// The paths an update must never delete, whatever the record says.
#[tokio::test]
async fn paths_an_update_must_not_delete_are_skipped() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(
        &ws,
        &bus,
        &[
            ("knowhow/notes.md", NOTES.as_bytes()),
            ("knowhow/shared.md", TOPIC.as_bytes()),
            ("knowhow/gone.md", TOPIC.as_bytes()),
            ("triggers/watch/trigger.toml", TRIGGER_TOML.as_bytes()),
        ],
    )
    .await;
    std::fs::remove_file(data_path(&ws, "knowhow/gone.md")).unwrap();
    // A tampered record: paths outside the content dirs, present on disk so
    // only the guard can be what skips them.
    write_data(&ws, "config/secrets.json", "{}");
    std::fs::write(ws.join("outside.md"), "x").unwrap();
    let recorded: Vec<String> = [
        "knowhow/notes.md",
        "knowhow/shared.md",
        "knowhow/gone.md",
        "triggers/watch/trigger.toml",
        "config/secrets.json",
        "../outside.md",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let base = PluginBaseline {
        commit: recorded_install_commit(&bus),
        files: recorded.clone(),
    };
    let others: BTreeSet<&str> = ["knowhow/shared.md"].into();

    let root = stage(
        &ws,
        "v2",
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
    );
    let (_, planned) = validate_tree(&root).unwrap();
    let plan = super::plan_update(&ws, &recorded, Some(&base), &planned, &others);

    assert!(
        plan.removes().is_empty(),
        "shared, missing, trigger and unsafe paths are all skipped: {:?}",
        plan.removes()
    );
    assert!(plan.preview_json().as_array().unwrap().is_empty());
}

/// A case-only rename names one file on a case-insensitive volume. Removing
/// the old spelling would delete the file the update just wrote.
#[tokio::test]
async fn a_case_only_rename_is_not_removed() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/Topic.md", TOPIC.as_bytes())]).await;
    let base = baseline(&bus, &["knowhow/Topic.md"]);

    let root = stage(
        &ws,
        "v2",
        MANIFEST_V2,
        &[("knowhow/topic.md", TOPIC.as_bytes())],
    );
    let (_, planned) = validate_tree(&root).unwrap();
    let plan = plan_for(&ws, &base, &planned);

    assert!(plan.removes().is_empty(), "{:?}", plan.removes());
}

/// Unedited means the install commit holds these exact bytes. A recorded file
/// the commit lacks, for one an ignore rule kept out, is saved aside first.
#[tokio::test]
async fn a_dropped_file_the_install_commit_lacks_is_saved_aside() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    write_data(&ws, "knowhow/uncommitted.md", "only on disk\n");
    // The Modified scan covers only the baseline's own files, so it cannot see
    // this path. The plan must not read that silence as "unedited".
    let base = baseline(&bus, &["knowhow/notes.md"]);
    let recorded = [
        "knowhow/notes.md".to_string(),
        "knowhow/uncommitted.md".to_string(),
    ];

    let root = stage(
        &ws,
        "v2",
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
    );
    let (_, planned) = validate_tree(&root).unwrap();
    let plan = super::plan_update(&ws, &recorded, Some(&base), &planned, &BTreeSet::new());

    assert_eq!(
        plan.preview_json(),
        serde_json::json!([{ "path": "knowhow/uncommitted.md", "outcome": "removed" }])
    );
}

/// Nothing is deleted on a judgment the user did not see. A dropped file that
/// changes inside the staging window refuses the confirm, edited or not.
#[tokio::test]
async fn a_dropped_file_changed_after_staging_refuses_the_confirm() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(
        &ws,
        &bus,
        &[
            ("knowhow/notes.md", NOTES.as_bytes()),
            ("knowhow/old.md", TOPIC.as_bytes()),
        ],
    )
    .await;
    let base = baseline(&bus, &["knowhow/notes.md", "knowhow/old.md"]);

    let root = stage(
        &ws,
        "v2",
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES.as_bytes())],
    );
    let (_, planned) = validate_tree(&root).unwrap();
    let plan = plan_for(&ws, &base, &planned);
    assert_eq!(plan.removes(), vec!["knowhow/old.md".to_string()]);
    assert!(plan.preview_json().as_array().unwrap().is_empty());
    assert!(
        plan.detect_drift(&ws).is_none(),
        "clean right after staging"
    );

    write_data(&ws, "knowhow/old.md", "a late edit\n");
    assert_eq!(
        plan.detect_drift(&ws).as_deref(),
        Some("knowhow/old.md"),
        "a late edit to a file about to be removed must stop the confirm"
    );
}
