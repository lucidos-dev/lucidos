//! Tests for the grant-file store. The migration has its own suite.

use super::*;

/// The whole point of per-workspace grants, in one test: a yes said in one
/// workspace binds there and nowhere else.
#[test]
fn a_grant_in_one_workspace_is_invisible_from_another() {
    let tmp = tempfile::tempdir().unwrap();
    let a = grants_dir(&tmp.path().join("workspace-a"));
    let b = grants_dir(&tmp.path().join("workspace-b"));

    for file in GrantFile::ALL {
        append(&a, file, "Bash").unwrap();
    }

    for file in GrantFile::ALL {
        assert_eq!(
            patterns(&a, file),
            vec!["Bash".to_string()],
            "{} must hold the grant in the workspace it was made in",
            file.file_name()
        );
        assert!(
            patterns(&b, file).is_empty(),
            "{} must not carry workspace A's grant into workspace B",
            file.file_name()
        );
    }
}

/// A restored workspace has no `.lucidos/` at all, and must read as "nothing
/// granted" rather than fail. Fail-closed and quiet: the user is prompted once.
#[test]
fn an_absent_grants_directory_reads_empty_rather_than_erroring() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = grants_dir(&tmp.path().join("never-created"));
    for file in GrantFile::ALL {
        assert!(patterns(&missing, file).is_empty());
        assert_eq!(read_raw(&missing, file).unwrap(), file.header());
    }
}

#[test]
fn append_is_idempotent_and_seeds_the_header() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = grants_dir(tmp.path());

    append(&dir, GrantFile::McpTools, "Mcp(slack:channels_list)").unwrap();
    append(&dir, GrantFile::McpTools, "Mcp(github:*)").unwrap();
    append(&dir, GrantFile::McpTools, "Mcp(slack:channels_list)").unwrap();

    let body = read_raw(&dir, GrantFile::McpTools).unwrap();
    assert!(
        body.starts_with(GrantFile::McpTools.header()),
        "a file created by append keeps the instructional header"
    );
    assert_eq!(
        patterns(&dir, GrantFile::McpTools),
        vec![
            "Mcp(slack:channels_list)".to_string(),
            "Mcp(github:*)".to_string()
        ],
        "a repeat grant must not be appended twice"
    );
}

/// A hand-edited file keeps its shape: append adds one line and rewrites
/// nothing the user typed, including their own comments.
#[test]
fn append_preserves_an_existing_file_verbatim() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = grants_dir(tmp.path());
    let file = GrantFile::CodingAgentTools;
    write_raw(&dir, file, "# my notes\nBash\nRead\n").unwrap();

    append(&dir, file, "Skill(code-review:*)").unwrap();

    assert_eq!(
        read_raw(&dir, file).unwrap(),
        "# my notes\nBash\nRead\nSkill(code-review:*)\n"
    );
}

/// An indented line is the same grant, so it must not be appended again. The
/// reader trims, and the duplicate check has to trim with it.
#[test]
fn append_recognises_an_indented_grant_as_already_present() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = grants_dir(tmp.path());
    let file = GrantFile::CodingAgentTools;
    let body = "# header\n  Skill(code-review:*)  \n";
    write_raw(&dir, file, body).unwrap();

    append(&dir, file, "Skill(code-review:*)").unwrap();

    assert_eq!(read_raw(&dir, file).unwrap(), body);
}

#[test]
fn write_then_read_round_trips_and_parses() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = grants_dir(tmp.path());
    let body = format!(
        "{}Bash(git:*)\n\n# a note\nPython\n",
        GrantFile::AgentCommands.header()
    );

    write_raw(&dir, GrantFile::AgentCommands, &body).unwrap();

    assert_eq!(read_raw(&dir, GrantFile::AgentCommands).unwrap(), body);
    assert_eq!(
        patterns(&dir, GrantFile::AgentCommands),
        vec!["Bash(git:*)".to_string(), "Python".to_string()],
        "blank lines and comments are not grants"
    );
}

/// Each lane owns its own file. A grant on one must never satisfy another,
/// because the three pattern languages overlap textually: bare `Bash` is legal
/// in both the command guard and the Claude Code allowlist.
#[test]
fn the_three_lanes_do_not_share_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = grants_dir(tmp.path());

    append(&dir, GrantFile::AgentCommands, "Bash").unwrap();

    assert_eq!(patterns(&dir, GrantFile::AgentCommands), vec!["Bash"]);
    assert!(patterns(&dir, GrantFile::CodingAgentTools).is_empty());
    assert!(patterns(&dir, GrantFile::McpTools).is_empty());

    let names: Vec<&str> = GrantFile::ALL.iter().map(|f| f.file_name()).collect();
    let unique: std::collections::HashSet<&&str> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "every lane needs its own file");
}

/// The feature ships dark. A default that quietly granted something would be a
/// permission nobody chose, in every workspace at once.
#[test]
fn no_lane_grants_anything_by_default() {
    for file in GrantFile::ALL {
        assert_eq!(
            file.compiled_defaults(),
            &[] as &[&str],
            "{} must grant nothing until the user says so",
            file.file_name()
        );
    }
}

/// There is exactly one lookup. A workspace with no grant file grants nothing,
/// whatever the machine-global file it replaced still holds: the migration
/// seeds, and after that nothing reads outside the workspace.
#[test]
fn a_workspace_never_falls_back_to_the_machine_global_file() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = grants_dir(&tmp.path().join("ws"));
    let global = tmp.path().join("global");

    for file in GrantFile::ALL {
        append(&global, file, "Bash").unwrap();
        assert!(
            patterns(&workspace, file).is_empty(),
            "{} must not reach outside its own workspace",
            file.file_name()
        );
    }
}

/// Grants live beside the other engine-owned runtime state, not in `data/`.
/// The file tools can reach `data/`, and a permission file the agent can
/// rewrite is not a permission file.
#[test]
fn grants_live_in_the_engine_owned_runtime_directory() {
    let dir = grants_dir(Path::new("/tmp/workspaces/myws"));
    assert_eq!(dir, Path::new("/tmp/workspaces/myws/.lucidos"));
    for file in GrantFile::ALL {
        let path = file.path_in(&dir);
        assert!(
            !path.starts_with("/tmp/workspaces/myws/data"),
            "{} must not land under data/",
            file.file_name()
        );
    }
}

/// The lane's wire name is its file name, everywhere.
///
/// A `PermissionGrantsChanged` event carries a `GrantFile`, and the audit
/// timeline is read next to the Settings editor, which is labelled by file
/// name. A second spelling would make the reader match them up by hand.
#[test]
fn the_wire_name_is_the_file_name() {
    for file in GrantFile::ALL {
        let wire = serde_json::to_value(file).unwrap();
        assert_eq!(
            wire,
            serde_json::Value::String(file.file_name().to_string()),
            "{file:?} must serialize as its file name"
        );
        let back: GrantFile = serde_json::from_value(wire).unwrap();
        assert_eq!(back, file, "{file:?} must round-trip");
    }
}
