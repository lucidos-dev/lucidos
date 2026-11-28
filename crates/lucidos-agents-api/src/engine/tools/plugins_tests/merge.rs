use super::super::helpers::{build_archive, extract_to, fresh_workspace};
use super::super::{install_from_unpacked_with_bus, InstallContext, LocalChangeWrites, SourceType};
use super::*;

use crate::core::plugins::validate_tree;
use crate::engine::event_bus::{BusEvent, MockEventBus, SystemEvent};
use crate::engine::tools::plugins::registry::PluginBaseline;
use crate::triggers::config::TriggerRun;
use crate::triggers::definition::TriggerDefinition;
use std::path::PathBuf;

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

const NOTES_V2: &str = "# Notes\nintro (v2)\nalpha\nbravo\ncharlie\ndelta\necho\nfoxtrot\n";

const NOTES_V3: &str = "# Notes\nintro (v3)\nalpha\nbravo\ncharlie\ndelta\necho\nfoxtrot\n";

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

fn stage(ws: &Path, tag: &str, manifest: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = ws.join("staging").join(tag);
    std::fs::create_dir_all(&dir).unwrap();
    let archive = build_archive(&dir, "v.lucidos-plugin", manifest, files);
    extract_to(&dir, &archive)
}

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

#[derive(Debug)]
struct UpdateOutcome {
    merged: Vec<String>,
    conflicted: Vec<String>,
    replaced: Vec<String>,
    restored: Vec<String>,
    removed: Vec<String>,
    saved_paths: Vec<String>,
    removes: Vec<String>,
    removed_files: Vec<String>,
}

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

async fn update_over(
    ws: &Path,
    bus: &MockEventBus,
    manifest: &str,
    files: &[(&str, &[u8])],
    recorded: &[String],
    baseline: Option<&PluginBaseline>,
    keep_local_changes: bool,
) -> UpdateOutcome {
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

#[tokio::test]
async fn trigger_projection_is_never_merged() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    let files: &[(&str, &[u8])] = &[
        ("knowhow/notes.md", NOTES.as_bytes()),
        ("triggers/watch/trigger.toml", TRIGGER_TOML.as_bytes()),
    ];
    install_first(&ws, &bus, files).await;

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

#[tokio::test]
async fn a_file_already_equal_to_the_new_version_is_not_a_local_change() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    let files: &[(&str, &[u8])] = &[
        ("knowhow/notes.md", NOTES.as_bytes()),
        ("apps/demo/index.html", b"<h1>v1</h1>\n"),
    ];
    install_first(&ws, &bus, files).await;

    let published = notes_with_user_section(NOTES);
    write_data(&ws, "knowhow/notes.md", &published);
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

#[tokio::test]
async fn a_second_save_of_one_version_never_overwrites_the_first() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;

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

#[cfg(unix)]
#[tokio::test]
async fn a_merged_file_keeps_the_shipped_executable_bit() {
    use std::os::unix::fs::PermissionsExt;

    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("scripts/run.sh", NOTES.as_bytes())]).await;
    write_data(&ws, "scripts/run.sh", &notes_with_user_section(NOTES));
    let base = baseline(&bus, &["scripts/run.sh"]);

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

    write_data(&ws, "knowhow/notes.md", "something else entirely\n");
    assert_eq!(
        plan.detect_drift(&ws).as_deref(),
        Some("knowhow/notes.md"),
        "the confirm must be able to see the drift"
    );
}

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

#[tokio::test]
async fn a_dropped_file_the_install_commit_lacks_is_saved_aside() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    write_data(&ws, "knowhow/uncommitted.md", "only on disk\n");
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

#[tokio::test]
async fn two_different_oversized_files_are_not_treated_as_equal() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    let size = super::MAX_MERGE_BYTES + 1;
    let installed = vec![b'a'; size];
    install_first(&ws, &bus, &[("knowhow/big.md", installed.as_slice())]).await;
    std::fs::write(data_path(&ws, "knowhow/big.md"), vec![b'b'; size]).unwrap();
    let base = baseline(&bus, &["knowhow/big.md"]);

    let shipped = vec![b'c'; size];
    let root = stage(
        &ws,
        "v2",
        MANIFEST_V2,
        &[("knowhow/big.md", shipped.as_slice())],
    );
    let (_, planned) = validate_tree(&root).unwrap();
    let plan = plan_for(&ws, &base, &planned);

    assert_eq!(
        plan.preview_json(),
        serde_json::json!([{ "path": "knowhow/big.md", "outcome": "replaced" }]),
        "different oversized files must stay a decision, not read as equal"
    );
}

#[tokio::test]
async fn a_failed_write_deletes_nothing() {
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
    write_data(&ws, "apps/demo", "a file where a folder is needed");
    let root = stage(
        &ws,
        "v2",
        MANIFEST_V2,
        &[("apps/demo/index.html", b"<h1>v2</h1>\n")],
    );

    let err = install_from_unpacked_with_bus(
        &ws,
        &bus,
        &root,
        InstallContext {
            removals: vec!["knowhow/old.md".to_string()],
            ..InstallContext::plain(SourceType::Archive, true)
        },
    )
    .await
    .expect_err("the write cannot land");

    assert!(err.contains("extract failed"), "{err}");
    assert_eq!(read_data(&ws, "knowhow/old.md"), TOPIC);
}

#[tokio::test]
async fn confirm_never_removes_an_unsafe_or_trigger_path_even_if_listed() {
    let (_tmp, ws) = fresh_workspace();
    let bus = MockEventBus::new();
    install_first(&ws, &bus, &[("knowhow/notes.md", NOTES.as_bytes())]).await;
    write_data(&ws, "config/secrets.json", "{}");
    write_data(&ws, "triggers/watch/trigger.toml", TRIGGER_TOML);
    std::fs::write(ws.join("outside.md"), "x").unwrap();
    let root = stage(
        &ws,
        "v2",
        MANIFEST_V2,
        &[("knowhow/notes.md", NOTES_V2.as_bytes())],
    );

    let write = install_from_unpacked_with_bus(
        &ws,
        &bus,
        &root,
        InstallContext {
            removals: vec![
                "config/secrets.json".to_string(),
                "triggers/watch/trigger.toml".to_string(),
                "../outside.md".to_string(),
            ],
            ..InstallContext::plain(SourceType::Archive, true)
        },
    )
    .await
    .expect("the update itself succeeds");

    assert!(write.removed_files.is_empty(), "{:?}", write.removed_files);
    assert!(data_path(&ws, "config/secrets.json").exists());
    assert!(data_path(&ws, "triggers/watch/trigger.toml").exists());
    assert!(ws.join("outside.md").exists());
}
