use super::*;

#[test]
fn write_raw_uses_a_process_unique_temp_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = grants_dir(tmp.path());
    let file = GrantFile::ALL[0];
    let path = file.path_in(&dir);

    let scratch = scratch_path(&path);
    assert_ne!(
        scratch,
        path.with_extension("tmp"),
        "a fixed `.tmp` name is shared by every process on this workspace"
    );
    assert!(
        scratch
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == format!("tmp{}", std::process::id())),
        "the scratch name carries this pid: {}",
        scratch.display()
    );

    write_raw(&dir, file, "Bash\n").unwrap();
    assert_eq!(patterns(&dir, file), vec!["Bash".to_string()]);
    assert!(!scratch.exists(), "the temp file is renamed away");
}

#[test]
fn a_grant_in_one_workspace_is_invisible_from_another() {
    let tmp = tempfile::tempdir().unwrap();
    let a = grants_dir(&tmp.path().join("workspace-a"));
    let b = grants_dir(&tmp.path().join("workspace-b"));

    for file in GrantFile::ALL {
        write_raw(&a, file, "Bash\n").unwrap();
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

#[test]
fn an_absent_grants_directory_reads_empty_rather_than_erroring() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = grants_dir(&tmp.path().join("never-created"));
    for file in GrantFile::ALL {
        assert!(patterns(&missing, file).is_empty());
    }
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

    let path = GrantFile::AgentCommands.path_in(&dir);
    assert_eq!(std::fs::read_to_string(path).unwrap(), body);
    assert_eq!(
        patterns(&dir, GrantFile::AgentCommands),
        vec!["Bash(git:*)".to_string(), "Python".to_string()],
        "blank lines and comments are not grants"
    );
}

#[test]
fn the_three_lanes_do_not_share_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = grants_dir(tmp.path());

    write_raw(&dir, GrantFile::AgentCommands, "Bash\n").unwrap();

    assert_eq!(patterns(&dir, GrantFile::AgentCommands), vec!["Bash"]);
    assert!(patterns(&dir, GrantFile::CodingAgentTools).is_empty());
    assert!(patterns(&dir, GrantFile::McpTools).is_empty());

    let names: Vec<&str> = GrantFile::ALL.iter().map(|f| f.file_name()).collect();
    let unique: std::collections::HashSet<&&str> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "every lane needs its own file");
}

#[test]
fn a_workspace_never_falls_back_to_the_machine_global_file() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = grants_dir(&tmp.path().join("ws"));
    let global = tmp.path().join("global");

    for file in GrantFile::ALL {
        write_raw(&global, file, "Bash\n").unwrap();
        assert!(
            patterns(&workspace, file).is_empty(),
            "{} must not reach outside its own workspace",
            file.file_name()
        );
    }
}

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

#[test]
fn an_editor_body_parses_back_to_its_lines() {
    for file in GrantFile::ALL {
        let lines = vec!["Bash".to_string(), "Mcp(slack:*)".to_string()];
        let body = file.editor_body(&lines);
        assert!(body.starts_with(file.header()), "{}", file.file_name());
        assert_eq!(parse_patterns(&body), lines, "{}", file.file_name());
    }
}

#[test]
fn each_lane_recognizes_only_the_shapes_its_matcher_reads() {
    let cases = [
        (GrantFile::AgentCommands, "Bash", true),
        (GrantFile::AgentCommands, "Python", true),
        (GrantFile::AgentCommands, "Bash(git:*)", true),
        (GrantFile::AgentCommands, "Bash(./x.sh:*)", false),
        (GrantFile::AgentCommands, "Bash(git status)", false),
        (GrantFile::AgentCommands, "Read", false),
        (GrantFile::McpTools, "Mcp(slack:*)", true),
        (GrantFile::McpTools, "Mcp(slack:channels_list)", true),
        (GrantFile::McpTools, "Mcp(slack)", false),
        (GrantFile::McpTools, "Bash", false),
        (GrantFile::CodingAgentTools, "Read", true),
        (GrantFile::CodingAgentTools, "Bash(git:*)", true),
        (GrantFile::CodingAgentTools, "Skill(superpowers:*)", true),
        (GrantFile::CodingAgentTools, "Edit(/tmp/x", false),
        (GrantFile::CodingAgentTools, "9lives", false),
    ];
    for (file, pattern, want) in cases {
        assert_eq!(
            file.recognizes(pattern),
            want,
            "{} on {pattern:?}",
            file.file_name()
        );
    }
}
