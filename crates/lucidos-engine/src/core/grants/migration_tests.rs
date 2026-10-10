//! The migration is the risky half of ADR 0095: it runs once, on installs
//! nobody here can inspect, and a mistake either strands a working setup or
//! reinstates a permission the user removed.

use super::*;
use crate::core::grants::{self, GrantFile};

/// A machine mid-upgrade: a user dir with global grants, a gateway registry,
/// and workspace directories on disk.
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

    /// A machine-global grant file holding `patterns`.
    fn with_global(&self, file: GrantFile, patterns: &[&str]) -> &Self {
        let mut body = file.header().to_string();
        for p in patterns {
            body.push_str(p);
            body.push('\n');
        }
        std::fs::write(self.user_dir.join(file.file_name()), body).unwrap();
        self
    }

    /// A workspace directory registered under an absolute path.
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

/// The upgrade half of the split. Every workspace on the machine at migration
/// time keeps reading exactly what the machine-global file held.
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

/// The seeded file says where the grants came from, so a later reader can tell
/// an inherited grant from one chosen here.
#[test]
fn a_seeded_file_names_its_origin_and_the_grants_survive_the_header() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);

    m.migrate(&dev);

    let dir = grants::grants_dir(&dev);
    let body = grants::read_raw(&dir, GrantFile::AgentCommands).unwrap();
    assert!(body.starts_with("# Inherited on "), "body was: {body}");
    assert!(body.contains(GrantFile::AgentCommands.file_name()));
    assert_eq!(
        grants::patterns(&dir, GrantFile::AgentCommands),
        vec!["Bash".to_string(), "Python".to_string()],
        "the origin header is a comment, so it is not read as a grant"
    );
}

/// The creation half of the split, and the whole point of the change. A
/// workspace made after the migration inherits nothing, least of all the bare
/// `Bash` and `Python` grants nobody chose for it.
#[test]
fn a_workspace_created_after_the_migration_starts_with_no_grants() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);
    m.migrate(&dev);

    // Created afterwards, registered afterwards, and its own engine boots.
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

/// Re-running must never bring back a grant the user dropped. The stamp is what
/// makes that true, rather than comparing contents.
#[test]
fn re_running_never_resurrects_a_deleted_grant_or_duplicates_a_line() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, GRANTED);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);
    m.migrate(&dev);

    // The user drops `Bash` in this workspace and keeps `Python`.
    let dir = grants::grants_dir(&dev);
    grants::write_raw(&dir, GrantFile::AgentCommands, "Python\n").unwrap();

    // A later boot, with the machine record removed so the seed is retried.
    std::fs::remove_file(m.record()).unwrap();
    m.migrate(&dev);

    assert_eq!(
        grants::patterns(&dir, GrantFile::AgentCommands),
        vec!["Python".to_string()],
        "a grant the user deleted must stay deleted"
    );
}

/// A fresh install has no global files at all. That is the ordinary case, not
/// an error, and every workspace simply starts empty.
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
            !grants::exists(&dir, file),
            "{} must not be created with nothing to put in it",
            file.file_name()
        );
    }
    assert!(
        m.record().exists(),
        "the seed ran, so it must not run again"
    );
}

/// The originals stay put. A later release deletes them, and until then they
/// are the only remaining copy if anything went wrong here.
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

/// Discovery is the registry, never a glob of a workspaces root. A packaged
/// install keeps its workspaces under the OS app-data dir, which is what the
/// relative entry stands for here.
#[test]
fn discovery_resolves_both_absolute_and_app_data_relative_registry_entries() {
    let m = Machine::new();
    m.with_global(GrantFile::McpTools, &["Mcp(slack:*)"]);
    let absolute = m.with_workspace("dev");
    let relative = m.gateway_data.join("workspaces/packaged");
    std::fs::create_dir_all(&relative).unwrap();
    m.write_registry(&[absolute.to_str().unwrap(), "workspaces/packaged"]);

    // Own workspace deliberately outside the registry, so only discovery can
    // reach these two.
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

/// An unregistered workspace still gets its grants, because the engine running
/// it is proof that it exists. The e2e suite runs exactly this shape, with no
/// gateway and no registry.
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

/// A registry that cannot be parsed is UNKNOWN, never "no workspaces". Writing
/// the record on a failed read would strand every workspace it names.
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

/// The record is the machine-wide gate. Present means the seed has run, so a
/// second call touches nothing even where a workspace was somehow missed.
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

/// The record names the date and the workspaces it seeded, so the reason a new
/// workspace starts empty is legible on disk.
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

/// A registered directory that no longer exists is skipped. Seeding it would
/// recreate a workspace the user deleted.
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

/// A global file holding only its instructional header carries no decision, so
/// nothing is inherited from it.
#[test]
fn a_global_file_with_no_grants_seeds_no_file() {
    let m = Machine::new();
    m.with_global(GrantFile::AgentCommands, &[]);
    let dev = m.with_workspace("dev");
    m.write_registry(&[dev.to_str().unwrap()]);

    m.migrate(&dev);

    assert!(!grants::exists(
        &grants::grants_dir(&dev),
        GrantFile::AgentCommands
    ));
}

/// One workspace named twice cannot be seeded twice, whichever way the registry
/// and the engine spell its path.
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
