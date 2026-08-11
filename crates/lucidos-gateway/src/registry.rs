//! The workspace registry — `<app-data>/config/workspaces.json`.
//!
//! Maps a stable, filesystem-safe `id` (the **slug**) to a workspace's display
//! `name`, its on-disk `dir`, and the loopback `port` its engine binds. The slug
//! is the routing key (`/<slug>/`, ADR 0014) and the directory name; it never
//! changes. The `name` is a free-text display label — rename edits only this
//! field, so a rename is a registry write with no directory move, DB reconnect,
//! or port change.
//!
//! A slug can never collide with the gateway's reserved sigil namespace (`/~/`):
//! slugs are `[a-z0-9-]`, so they cannot start with `~`. That single rule
//! replaces the reserved-word list 0013 would have needed (ADR 0014 §2).
//!
//! `database_url` is a backward-compatible migration source from the old
//! per-workspace-Postgres topology. Steady state is ADR 0014's shared cluster:
//! the gateway creates/uses `lucidos_<workspace-id>` and passes that single
//! database URL to the engine. When a legacy `database_url` is present and the
//! shared database is missing, the gateway imports it, then ignores the legacy
//! URL on future starts so the old cluster can be explicitly decommissioned.

use serde::{Deserialize, Serialize};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The reserved sigil that prefixes all gateway-owned paths (`/~/…`). A
/// workspace slug can never start with it.
pub const SIGIL: char = '~';

/// One registered workspace.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// Stable, filesystem-safe slug. The routing key (`/<slug>/`) and the
    /// directory name for gateway-provisioned workspaces. Never changes.
    pub id: String,
    /// Display label — any text (spaces, emoji). Rename edits only this.
    pub name: String,
    /// Workspace directory. Relative paths resolve against `<app-data>`
    /// (gateway-provisioned, packaged); absolute paths are used verbatim (dev
    /// workspaces under `~/workspaces/<name>`).
    pub dir: String,
    /// The workspace engine's loopback port. Assigned free at create time and
    /// persisted so it is stable across reboots.
    pub port: u16,
    /// Legacy migration source from the old per-workspace-Postgres topology.
    /// New/updated entries omit it; the shared database is derived from `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_url: Option<String>,
    /// Whether the gateway spawns this workspace's engine on boot.
    ///
    /// `true` → the engine is **auto-started** when the gateway (re)starts (the
    /// always-on posture: a packaged install's login-launched gateway brings up
    /// its auto-start workspaces, so triggers, scheduled tasks and push keep
    /// working without anyone opening a window). This is the default every
    /// gateway-provisioned workspace is born with, see
    /// [`Workspace::gateway_provisioned`]. `false` (what the dev launcher
    /// seeds, and what the picker toggle can set) → the workspace is **listed**
    /// in the picker but its engine is started only on an explicit open/launch
    /// (lazy).
    ///
    /// It governs the BOOT posture and nothing else. An already-running engine is
    /// re-adopted on gateway restart regardless, and a workspace the last
    /// teardown stopped is restored regardless (see `crate::next_boot`): a
    /// restart returns what it took whatever this says.
    ///
    /// Backward-compatible: a legacy entry with no field reads as `false`, and
    /// [`Registry::migrate_to_current`] then lifts it to the current default.
    #[serde(default)]
    pub autostart: bool,
}

impl Workspace {
    /// A workspace the gateway provisions for itself under
    /// `<app-data>/workspaces/<id>`: the entry shape both creation paths share,
    /// picker **create** and **restore from backup**.
    ///
    /// Both go through here so the two can never disagree on the [`autostart`]
    /// default again. They did until 2026-08-11: create said `true` and restore
    /// said `false`, so a restored backup sat dark after every login, running no
    /// triggers, scheduled tasks or push until somebody found the picker toggle.
    /// Restoring a backup is at least as strong a statement that you want the
    /// workspace running as creating one from scratch is, and the restored
    /// workspace is exactly the one whose triggers and scheduled tasks were
    /// already set up.
    ///
    /// Not for a workspace the gateway merely *registers*: the dev launcher
    /// seeds an absolute `dir` outside app-data and wants autostart off (see
    /// `scripts/lib/workspace.sh`), and it writes the registry itself rather
    /// than calling this.
    ///
    /// [`autostart`]: Workspace::autostart
    pub fn gateway_provisioned(id: String, name: String, port: u16) -> Self {
        Self {
            dir: format!("workspaces/{id}"),
            id,
            name,
            port,
            database_url: None,
            autostart: true,
        }
    }

    /// Resolve [`Workspace::dir`] to an absolute path: absolute dirs are used
    /// verbatim; relative dirs join `app_data`.
    pub fn resolve_dir(&self, app_data: &Path) -> PathBuf {
        let dir = PathBuf::from(&self.dir);
        if dir.is_absolute() {
            dir
        } else {
            app_data.join(dir)
        }
    }
}

/// Schema version of the registry document, bumped when a load-time migration
/// is added. `1` introduced the autostart default (see
/// [`Registry::migrate_to_current`]).
pub const REGISTRY_VERSION: u32 = 1;

/// The on-disk registry document.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Registry {
    /// What the document was written by, so a load-time migration runs exactly
    /// once. A file written before versioning has no field and reads as `0`; a
    /// registry created in memory starts at [`REGISTRY_VERSION`] (see the
    /// hand-written `Default`, which serde's field default deliberately does not
    /// share).
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
    /// Load the registry from `path`. A missing file is an empty registry (the
    /// first-run state — the gateway boots with no workspaces and the smart root
    /// serves the picker, where the user names their first one). A present but
    /// unparseable file is a hard error: silently discarding it would orphan
    /// every registered workspace's data behind a fresh empty registry.
    pub fn load(path: &Path) -> Result<Self, BoxError> {
        match std::fs::read_to_string(path) {
            Ok(s) if s.trim().is_empty() => Ok(Self::default()),
            Ok(s) => serde_json::from_str(&s)
                .map_err(|e| format!("parse {}: {e}", path.display()).into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("read {}: {e}", path.display()).into()),
        }
    }

    /// Bring a registry loaded from disk up to [`REGISTRY_VERSION`], returning
    /// how many entries changed, or `None` when it was already current (nothing
    /// to save).
    ///
    /// Version 1 lifts every entry to the autostart default. Nobody was ever
    /// asked: `autostart: false` in an existing registry is the OLD default
    /// rather than a decision, and leaving it means an always-on install stays
    /// dark after every login, with no triggers, scheduled tasks or push until
    /// somebody opens each workspace. A user who did deliberately turn it off
    /// pays one toggle in the picker, which is the smaller harm. Running once,
    /// stamped by the version, is what keeps a later deliberate `false` from
    /// being flipped back on every boot.
    ///
    /// The caller decides WHEN this is appropriate: the packaged gateway runs it
    /// at startup, while dev does not, because the dev launcher seeds
    /// `autostart: false` on purpose and would otherwise find every workspace it
    /// has ever launched spawning at once.
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

    /// Persist the registry to `path` atomically (write a sibling temp file,
    /// then rename) so a crash mid-write can't truncate the registry and strand
    /// every workspace.
    pub fn save(&self, path: &Path) -> Result<(), BoxError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json.as_bytes())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
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

    /// The workspace already carrying this display name, ignoring surrounding
    /// space and letter case, skipping `except_id` (so renaming a workspace to
    /// what it is already called is not a collision with itself).
    ///
    /// The display name is what a user picks a workspace BY, so two rows reading
    /// the same thing simply cannot be told apart in the picker: what
    /// distinguishes them is the address, and the address is the thing the user
    /// never sees. Create / rename / restore therefore refuse a duplicate rather
    /// than produce one. Case-insensitive because "Work" and "work" are no more
    /// distinguishable in a list than two identical strings, and they slug to the
    /// same address anyway.
    ///
    /// A WRITE-time rule only. Duplicates were legal until now, so a registry
    /// holding some keeps working: the picker shows those rows' addresses so they
    /// stay tellable apart until the user renames one. Nothing is renamed for
    /// them.
    pub fn find_by_display_name(&self, name: &str, except_id: Option<&str>) -> Option<&Workspace> {
        let wanted = name.trim().to_lowercase();
        self.workspaces
            .iter()
            .find(|w| Some(w.id.as_str()) != except_id && w.name.trim().to_lowercase() == wanted)
    }

    /// Add a workspace. Errors if the id already exists.
    pub fn add(&mut self, ws: Workspace) -> Result<(), BoxError> {
        if self.contains(&ws.id) {
            return Err(format!("workspace id '{}' already registered", ws.id).into());
        }
        self.workspaces.push(ws);
        Ok(())
    }

    /// Remove a workspace by id, returning it. Errors if absent.
    pub fn remove(&mut self, id: &str) -> Result<Workspace, BoxError> {
        let idx = self
            .workspaces
            .iter()
            .position(|w| w.id == id)
            .ok_or_else(|| format!("workspace id '{id}' not found"))?;
        Ok(self.workspaces.remove(idx))
    }

    /// Allocate a free loopback port not already claimed by a registry entry.
    /// Binds `:0` to get an OS-assigned free port (the desktop convention),
    /// then re-rolls if it collides with an existing registry port.
    pub fn allocate_port(&self) -> Result<u16, BoxError> {
        let taken: Vec<u16> = self.workspaces.iter().map(|w| w.port).collect();
        for _ in 0..50 {
            let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
            if !taken.contains(&port) {
                return Ok(port);
            }
        }
        Err("could not allocate a free loopback port after 50 attempts".into())
    }
}

/// Derive a stable, filesystem- and URL-safe slug from a display name:
/// lowercase, non-alphanumerics collapsed to single `-`, leading/trailing `-`
/// trimmed. Empty input (or all-punctuation) falls back to `workspace`. The
/// output is always `[a-z0-9-]`, so it can never start with the gateway sigil.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "workspace".to_string()
    } else {
        out
    }
}

/// Make `base` unique against `existing` by appending `-2`, `-3`, … until it
/// no longer collides.
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

/// True when `s` is a backup-filename timestamp `YYYYMMDD-HHMMSS` (8 digits, a
/// hyphen, 6 digits). Mirrors the engine's `is_backup_timestamp` (duplicated per
/// ADR 0014 §1 — the gateway must not link the engine crate).
fn is_backup_timestamp(s: &str) -> bool {
    match s.split_once('-') {
        Some((date, time)) => {
            date.len() == 8
                && time.len() == 6
                && date.bytes().all(|b| b.is_ascii_digit())
                && time.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

/// Parse the workspace name a backup archive was produced for, from its
/// filename. A backup is named `lucidos-backup-{name}-{YYYYMMDD-HHMMSS}.enc`; the
/// `{name}` may contain hyphens, so strip the fixed prefix/suffix then the
/// trailing `-YYYYMMDD-HHMMSS`. Returns `None` for anything not in that exact
/// shape (the picker then asks the user for a name). Mirrors the engine's
/// `core::backup::parse_workspace_name_from_archive` (duplicated, ADR 0014 §1) so
/// the picker can prefill a default restore name without an engine round-trip.
pub fn parse_workspace_name_from_archive(filename: &str) -> Option<String> {
    let rest = filename
        .strip_prefix("lucidos-backup-")?
        .strip_suffix(".enc")?;
    let (before_time, time) = rest.rsplit_once('-')?;
    let (name, date) = before_time.rsplit_once('-')?;
    if name.is_empty() || !is_backup_timestamp(&format!("{date}-{time}")) {
        return None;
    }
    Some(name.to_string())
}

/// True if `id` is a safe routing key + directory name: non-empty, only
/// `[a-z0-9-]` (so it can never start with the [`SIGIL`]), no path-traversal.
/// The slug pipeline already produces this; the check guards control-API inputs
/// that bypass slugify.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Even a name that is all sigils collapses to the fallback, never a
        // slug that could shadow the gateway's `/~/` namespace.
        assert_eq!(slugify("~~~"), "workspace");
        assert_eq!(slugify("~admin"), "admin");
        assert!(!slugify("~weird~name").starts_with(SIGIL));
    }

    #[test]
    fn parse_workspace_name_from_archive_roundtrips_and_rejects_non_archives() {
        assert_eq!(
            parse_workspace_name_from_archive("lucidos-backup-myws-20260601-040254.enc").as_deref(),
            Some("myws")
        );
        // Hyphenated name — only the timestamp tail is stripped.
        assert_eq!(
            parse_workspace_name_from_archive("lucidos-backup-e2e-test-20260601-040254.enc")
                .as_deref(),
            Some("e2e-test")
        );
        // Not archive-shaped → None (picker then requires a typed name).
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
        assert!(!is_valid_id("Work")); // uppercase
        assert!(!is_valid_id("../etc")); // traversal
        assert!(!is_valid_id("a/b")); // slash
        assert!(!is_valid_id("-lead"));
        assert!(!is_valid_id("trail-"));
        assert!(!is_valid_id("under_score"));
        assert!(!is_valid_id("~admin")); // sigil-prefixed
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
            autostart: false,
        };
        assert_eq!(
            rel.resolve_dir(app_data),
            PathBuf::from("/app-data/workspaces/default")
        );
    }

    #[test]
    fn load_missing_is_empty_present_garbage_errors() {
        let dir = std::env::temp_dir().join(format!("lucidos-reg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("workspaces.json");

        // Missing → empty.
        let reg = Registry::load(&path).unwrap();
        assert!(reg.workspaces.is_empty());

        // Empty file → empty.
        std::fs::write(&path, "   \n").unwrap();
        assert!(Registry::load(&path).unwrap().workspaces.is_empty());

        // Garbage → hard error (never silently drop registered workspaces).
        std::fs::write(&path, "{ not json").unwrap();
        assert!(Registry::load(&path).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("lucidos-reg-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config/workspaces.json");

        let mut reg = Registry::default();
        reg.add(Workspace {
            id: "default".into(),
            name: "Default".into(),
            dir: "workspaces/default".into(),
            port: 51811,
            database_url: None,
            autostart: false,
        })
        .unwrap();
        reg.add(Workspace {
            id: "myws".into(),
            name: "My Workspace 💼".into(),
            dir: "workspaces/myws".into(),
            port: 51812,
            database_url: Some("postgres://lucidos:lucidos@127.0.0.1:5599/lucidos".into()),
            autostart: true,
        })
        .unwrap();
        reg.save(&path).unwrap();

        let loaded = Registry::load(&path).unwrap();
        assert_eq!(loaded.workspaces, reg.workspaces);
        assert_eq!(loaded.get("myws").unwrap().name, "My Workspace 💼");
        assert!(loaded.get("default").unwrap().database_url.is_none());
        // The autostart flag round-trips per entry.
        assert!(loaded.get("myws").unwrap().autostart);
        assert!(!loaded.get("default").unwrap().autostart);

        let _ = std::fs::remove_dir_all(&dir);
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
            autostart: false,
        };
        reg.add(ws.clone()).unwrap();
        assert!(reg.add(ws.clone()).is_err());
        assert_eq!(reg.remove("x").unwrap(), ws);
        assert!(reg.remove("x").is_err());
    }

    #[test]
    fn legacy_entry_without_autostart_defaults_false() {
        // A registry written before the autostart field must still load, with
        // autostart reading as false (backward-compatible #[serde(default)]).
        let json = r#"{"workspaces":[{"id":"dev","name":"Dev","dir":"/ws/dev","port":5173}]}"#;
        let reg: Registry = serde_json::from_str(json).unwrap();
        assert_eq!(reg.workspaces.len(), 1);
        assert!(!reg.get("dev").unwrap().autostart);
        assert!(reg.get("dev").unwrap().database_url.is_none());
    }

    // ── The autostart default and its one-time migration ─────────────────────

    /// The default a freshly provisioned entry carries. Asserted on the shared
    /// constructor rather than per call site, because the constructor is what
    /// makes create and restore agree: a second creation path that hand-rolls
    /// the literal is the bug this replaced (see
    /// [`Workspace::gateway_provisioned`]).
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
        // App-data relative: the gateway owns the directory it provisions.
        assert_eq!(ws.dir, "workspaces/myws");
        assert!(
            ws.database_url.is_none(),
            "the shared cluster's database is derived from the id (ADR 0014)",
        );
    }

    /// A pre-versioning document: no `version`, entries written with the old
    /// `autostart: false` default.
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

    // The stamp is the whole point: without it, every boot would flip a toggle
    // the user had deliberately turned off, and the picker control would be a lie.
    #[test]
    fn a_migrated_registry_is_never_migrated_again() {
        let mut reg = legacy_registry();
        reg.migrate_to_current();
        reg.get_mut("myws").unwrap().autostart = false; // a deliberate choice
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
        let dir = std::env::temp_dir().join(format!("lucidos-reg-ver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("workspaces.json");
        let mut reg = legacy_registry();
        reg.migrate_to_current();
        reg.save(&path).unwrap();

        let loaded = Registry::load(&path).unwrap();
        assert_eq!(loaded.version, REGISTRY_VERSION);
        assert!(loaded.get("myws").unwrap().autostart);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Display names are unique ─────────────────────────────────────────────

    fn two_workspaces() -> Registry {
        let mut reg = Registry::default();
        for (id, name) in [("personal", "personaaa"), ("personaaa", "personaaa2")] {
            reg.add(Workspace {
                id: id.into(),
                name: name.into(),
                dir: format!("workspaces/{id}"),
                port: 5000,
                database_url: None,
                autostart: false,
            })
            .unwrap();
        }
        reg
    }

    #[test]
    fn a_taken_display_name_is_found_whatever_its_case_or_padding() {
        // The reported picker screenshot: two rows both reading "personaaa",
        // one at /personal/ and one at /personaaa/. Nothing may create that.
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
        // Re-saving the same name (or a case edit of it) must not be refused.
        let reg = two_workspaces();
        assert!(reg
            .find_by_display_name("personaaa", Some("personal"))
            .is_none());
        assert!(reg
            .find_by_display_name("PERSONAAA", Some("personal"))
            .is_none());
        // But taking the OTHER workspace's name still collides.
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
}
