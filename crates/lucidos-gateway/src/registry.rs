use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub use lucidos_gateway_constants::{slugify, SIGIL};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceOrigin {
    User,
    Demo,
    Eval,
    E2e,
    Agent,
    #[default]
    Unknown,
}

impl WorkspaceOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Demo => "demo",
            Self::Eval => "eval",
            Self::E2e => "e2e",
            Self::Agent => "agent",
            Self::Unknown => "unknown",
        }
    }

    pub fn is_machine_made(self) -> bool {
        match self {
            Self::Agent | Self::Demo | Self::Eval | Self::E2e => true,
            Self::User | Self::Unknown => false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CreatedBy {
    pub workspace_id: String,
    pub thread_id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceCleanup {
    #[serde(default)]
    pub origin: WorkspaceOrigin,
    #[serde(default)]
    pub ephemeral: bool,
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub owner_token_digest: Option<String>,
    #[serde(default)]
    pub last_opened_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<CreatedBy>,
}

#[derive(Deserialize, Default)]
pub struct CleanupRegistration {
    pub origin: Option<WorkspaceOrigin>,
    pub ephemeral: Option<bool>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl WorkspaceCleanup {
    pub(crate) fn backs_up_database_on_delete(&self) -> bool {
        !self.origin.is_machine_made()
    }

    pub fn permits_owner(&self, token: &str) -> bool {
        self.ephemeral
            && self.origin.is_machine_made()
            && !token.is_empty()
            && self
                .owner_token_digest
                .as_ref()
                .is_some_and(|digest| crate::auth::ct_eq(digest, &crate::auth::digest(token)))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub dir: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_url: Option<String>,
    #[serde(default)]
    pub autostart: bool,
    #[serde(flatten)]
    pub cleanup: WorkspaceCleanup,
}

impl Workspace {
    pub fn gateway_provisioned(id: String, name: String, port: u16) -> Self {
        Self {
            dir: format!("workspaces/{id}"),
            id,
            name,
            port,
            database_url: None,
            cleanup: WorkspaceCleanup {
                origin: WorkspaceOrigin::User,
                ..Default::default()
            },
            autostart: true,
        }
    }

    pub fn agent_made(self, autostart: bool, created_by: CreatedBy) -> Self {
        Self {
            autostart,
            cleanup: WorkspaceCleanup {
                origin: WorkspaceOrigin::Agent,
                ephemeral: true,
                created_by: Some(created_by),
                ..Default::default()
            },
            ..self
        }
    }

    pub fn resolve_dir(&self, app_data: &Path) -> PathBuf {
        let dir = PathBuf::from(&self.dir);
        if dir.is_absolute() {
            dir
        } else {
            app_data.join(dir)
        }
    }
}

pub const REGISTRY_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Registry {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            workspaces: Vec::new(),
        }
    }
}

impl Registry {
    pub fn load(path: &Path) -> Result<Self, BoxError> {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s)
                .map_err(|e| format!("parse {}: {e}", path.display()).into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("read {}: {e}", path.display()).into()),
        }
    }

    pub fn migrate_to_current(&mut self) -> Option<usize> {
        if self.version >= REGISTRY_VERSION {
            return None;
        }
        let mut changed = 0;
        for ws in &mut self.workspaces {
            if !ws.autostart {
                ws.autostart = true;
                changed += 1;
            }
        }
        self.version = REGISTRY_VERSION;
        Some(changed)
    }

    pub fn save(&self, path: &Path) -> Result<(), BoxError> {
        crate::atomic_file::write(path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    pub fn get(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.workspaces.iter().any(|w| w.id == id)
    }

    pub fn find_by_display_name(&self, name: &str, except_id: Option<&str>) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| {
            Some(w.id.as_str()) != except_id
                && lucidos_gateway_constants::display_names_match(&w.name, name)
        })
    }

    pub fn add(&mut self, ws: Workspace) -> Result<(), BoxError> {
        if self.contains(&ws.id) {
            return Err(format!("workspace id '{}' already registered", ws.id).into());
        }
        self.workspaces.push(ws);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<Workspace, BoxError> {
        let idx = self
            .workspaces
            .iter()
            .position(|w| w.id == id)
            .ok_or_else(|| format!("workspace id '{id}' not found"))?;
        Ok(self.workspaces.remove(idx))
    }

    pub fn allocate_port(&self) -> Result<u16, BoxError> {
        let taken: Vec<u16> = self.workspaces.iter().map(|w| w.port).collect();
        let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        for _ in 0..50 {
            let port = lucidos_loopback_listener::bind(&[loopback])?.port()?;
            if !taken.contains(&port) {
                return Ok(port);
            }
        }
        Err("could not allocate a free loopback port after 50 attempts".into())
    }
}

pub fn unique_slug(base: &str, existing: &dyn Fn(&str) -> bool) -> String {
    if !existing(base) {
        return base.to_string();
    }
    for n in 2.. {
        let candidate = format!("{base}-{n}");
        if !existing(&candidate) {
            return candidate;
        }
    }
    unreachable!("the loop returns once a free suffix is found")
}

pub fn parse_workspace_name_from_archive(filename: &str) -> Option<String> {
    Some(
        lucidos_gateway_constants::backup_archive::parse(filename)?
            .workspace
            .to_string(),
    )
}

pub fn is_valid_id(id: &str) -> bool {
    lucidos_gateway_constants::is_valid_workspace_id(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_legacy_entries_are_protected_and_owner_tokens_are_revocable() {
        let legacy: Workspace = serde_json::from_value(serde_json::json!({
            "id": "legacy", "name": "Legacy", "dir": "workspaces/legacy", "port": 1234
        }))
        .unwrap();
        assert_eq!(legacy.cleanup.origin, WorkspaceOrigin::Unknown);
        assert!(!legacy.cleanup.permits_owner("owner"));
        let mut cleanup = WorkspaceCleanup {
            origin: WorkspaceOrigin::Eval,
            ephemeral: true,
            owner_token_digest: Some(crate::auth::digest("owner")),
            ..Default::default()
        };
        assert!(cleanup.permits_owner("owner"));
        assert!(!cleanup.permits_owner("other"));
        assert!(!cleanup.permits_owner(""));
        cleanup.origin = WorkspaceOrigin::User;
        assert!(!cleanup.permits_owner("owner"));
        cleanup.origin = WorkspaceOrigin::Unknown;
        assert!(!cleanup.permits_owner("owner"));
        cleanup.origin = WorkspaceOrigin::Eval;
        cleanup.ephemeral = false;
        assert!(!cleanup.permits_owner("owner"));
        let roundtrip: Workspace =
            serde_json::from_str(&serde_json::to_string(&legacy).unwrap()).unwrap();
        assert_eq!(legacy, roundtrip);
    }

    #[test]
    fn a_legacy_registry_loads_unknown_and_saves_back_without_created_by() {
        let legacy = r#"{"version":1,"workspaces":[
            {"id":"old","name":"Old","dir":"workspaces/old","port":5001,"autostart":true,
             "origin":"demo","ephemeral":true},
            {"id":"bare","name":"Bare","dir":"workspaces/bare","port":5002}
        ]}"#;
        let reg: Registry = serde_json::from_str(legacy).unwrap();
        let bare = reg.get("bare").unwrap();
        assert_eq!(bare.cleanup.origin, WorkspaceOrigin::Unknown);
        assert!(bare.cleanup.created_by.is_none());
        assert_eq!(
            reg.get("old").unwrap().cleanup.origin,
            WorkspaceOrigin::Demo
        );

        let saved = serde_json::to_value(&reg).unwrap();
        for ws in saved["workspaces"].as_array().unwrap() {
            assert!(ws.get("created_by").is_none(), "{ws}");
        }
        let reloaded: Registry = serde_json::from_value(saved).unwrap();
        assert_eq!(reloaded.workspaces, reg.workspaces);
    }

    #[test]
    fn an_agent_made_entry_round_trips_its_creator() {
        let ws = Workspace::gateway_provisioned("fixture".into(), "Fixture".into(), 5003)
            .agent_made(
                false,
                CreatedBy {
                    workspace_id: "dev".into(),
                    thread_id: "thread-1".into(),
                },
            );
        let json = serde_json::to_value(&ws).unwrap();
        assert_eq!(json["origin"], "agent");
        assert_eq!(json["ephemeral"], true);
        assert_eq!(json["autostart"], false);
        assert_eq!(json["created_by"]["thread_id"], "thread-1");
        assert_eq!(serde_json::from_value::<Workspace>(json).unwrap(), ws);
    }

    #[test]
    fn only_agent_demo_eval_and_e2e_are_machine_made() {
        use WorkspaceOrigin::*;
        for origin in [Agent, Demo, Eval, E2e] {
            assert!(origin.is_machine_made(), "{origin:?}");
        }
        for origin in [User, Unknown] {
            assert!(!origin.is_machine_made(), "{origin:?}");
        }
    }

    #[test]
    fn slugify_handles_spaces_emoji_and_punctuation() {
        assert_eq!(slugify("Default"), "default");
        assert_eq!(slugify("Work 💼"), "work");
        assert_eq!(slugify("My Cool Workspace!!"), "my-cool-workspace");
        assert_eq!(slugify("  leading/trailing  "), "leading-trailing");
        assert_eq!(slugify("a___b---c"), "a-b-c");
    }

    #[test]
    fn slugify_empty_or_all_punctuation_falls_back() {
        assert_eq!(slugify(""), "workspace");
        assert_eq!(slugify("💼💼💼"), "workspace");
        assert_eq!(slugify("---"), "workspace");
    }

    #[test]
    fn slugify_never_starts_with_sigil() {
        assert_eq!(slugify("~~~"), "workspace");
        assert_eq!(slugify("~admin"), "admin");
        assert!(!slugify("~weird~name").starts_with(SIGIL));
    }

    #[test]
    fn parse_workspace_name_from_archive_roundtrips_and_rejects_non_archives() {
        assert_eq!(
            parse_workspace_name_from_archive(
                "lucidos-backup-e2e-test-20260601-040254-o012345abcdef.age",
            )
            .as_deref(),
            Some("e2e-test")
        );
        assert!(parse_workspace_name_from_archive(
            "lucidos-backup-e2e-test-20260601-040254-o012345ABCDEF.age",
        )
        .is_none());

        assert_eq!(
            parse_workspace_name_from_archive("lucidos-backup-myws-20260601-040254.enc").as_deref(),
            Some("myws")
        );
        assert_eq!(
            parse_workspace_name_from_archive("lucidos-backup-e2e-test-20260601-040254.enc")
                .as_deref(),
            Some("e2e-test")
        );
        assert_eq!(
            parse_workspace_name_from_archive("lucidos-backup-myws-20260601-040254.age").as_deref(),
            Some("myws")
        );
        assert!(parse_workspace_name_from_archive("random.enc").is_none());
        assert!(parse_workspace_name_from_archive("lucidos-backup-myws.enc").is_none());
        assert!(parse_workspace_name_from_archive("lucidos-backup-myws-20260601.enc").is_none());
        assert!(parse_workspace_name_from_archive("lucidos-backup-20260601-040254.enc").is_none());
        assert!(parse_workspace_name_from_archive("backup.tar.gz").is_none());
    }

    #[test]
    fn unique_slug_appends_suffix_on_collision() {
        let taken = ["work", "work-2"];
        let exists = |s: &str| taken.contains(&s);
        assert_eq!(unique_slug("fresh", &exists), "fresh");
        assert_eq!(unique_slug("work", &exists), "work-3");
    }

    #[test]
    fn is_valid_id_accepts_slugs_rejects_traversal_and_sigil() {
        assert!(is_valid_id("default"));
        assert!(is_valid_id("work-2"));
        assert!(is_valid_id("a1b2"));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("Work"));
        assert!(!is_valid_id("../etc"));
        assert!(!is_valid_id("a/b"));
        assert!(!is_valid_id("-lead"));
        assert!(!is_valid_id("trail-"));
        assert!(!is_valid_id("under_score"));
        assert!(!is_valid_id("~admin"));
        assert!(!is_valid_id("dev@cloud"));
        assert!(!is_valid_id("@cloud"));
    }

    #[test]
    fn resolve_dir_absolute_vs_relative() {
        let app_data = Path::new("/app-data");
        let abs = Workspace {
            id: "dev".into(),
            name: "Dev".into(),
            dir: "/Users/me/workspaces/dev".into(),
            port: 5000,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        };
        assert_eq!(
            abs.resolve_dir(app_data),
            PathBuf::from("/Users/me/workspaces/dev")
        );
        let rel = Workspace {
            id: "default".into(),
            name: "Default".into(),
            dir: "workspaces/default".into(),
            port: 5001,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        };
        assert_eq!(
            rel.resolve_dir(app_data),
            PathBuf::from("/app-data/workspaces/default")
        );
    }

    #[test]
    fn load_missing_is_empty_present_garbage_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.json");

        let reg = Registry::load(&path).unwrap();
        assert!(reg.workspaces.is_empty());

        std::fs::write(&path, "   \n").unwrap();
        let err = Registry::load(&path).unwrap_err().to_string();
        assert!(err.contains(&path.display().to_string()), "{err}");

        std::fs::write(&path, "{ not json").unwrap();
        assert!(Registry::load(&path).is_err());
    }

    #[test]
    fn a_save_never_shares_its_temp_file_with_another_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.json");
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();

        Registry::default().save(&path).unwrap();

        assert!(Registry::load(&path).unwrap().workspaces.is_empty());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n != "workspaces.json" && n != "workspaces.json.tmp")
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/workspaces.json");

        let mut reg = Registry::default();
        reg.add(Workspace {
            id: "default".into(),
            name: "Default".into(),
            dir: "workspaces/default".into(),
            port: 51811,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        })
        .unwrap();
        reg.add(Workspace {
            id: "myws".into(),
            name: "My Workspace 💼".into(),
            dir: "workspaces/myws".into(),
            port: 51812,
            database_url: Some("postgres://lucidos:lucidos@127.0.0.1:5599/lucidos".into()),
            cleanup: Default::default(),
            autostart: true,
        })
        .unwrap();
        reg.save(&path).unwrap();

        let loaded = Registry::load(&path).unwrap();
        assert_eq!(loaded.workspaces, reg.workspaces);
        assert_eq!(loaded.get("myws").unwrap().name, "My Workspace 💼");
        assert!(loaded.get("default").unwrap().database_url.is_none());
        assert!(loaded.get("myws").unwrap().autostart);
        assert!(!loaded.get("default").unwrap().autostart);
    }

    #[test]
    fn add_rejects_duplicate_remove_returns_entry() {
        let mut reg = Registry::default();
        let ws = Workspace {
            id: "x".into(),
            name: "X".into(),
            dir: "x".into(),
            port: 1,
            database_url: None,
            cleanup: Default::default(),
            autostart: false,
        };
        reg.add(ws.clone()).unwrap();
        assert!(reg.add(ws.clone()).is_err());
        assert_eq!(reg.remove("x").unwrap(), ws);
        assert!(reg.remove("x").is_err());
    }

    #[test]
    fn legacy_entry_without_autostart_defaults_false() {
        let json = r#"{"workspaces":[{"id":"dev","name":"Dev","dir":"/ws/dev","port":5173}]}"#;
        let reg: Registry = serde_json::from_str(json).unwrap();
        assert_eq!(reg.workspaces.len(), 1);
        assert!(!reg.get("dev").unwrap().autostart);
        assert!(reg.get("dev").unwrap().database_url.is_none());
    }

    #[test]
    fn a_gateway_provisioned_workspace_runs_in_the_background() {
        let ws = Workspace::gateway_provisioned("myws".into(), "My Workspace 💼".into(), 5001);
        assert!(
            ws.autostart,
            "a workspace the user created or restored is one they want running, \
             so it auto-starts and keeps its triggers, scheduled tasks and push alive",
        );
        assert_eq!(ws.id, "myws");
        assert_eq!(ws.name, "My Workspace 💼");
        assert_eq!(ws.port, 5001);
        assert_eq!(ws.dir, "workspaces/myws");
        assert!(
            ws.database_url.is_none(),
            "the shared cluster's database is derived from the id (ADR 0014)",
        );
    }

    fn legacy_registry() -> Registry {
        serde_json::from_str(
            r#"{"workspaces":[
                {"id":"myws","name":"myws","dir":"workspaces/myws","port":5001,"autostart":false},
                {"id":"dev","name":"dev","dir":"workspaces/dev","port":5002,"autostart":true}
            ]}"#,
        )
        .expect("the legacy shape parses")
    }

    #[test]
    fn a_legacy_registry_reads_as_version_zero_and_migrates_to_the_new_default() {
        let mut reg = legacy_registry();
        assert_eq!(
            reg.version, 0,
            "a document with no version is pre-migration"
        );
        assert_eq!(reg.migrate_to_current(), Some(1), "one entry was flipped");
        assert!(reg.get("myws").unwrap().autostart);
        assert!(reg.get("dev").unwrap().autostart, "already on, left alone");
        assert_eq!(reg.version, REGISTRY_VERSION);
    }

    #[test]
    fn a_migrated_registry_is_never_migrated_again() {
        let mut reg = legacy_registry();
        reg.migrate_to_current();
        reg.get_mut("myws").unwrap().autostart = false;
        assert_eq!(reg.migrate_to_current(), None, "already current");
        assert!(
            !reg.get("myws").unwrap().autostart,
            "a deliberate off must survive every later boot",
        );
    }

    #[test]
    fn a_registry_created_in_memory_is_already_current() {
        let mut reg = Registry::default();
        assert_eq!(reg.version, REGISTRY_VERSION);
        assert_eq!(reg.migrate_to_current(), None);
    }

    #[test]
    fn the_version_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.json");
        let mut reg = legacy_registry();
        reg.migrate_to_current();
        reg.save(&path).unwrap();

        let loaded = Registry::load(&path).unwrap();
        assert_eq!(loaded.version, REGISTRY_VERSION);
        assert!(loaded.get("myws").unwrap().autostart);
    }

    fn two_workspaces() -> Registry {
        let mut reg = Registry::default();
        for (id, name) in [("personal", "personaaa"), ("personaaa", "personaaa2")] {
            reg.add(Workspace {
                id: id.into(),
                name: name.into(),
                dir: format!("workspaces/{id}"),
                port: 5000,
                database_url: None,
                cleanup: Default::default(),
                autostart: false,
            })
            .unwrap();
        }
        reg
    }

    #[test]
    fn a_taken_display_name_is_found_whatever_its_case_or_padding() {
        let reg = two_workspaces();
        for probe in ["personaaa", "PersonAAA", "  personaaa  "] {
            assert_eq!(
                reg.find_by_display_name(probe, None).map(|w| w.id.as_str()),
                Some("personal"),
                "probe {probe:?}",
            );
        }
        assert!(reg.find_by_display_name("something else", None).is_none());
    }

    #[test]
    fn a_workspace_never_collides_with_itself_on_rename() {
        let reg = two_workspaces();
        assert!(reg
            .find_by_display_name("personaaa", Some("personal"))
            .is_none());
        assert!(reg
            .find_by_display_name("PERSONAAA", Some("personal"))
            .is_none());
        assert_eq!(
            reg.find_by_display_name("personaaa2", Some("personal"))
                .map(|w| w.id.as_str()),
            Some("personaaa"),
        );
    }

    #[test]
    fn allocate_port_avoids_registry_collisions() {
        let reg = Registry::default();
        let p = reg.allocate_port().unwrap();
        assert!(p > 0);
    }

    #[test]
    fn an_allocated_port_is_free_on_both_halves_of_loopback() {
        let port = Registry::default().allocate_port().unwrap();
        lucidos_loopback_listener::bind(&[SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
            .expect("the engine can hold 127.0.0.1 and [::1] on the picked port");
    }
}
