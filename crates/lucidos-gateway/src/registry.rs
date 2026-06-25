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
    /// its auto-start workspaces). `false` (the default for a newly-created
    /// workspace; enabled only via the picker toggle) → the workspace is
    /// **listed** in the picker
    /// but its engine is started only on an explicit open/launch (lazy). An
    /// already-running engine is re-adopted on gateway restart regardless of this
    /// flag. Backward-compatible: a legacy entry with no field reads as `false`.
    #[serde(default)]
    pub autostart: bool,
}

impl Workspace {
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

/// The on-disk registry document.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Registry {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
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
            parse_workspace_name_from_archive("lucidos-backup-personal-20260601-040254.enc")
                .as_deref(),
            Some("personal")
        );
        // Hyphenated name — only the timestamp tail is stripped.
        assert_eq!(
            parse_workspace_name_from_archive("lucidos-backup-e2e-test-20260601-040254.enc")
                .as_deref(),
            Some("e2e-test")
        );
        // Not archive-shaped → None (picker then requires a typed name).
        assert!(parse_workspace_name_from_archive("random.enc").is_none());
        assert!(parse_workspace_name_from_archive("lucidos-backup-personal.enc").is_none());
        assert!(
            parse_workspace_name_from_archive("lucidos-backup-personal-20260601.enc").is_none()
        );
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
            id: "work".into(),
            name: "Work 💼".into(),
            dir: "workspaces/work".into(),
            port: 51812,
            database_url: Some("postgres://lucidos:lucidos@127.0.0.1:5599/lucidos".into()),
            autostart: true,
        })
        .unwrap();
        reg.save(&path).unwrap();

        let loaded = Registry::load(&path).unwrap();
        assert_eq!(loaded.workspaces, reg.workspaces);
        assert_eq!(loaded.get("work").unwrap().name, "Work 💼");
        assert!(loaded.get("default").unwrap().database_url.is_none());
        // The autostart flag round-trips per entry.
        assert!(loaded.get("work").unwrap().autostart);
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

    #[test]
    fn allocate_port_avoids_registry_collisions() {
        let reg = Registry::default();
        let p = reg.allocate_port().unwrap();
        assert!(p > 0);
    }
}
