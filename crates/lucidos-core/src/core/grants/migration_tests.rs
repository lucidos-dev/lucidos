use super::*;
use crate::core::grants::{self, GrantFile};

struct Machine {
    _tmp: tempfile::TempDir,
    user_dir: PathBuf,
    gateway_data: PathBuf,
    root: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("user");
        let gateway_data = tmp.path().join("user/gateway");
        let root = tmp.path().join("workspaces");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        Self {
            _tmp: tmp,
            user_dir,
            gateway_data,
            root,
        }
    }

    fn with_global(&self, file: GrantFile, patterns: &[&str]) -> &Self {
        let mut body = file.header().to_string();
        for p in patterns {
            body.push_str(p);
            body.push('\n');
        }
        std::fs::write(self.user_dir.join(file.file_name()), body).unwrap();
        self
    }

    fn with_workspace(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_registry(&self, dirs: &[&str]) {
        let entries: Vec<String> = dirs
            .iter()
            .map(|d| format!(r#"{{"id":"x","name":"x","dir":{d:?},"port":1}}"#))
            .collect();
        self.write_registry_raw(&format!(r#"{{"workspaces":[{}]}}"#, entries.join(",")));
    }

    fn write_registry_raw(&self, text: &str) {
        let path = self.gateway_data.join("config/workspaces.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn migrate(&self, own_workspace: &Path) {
        seed_from(&self.user_dir, &self.gateway_data, own_workspace);
    }

    fn record(&self) -> PathBuf {
        self.user_dir.join(MACHINE_RECORD)
    }
}

const GRANTED: &[&str] = &["Bash", "Python"];

#[test]
fn a_workspace_present_at_migration_time_inherits_the_global_grants() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    m.with_global(GrantFile::McpTools, &["Mcp(slack:*)"]);
    let dev = m.with_workspace("dev");
    let other = m.with_workspace("other");
    m.write_registry(&[dev.to_str().unwrap(), other.to_str().unwrap()]);

    m.migrate(&dev);

    for ws in [&dev, &other] {
        let dir = grants::grants_dir(ws);
        assert_eq!(
            grants::patterns(&dir, GrantFile::AgentCommands),
            vec!["Bash".to_string(), "Python".to_string()],
            "{} must inherit the command grants verbatim",
            ws.display()
        );
        assert_eq!(
            grants::patterns(&dir, GrantFile::McpTools),
            vec!["Mcp(slack:*)".to_string()]
        );
    }
}

#[test]
fn a_seeded_file_names_its_origin_and_the_grants_survive_the_header() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);

    m.migrate(&dev);

    let dir = grants::grants_dir(&dev);
    let body = std::fs::read_to_string(dir.join(GrantFile::AgentCommands.file_name())).unwrap();
    assert!(body.starts_with("# Inherited on "), "body was: {body}");
    assert!(body.contains(GrantFile::AgentCommands.file_name()));
    assert_eq!(
        grants::patterns(&dir, GrantFile::AgentCommands),
        vec!["Bash".to_string(), "Python".to_string()],
        "the origin header is a comment, so it is not read as a grant"
    );
}

#[test]
fn a_workspace_created_after_the_migration_starts_with_no_grants() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);
    m.migrate(&dev);

    let fresh = m.with_workspace("fresh");
    m.write_registry(&[dev.to_str().unwrap(), fresh.to_str().unwrap()]);
    m.migrate(&fresh);

    let dir = grants::grants_dir(&fresh);
    for file in GrantFile::ALL {
        assert!(
            grants::patterns(&dir, file).is_empty(),
            "{} must grant nothing in a workspace created after the migration",
            file.file_name()
        );
    }
    assert!(
        !dir.join(WORKSPACE_STAMP).exists(),
        "the migration is over, so nothing should have visited this workspace"
    );
}

#[test]
fn re_running_never_resurrects_a_deleted_grant_or_duplicates_a_line() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);
    m.migrate(&dev);

    let dir = grants::grants_dir(&dev);
    grants::write_raw(&dir, GrantFile::AgentCommands, "Python\n").unwrap();

    std::fs::remove_file(m.record()).unwrap();
    m.migrate(&dev);

    assert_eq!(
        grants::patterns(&dir, GrantFile::AgentCommands),
        vec!["Python".to_string()],
        "a grant the user deleted must stay deleted"
    );
}

#[test]
fn a_fresh_install_with_no_global_files_seeds_nothing_and_still_closes_the_door() {
    let m = Machine::new();
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);

    m.migrate(&dev);

    let dir = grants::grants_dir(&dev);
    for file in GrantFile::ALL {
        assert!(grants::patterns(&dir, file).is_empty());
        assert!(
            !dir.join(file.file_name()).exists(),
            "{} must not be created with nothing to put in it",
            file.file_name()
        );
    }
    assert!(
        m.record().exists(),
        "the seed ran, so it must not run again"
    );
}

#[test]
fn the_global_files_are_copied_not_moved() {
    let m = Machine::new();
    for file in GrantFile::ALL {
        m.with_global(file, GRANTED);
    }
    let before: Vec<String> = GrantFile::ALL
        .iter()
        .map(|f| std::fs::read_to_string(m.user_dir.join(f.file_name())).unwrap())
        .collect();
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);

    m.migrate(&dev);

    for (file, expected) in GrantFile::ALL.iter().zip(before) {
        assert_eq!(
            std::fs::read_to_string(m.user_dir.join(file.file_name())).unwrap(),
            expected,
            "{} must be left byte for byte as it was",
            file.file_name()
        );
    }
}

#[test]
fn discovery_resolves_both_absolute_and_app_data_relative_registry_entries() {
    let m = Machine::new();
    m.with_global(GrantFile::McpTools, &["Mcp(slack:*)"]);
    let absolute = m.with_workspace("dev");
    let relative = m.gateway_data.join("workspaces/packaged");
    std::fs::create_dir_all(&relative).unwrap();
    m.write_registry(&[absolute.to_str().unwrap(), "workspaces/packaged"]);

    let own = m.with_workspace("own");
    m.migrate(&own);

    for ws in [&absolute, &relative] {
        assert_eq!(
            grants::patterns(&grants::grants_dir(ws), GrantFile::McpTools),
            vec!["Mcp(slack:*)".to_string()],
            "{} must be discovered through the registry",
            ws.display()
        );
    }
}

#[test]
fn the_engines_own_workspace_is_seeded_even_with_no_registry() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let own = m.with_workspace("own");

    m.migrate(&own);

    assert_eq!(
        grants::patterns(&grants::grants_dir(&own), GrantFile::AgentCommands),
        vec!["Bash".to_string(), "Python".to_string()]
    );
    assert!(m.record().exists(), "an absent registry is a normal case");
}

#[test]
fn an_unparseable_registry_seeds_nothing_and_leaves_the_door_open() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry_raw("{ not json");

    m.migrate(&dev);

    let dir = grants::grants_dir(&dev);
    assert!(!dir.join(WORKSPACE_STAMP).exists(), "nothing was seeded");
    assert!(grants::patterns(&dir, GrantFile::AgentCommands).is_empty());
    assert!(
        !m.record().exists(),
        "the next boot must try again, not treat this as done"
    );
}

#[test]
fn a_blank_registry_seeds_nothing_and_leaves_the_door_open() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry_raw("  \n");

    m.migrate(&dev);

    assert!(!grants::grants_dir(&dev).join(WORKSPACE_STAMP).exists());
    assert!(!m.record().exists(), "the next boot must try again");
}

#[test]
fn the_machine_record_stops_a_second_run() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);
    std::fs::write(m.record(), "# already done\n").unwrap();

    m.migrate(&dev);

    assert!(!grants::grants_dir(&dev).join(WORKSPACE_STAMP).exists());
    assert!(grants::patterns(&grants::grants_dir(&dev), GrantFile::AgentCommands).is_empty());
}

#[test]
fn the_machine_record_reads_as_an_explanation() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);

    m.migrate(&dev);

    let body = std::fs::read_to_string(m.record()).unwrap();
    assert!(body.contains("per workspace"), "body was: {body}");
    assert!(body.contains(&today()));
    assert!(body.contains(&dev.canonicalize().unwrap().display().to_string()));
}

#[test]
fn a_registered_workspace_that_is_gone_is_skipped_not_recreated() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    let deleted = m.root.join("deleted");
    m.write_registry(&[dev.to_str().unwrap(), deleted.to_str().unwrap()]);

    m.migrate(&dev);

    assert!(!deleted.exists(), "a deleted workspace stays deleted");
    assert!(m.record().exists());
}

#[test]
fn a_global_file_with_no_grants_seeds_no_file() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, &[]);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);

    m.migrate(&dev);

    assert!(!grants::grants_dir(&dev)
        .join(GrantFile::AgentCommands.file_name())
        .exists());
}

#[test]
fn a_workspace_named_by_both_the_registry_and_the_engine_is_seeded_once() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);

    m.migrate(&dev.join("."));

    let body = std::fs::read_to_string(m.record()).unwrap();
    assert_eq!(
        body.lines().filter(|l| !l.starts_with('#')).count(),
        1,
        "the record must list the workspace once, body was: {body}"
    );
}
