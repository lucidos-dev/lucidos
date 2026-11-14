use super::ledger::{claude_version, StartupCheck};
use super::text::load;
use super::*;
use std::collections::BTreeMap;

// ---- text: Claude Code's per-file processing ----

#[test]
fn a_block_comment_goes_whole_and_a_comment_with_text_after_it_keeps_the_text() {
    let loaded = load("# T\n\n<!-- gone -->\nKept.\n\n<!-- lead --> residue\nMore.\n");
    assert_eq!(loaded.content, "# T\n\nKept.\n\n residue\nMore.");
}

#[test]
fn a_multi_line_comment_goes_and_a_comment_in_a_fence_stays() {
    let loaded = load("<!-- one\ntwo -->\nA.\n\n```\n<!-- stays -->\n```\n");
    assert_eq!(loaded.content, "A.\n\n```\n<!-- stays -->\n```");
}

#[test]
fn an_inline_comment_inside_a_paragraph_stays() {
    assert_eq!(
        load("Some <!-- inline --> text.").content,
        "Some <!-- inline --> text."
    );
}

#[test]
fn only_the_ends_of_the_text_are_trimmed() {
    assert_eq!(load("\u{feff}A.   \n\nB.   \n\n\n").content, "A.   \n\nB.");
    assert_eq!(load("# T\r\n\r\nBody.\r\n").content, "# T\r\n\r\nBody.");
}

#[test]
fn a_file_with_only_a_comment_is_empty() {
    assert_eq!(load("<!-- nothing else -->\n").content, "");
}

#[test]
fn frontmatter_is_dropped_and_declares_its_paths() {
    let scoped = load("---\npaths: \"**/*.rs\"\n---\n# R\n");
    assert_eq!(scoped.content, "# R");
    assert!(scoped.path_scoped);
    assert!(load("---\npaths:\n  - \"a/**\"\n---\n# R\n").path_scoped);
    assert!(
        !load("---\npaths:\n---\n# R\n").path_scoped,
        "a bare key is null"
    );
    assert!(!load("---\nglobs: \"**/*.rs\"\n---\n# R\n").path_scoped);
    assert!(!load("# R\n").path_scoped);
}

#[test]
fn an_unclosed_frontmatter_block_is_not_frontmatter() {
    assert_eq!(
        load("---\nnot closed\n# R\n").content,
        "---\nnot closed\n# R"
    );
}

#[test]
fn an_import_follows_whitespace_and_ends_at_whitespace() {
    let loaded = load(
        "See @docs/a.md, and @b.md and @c.md#frag and @./d.md and mail me@e.md and @f\\ g.md.\n\n\
         - item @h.md\n",
    );
    assert_eq!(
        loaded.imports,
        ["docs/a.md,", "b.md", "c.md", "./d.md", "f g.md.", "h.md"]
    );
}

#[test]
fn an_import_in_code_is_not_an_import() {
    let loaded =
        load("`@inline.md` and\n\n```\n@fenced.md\n```\n\n    @indented.md\n\nOK @real.md\n");
    assert_eq!(loaded.imports, ["real.md"]);
}

// ---- the mirror, against a transcript recorded from the real Claude Code ----

#[derive(serde::Deserialize)]
struct Fixture {
    cwd: String,
    user: String,
    tree: BTreeMap<String, String>,
    symlinks: BTreeMap<String, String>,
    recorded: Vec<FixtureRecord>,
}

#[derive(serde::Deserialize)]
struct FixtureRecord {
    r#type: String,
    path: String,
    content: String,
}

/// The fixture written to disk: a tree under `root`, and a config directory
/// holding the user file.
struct Materialized {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    cwd: PathBuf,
    recorded: Vec<RecordedFile>,
}

fn materialize() -> Materialized {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/real-claude-code-startup.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    for (path, content) in &fixture.tree {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    for (path, target) in &fixture.symlinks {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, root.join(path)).unwrap();
    }
    let config = root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("CLAUDE.md"), &fixture.user).unwrap();
    let recorded = fixture
        .recorded
        .into_iter()
        .map(|r| RecordedFile {
            path: match r.path.strip_prefix("<config>/") {
                Some(under) => config.join(under),
                None => root.join(&r.path),
            }
            .to_string_lossy()
            .into_owned(),
            kind: r.r#type,
            content: r.content,
        })
        .collect();
    Materialized {
        cwd: root.join(&fixture.cwd),
        _dir: dir,
        root,
        config,
        recorded,
    }
}

impl Materialized {
    fn mirror(&self) -> StartupInstructions {
        StartupInstructions::mirror(&self.cwd, &self.config, Some(&self.root))
    }

    /// The recorded entries in the mirror's order. Claude Code lists a rules
    /// directory in the filesystem's order, which differs between filesystems.
    fn recorded_in_order_of(&self, mirror: &StartupInstructions) -> Vec<RecordedFile> {
        let mut recorded = self.recorded.clone();
        recorded.sort_by_key(|r| {
            mirror
                .files
                .iter()
                .position(|f| f.path.to_string_lossy() == r.path)
                .unwrap_or(usize::MAX)
        });
        recorded
    }
}

fn entries(files: &[RecordedFile]) -> Vec<(String, String, String)> {
    let mut entries: Vec<_> = files
        .iter()
        .map(|f| (f.path.clone(), f.kind.clone(), f.content.clone()))
        .collect();
    entries.sort();
    entries
}

#[test]
fn the_mirror_loads_the_files_the_real_claude_code_loaded() {
    let fixture = materialize();
    let mirror = fixture.mirror();
    let mine: Vec<_> = mirror
        .files
        .iter()
        .map(|f| RecordedFile {
            kind: f.kind.name().to_string(),
            path: f.path.to_string_lossy().into_owned(),
            content: f.content.clone(),
        })
        .collect();
    assert_eq!(entries(&mine), entries(&fixture.recorded));
}

#[test]
fn the_mirror_keeps_claude_codes_order_between_directories_and_types() {
    let fixture = materialize();
    let mirror = fixture.mirror();
    let outside_rules: Vec<_> = mirror
        .files
        .iter()
        .filter(|f| !f.path.starts_with(fixture.cwd.join(".claude/rules")))
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect();
    let recorded: Vec<_> = fixture
        .recorded
        .iter()
        .filter(|r| {
            !r.path
                .starts_with(&*fixture.cwd.join(".claude/rules").to_string_lossy())
        })
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(outside_rules, recorded);
}

#[test]
fn the_mirror_matches_the_record_entry_for_entry() {
    let fixture = materialize();
    let mirror = fixture.mirror();
    assert_eq!(
        mirror.first_difference(&fixture.recorded_in_order_of(&mirror)),
        None
    );
}

#[test]
fn a_difference_names_the_first_entry_and_quotes_no_file() {
    let fixture = materialize();
    let mirror = fixture.mirror();
    let mut recorded = fixture.recorded_in_order_of(&mirror);
    recorded[2].content.push_str("SECRET");
    let difference = mirror.first_difference(&recorded).unwrap();
    assert!(difference.starts_with("entry 2"), "{difference}");
    assert!(!difference.contains("SECRET"));
    recorded.pop();
    recorded[2].content = mirror.files[2].content.clone();
    assert!(mirror
        .first_difference(&recorded)
        .unwrap()
        .contains("expected 30 entries, Claude Code recorded 29"));
}

#[test]
fn a_rendered_block_names_no_absolute_path_and_two_worktrees_render_alike() {
    let (a, b) = (materialize(), materialize());
    assert_ne!(a.root, b.root);
    let (mirror_a, mirror_b) = (a.mirror(), b.mirror());
    let block = mirror_a.render();
    assert_eq!(block, mirror_b.render());
    assert_eq!(mirror_a.key("2.1.300"), mirror_b.key("2.1.300"));
    assert!(!block.contains(a.root.to_str().unwrap()), "{block}");
    assert!(block.starts_with("Codebase and user instructions are shown below."));
    assert!(block.contains(
        "Contents of CLAUDE.md (project instructions, checked into the codebase):\n\n# Repo"
    ));
    assert!(block.contains("Contents of ../../CLAUDE.md (project instructions"));
    assert!(block.contains(
        "Contents of <config dir>/CLAUDE.md (user's private global instructions for all projects)"
    ));
    assert!(block.contains("(user's private project instructions, not checked in):"));
}

#[test]
fn the_key_changes_with_the_version_and_with_any_file_text() {
    let fixture = materialize();
    let before = fixture.mirror().key("2.1.300");
    assert_ne!(before, fixture.mirror().key("2.1.301"));
    std::fs::write(fixture.cwd.join(".claude/rules/mr-new.md"), "# New\n").unwrap();
    std::fs::write(fixture.cwd.join(".claude/rules/a.md"), "# A changed\n").unwrap();
    assert_ne!(before, fixture.mirror().key("2.1.300"));
}

#[test]
fn every_mirrored_file_is_excluded_including_a_symlink_and_its_target() {
    let fixture = materialize();
    let mirror = fixture.mirror();
    let excluded = mirror.excluded_paths();
    for file in mirror.files {
        assert!(excluded.contains(&file.path.to_string_lossy().into_owned()));
    }
    let link = fixture.cwd.join(".claude/rules/linked.md");
    assert!(excluded.contains(&link.to_string_lossy().into_owned()));
    assert!(excluded.contains(
        &fixture
            .cwd
            .join("docs/linked-target.md")
            .to_string_lossy()
            .into_owned()
    ));
    let mut unique = excluded.clone();
    unique.dedup();
    assert_eq!(unique, excluded);
}

#[test]
fn a_rule_scoped_to_paths_and_a_nested_file_are_not_startup_files() {
    let fixture = materialize();
    let loaded: Vec<_> = fixture
        .mirror()
        .files
        .iter()
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect();
    for absent in [
        "scoped-string.md",
        "scoped-list.md",
        "sub/CLAUDE.md",
        "empty.md",
        "only-comment.md",
        "rules/notes.txt",
        "d5.md",
    ] {
        assert!(
            !loaded.iter().any(|p| p.ends_with(absent)),
            "{absent} loaded"
        );
    }
}

// ---- the decision and the ledger ----

struct Workspace(tempfile::TempDir);

impl Workspace {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
}

fn decide_for(
    workspace: &Workspace,
    fixture: &Materialized,
    version: Option<&str>,
) -> StartupDecision {
    decide(
        workspace.path(),
        &fixture.cwd,
        &fixture.config,
        Some(&fixture.root),
        version,
        None,
    )
}

fn expect_check(decision: StartupDecision) -> StartupCheck {
    match decision {
        StartupDecision::Keep(Some(check)) => check,
        StartupDecision::Keep(None) => panic!("expected a check, the key is not unknown"),
        StartupDecision::Move(_) => panic!("expected a check, the session moved"),
    }
}

fn write_transcript(fixture: &Materialized, session: &str, lines: &[String]) {
    let mirror = fixture.mirror();
    let dir = fixture.config.join("projects").join(
        mirror
            .cwd()
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>(),
    );
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{session}.jsonl")), lines.join("\n")).unwrap();
}

fn instructions_line(recorded: &[RecordedFile]) -> String {
    serde_json::json!({
        "type": "attachment",
        "attachment": {
            "type": "instructions",
            "files": recorded.iter().map(|f| serde_json::json!({
                "type": f.kind, "path": f.path, "content": f.content,
            })).collect::<Vec<_>>(),
        }
    })
    .to_string()
}

#[test]
fn an_unknown_key_keeps_claude_codes_loading_and_asks_for_a_check() {
    let (workspace, fixture) = (Workspace::new(), materialize());
    expect_check(decide_for(&workspace, &fixture, Some("2.1.300")));
}

#[test]
fn a_session_with_no_readable_version_keeps_claude_codes_loading_and_checks_nothing() {
    let (workspace, fixture) = (Workspace::new(), materialize());
    assert!(matches!(
        decide_for(&workspace, &fixture, None),
        StartupDecision::Keep(None)
    ));
}

#[test]
fn a_matching_transcript_verifies_the_key_and_the_next_session_moves() {
    let (workspace, fixture) = (Workspace::new(), materialize());
    let check = expect_check(decide_for(&workspace, &fixture, Some("2.1.300")));
    let recorded = fixture.recorded_in_order_of(&fixture.mirror());
    write_transcript(&fixture, "s1", &[instructions_line(&recorded)]);
    check.run("s1");
    let StartupDecision::Move(moved) = decide_for(&workspace, &fixture, Some("2.1.300")) else {
        panic!("a verified key moves");
    };
    assert_eq!(moved.block, fixture.mirror().render());
    assert_eq!(moved.excluded, fixture.mirror().excluded_paths());
}

#[test]
fn a_verified_key_does_not_carry_to_another_claude_code_version() {
    let (workspace, fixture) = (Workspace::new(), materialize());
    let check = expect_check(decide_for(&workspace, &fixture, Some("2.1.300")));
    write_transcript(
        &fixture,
        "s1",
        &[instructions_line(
            &fixture.recorded_in_order_of(&fixture.mirror()),
        )],
    );
    check.run("s1");
    expect_check(decide_for(&workspace, &fixture, Some("2.1.301")));
}

#[test]
fn a_verified_key_does_not_carry_to_changed_files() {
    let (workspace, fixture) = (Workspace::new(), materialize());
    let check = expect_check(decide_for(&workspace, &fixture, Some("2.1.300")));
    write_transcript(
        &fixture,
        "s1",
        &[instructions_line(
            &fixture.recorded_in_order_of(&fixture.mirror()),
        )],
    );
    check.run("s1");
    std::fs::write(fixture.cwd.join("CLAUDE.md"), "# Changed\n").unwrap();
    expect_check(decide_for(&workspace, &fixture, Some("2.1.300")));
}

#[test]
fn a_differing_transcript_fails_the_key_and_the_session_never_moves() {
    let (workspace, fixture) = (Workspace::new(), materialize());
    let check = expect_check(decide_for(&workspace, &fixture, Some("2.1.300")));
    let mut recorded = fixture.recorded_in_order_of(&fixture.mirror());
    recorded[1].content.push('!');
    write_transcript(&fixture, "s1", &[instructions_line(&recorded)]);
    check.run("s1");
    assert!(matches!(
        decide_for(&workspace, &fixture, Some("2.1.300")),
        StartupDecision::Keep(None)
    ));
}

#[test]
fn a_transcript_that_is_missing_or_malformed_leaves_the_key_unknown() {
    let (workspace, fixture) = (Workspace::new(), materialize());
    let check = expect_check(decide_for(&workspace, &fixture, Some("2.1.300")));
    check.run("absent");
    write_transcript(
        &fixture,
        "garbled",
        &["not json {\"instructions\"".to_string()],
    );
    check.run("garbled");
    write_transcript(&fixture, "empty", &[]);
    check.run("empty");
    expect_check(decide_for(&workspace, &fixture, Some("2.1.300")));
}

#[test]
fn a_resumed_transcript_is_checked_against_its_last_record() {
    let (workspace, fixture) = (Workspace::new(), materialize());
    let check = expect_check(decide_for(&workspace, &fixture, Some("2.1.300")));
    let good = fixture.recorded_in_order_of(&fixture.mirror());
    let mut stale = good.clone();
    stale[0].content.push('x');
    write_transcript(
        &fixture,
        "s1",
        &[
            instructions_line(&stale),
            "{}".to_string(),
            instructions_line(&good),
        ],
    );
    check.run("s1");
    assert!(matches!(
        decide_for(&workspace, &fixture, Some("2.1.300")),
        StartupDecision::Move(_)
    ));
}

#[test]
fn the_ledger_survives_garbage_and_forgets_nothing_it_was_told() {
    let workspace = Workspace::new();
    std::fs::create_dir_all(workspace.path().join(".lucidos")).unwrap();
    std::fs::write(
        workspace
            .path()
            .join(".lucidos/cc-startup-instructions-ledger.json"),
        "{ nope",
    )
    .unwrap();
    assert_eq!(
        StartupLedger::state_of(workspace.path(), "k"),
        KeyState::Unknown
    );
    StartupLedger::record(workspace.path(), "k", Verdict::Verified);
    StartupLedger::record(workspace.path(), "j", Verdict::Failed);
    assert_eq!(
        StartupLedger::state_of(workspace.path(), "k"),
        KeyState::Verified
    );
    assert_eq!(
        StartupLedger::state_of(workspace.path(), "j"),
        KeyState::Failed
    );
}

// ---- the Claude Code version ----

#[cfg(unix)]
fn fake_claude(dir: &Path, banner: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("claude");
    std::fs::write(&path, format!("#!/bin/sh\necho '{banner}'\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn the_version_is_what_the_binary_prints() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_claude(dir.path(), "9.9.9 (Claude Code)");
    assert_eq!(
        claude_version(bin.as_os_str()).as_deref(),
        Some("9.9.9 (Claude Code)")
    );
}

#[cfg(unix)]
#[test]
fn an_upgraded_binary_is_read_again() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_claude(dir.path(), "1.0.0 (Claude Code)");
    assert_eq!(
        claude_version(bin.as_os_str()).as_deref(),
        Some("1.0.0 (Claude Code)")
    );
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fake_claude(dir.path(), "2.0.0 (Claude Code)");
    assert_eq!(
        claude_version(bin.as_os_str()).as_deref(),
        Some("2.0.0 (Claude Code)")
    );
}

#[test]
fn a_missing_binary_has_no_version() {
    assert_eq!(
        claude_version(std::ffi::OsStr::new("/nonexistent/claude")),
        None
    );
}

#[test]
fn a_session_records_that_it_moved_and_forgets_nothing_it_was_told() {
    let workspace = Workspace::new();
    assert!(!StartupLedger::session_moved(workspace.path(), "s1"));
    StartupLedger::record_moved_session(workspace.path(), "s1");
    StartupLedger::record(workspace.path(), "k", Verdict::Verified);
    assert!(StartupLedger::session_moved(workspace.path(), "s1"));
    assert!(!StartupLedger::session_moved(workspace.path(), "s2"));
    assert_eq!(
        StartupLedger::state_of(workspace.path(), "k"),
        KeyState::Verified
    );
}

#[cfg(unix)]
#[test]
fn a_linked_config_directory_labels_the_user_file_under_the_config_directory() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("dotfiles");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("CLAUDE.md"), "# Mine\n").unwrap();
    let linked = root.path().join("claude-config");
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    let (shallow, deep) = (root.path().join("a"), root.path().join("b/c/d"));
    for cwd in [&shallow, &deep] {
        std::fs::create_dir_all(cwd).unwrap();
        std::fs::write(cwd.join("CLAUDE.md"), "# Repo\n").unwrap();
    }
    let render = |cwd: &Path| StartupInstructions::mirror(cwd, &linked, None).render();
    assert!(render(&shallow).contains("Contents of <config dir>/CLAUDE.md"));
    assert_eq!(render(&shallow), render(&deep));
}
