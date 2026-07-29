//! On-disk `trigger.toml` projection — a **derived read-model** of every
//! trigger's definition at `data/triggers/<slug>/trigger.toml`.
//!
//! Events are the source of truth (the scheduler replays `Trigger{Created,
//! Updated,Deleted}`); this projection mirrors the current definition to disk so
//! it's inspectable (Plugins → Installed file links) and so a plugin can SHIP a
//! trigger by declaring one (parsed back via [`TriggerDefinition::from_toml`] —
//! used by plugin install). It is NOT version-controlled: the files are added to
//! the workspace repo's local `.git/info/exclude` and rebuilt from events on
//! boot (a hand-edit is overwritten). See ADR 0019.
//!
//! TOML field order is load-bearing: all scalar / scalar-array fields come
//! before the table (`run`) and array-of-tables (`on`) fields, because the
//! `toml` serializer rejects a scalar emitted after a table.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::DATA_DIR;
use crate::engine::command_guard::SideEffectCategory;
use crate::triggers::config::{EventSubscription, TriggerConfig, TriggerRun};

fn default_timezone() -> String {
    "UTC".to_string()
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The authored / projected definition of a trigger. Maps the durable subset of
/// [`TriggerConfig`] — excludes runtime state (`id`, `last_run`, `paused`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TriggerDefinition {
    pub name: String,
    /// Stable kebab-case slug (the `data/triggers/<slug>/` segment). Optional in
    /// authored files — derived from `name` at registration when omitted.
    #[serde(default)]
    pub slug: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schedule: Vec<String>,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub go_to_review: bool,
    /// Plugin provenance (ADR 0019) — set on a plugin-auto-registered trigger,
    /// omitted (None) for a user trigger. A SHIPPED plugin `trigger.toml` leaves
    /// this unset; install stamps it from the plugin id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub side_effect_grant: Vec<SideEffectCategory>,
    // Table / array-of-tables fields LAST (TOML ordering constraint):
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<EventSubscription>,
    pub run: TriggerRun,
}

impl TriggerDefinition {
    /// Project a live [`TriggerConfig`] to its on-disk definition.
    pub fn from_config(c: &TriggerConfig) -> Self {
        Self {
            name: c.name.clone(),
            slug: c.slug.clone(),
            schedule: c.schedule.clone(),
            timezone: c.timezone.clone(),
            app_id: c.app_id.clone(),
            group_id: c.group_id.clone(),
            go_to_review: c.go_to_review,
            plugin_id: c.plugin_id.clone(),
            side_effect_grant: c.side_effect_grant.clone(),
            on: c.on.clone(),
            run: c.run.clone(),
        }
    }

    /// Build a `TriggerCreated`/`TriggerUpdated` event payload from this
    /// definition, stamping `trigger_id` + plugin provenance. `schedule` is
    /// forced empty because plugin triggers are event-driven only. Used by
    /// plugin install to auto-register a shipped `trigger.toml`.
    pub fn to_trigger_payload(&self, trigger_id: &str, plugin_id: &str) -> serde_json::Value {
        serde_json::json!({
            "trigger_id": trigger_id,
            "name": self.name,
            "slug": self.slug,
            "schedule": serde_json::Value::Array(vec![]),
            "timezone": self.timezone,
            "run": self.run,
            "on": self.on,
            "app_id": self.app_id,
            "go_to_review": self.go_to_review,
            "group_id": self.group_id,
            "side_effect_grant": self.side_effect_grant,
            "plugin_id": plugin_id,
        })
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// Parse an authored / projected `trigger.toml`. Used by plugin install to
    /// turn a shipped declaration into a trigger registration.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

/// `data/`-relative path of a trigger's definition file (for serving / preview).
pub fn trigger_toml_data_relpath(slug: &str) -> String {
    format!("triggers/{slug}/trigger.toml")
}

/// Absolute path of a trigger's definition file under the workspace.
fn trigger_toml_abspath(workspace_path: &Path, slug: &str) -> PathBuf {
    workspace_path
        .join(DATA_DIR)
        .join("triggers")
        .join(slug)
        .join("trigger.toml")
}

/// Write (or overwrite) one trigger's `trigger.toml`. Best-effort: a failure is
/// logged, never propagated — the projection is derived, the scheduler must not
/// stall on a disk hiccup. A blank slug is skipped (no stable path).
pub fn write_trigger_definition(workspace_path: &Path, config: &TriggerConfig) {
    if config.slug.is_empty() {
        return;
    }
    let def = TriggerDefinition::from_config(config);
    let toml = match def.to_toml() {
        Ok(t) => t,
        Err(e) => {
            crate::log!(
                "[TriggerDefn] serialize trigger.toml for '{}' failed: {}",
                config.slug,
                e
            );
            return;
        }
    };
    let path = trigger_toml_abspath(workspace_path, &config.slug);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            crate::log!("[TriggerDefn] mkdir {:?} failed: {}", parent, e);
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, toml) {
        crate::log!("[TriggerDefn] write {:?} failed: {}", path, e);
    }
}

/// Remove one trigger's `trigger.toml` (and prune the `<slug>/` dir if it's now
/// empty — leaving any sibling `knowhow/` / `scripts/` intact). Best-effort.
pub fn remove_trigger_definition(workspace_path: &Path, slug: &str) {
    if slug.is_empty() {
        return;
    }
    let path = trigger_toml_abspath(workspace_path, slug);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            crate::log!("[TriggerDefn] remove {:?} failed: {}", path, e);
            return;
        }
    }
    if let Some(dir) = path.parent() {
        // remove_dir only succeeds when empty — exactly the prune we want.
        let _ = std::fs::remove_dir(dir);
    }
}

/// Rebuild the whole projection from the current trigger set: write every
/// trigger's `trigger.toml` and remove orphaned ones (a deleted trigger, or a
/// `<slug>` rename that left a stale file). Called once at boot after the
/// scheduler replays events. Best-effort throughout.
pub fn rebuild_trigger_definitions(workspace_path: &Path, configs: &[TriggerConfig]) {
    use std::collections::HashSet;
    let live_slugs: HashSet<&str> = configs
        .iter()
        .map(|c| c.slug.as_str())
        .filter(|s| !s.is_empty())
        .collect();

    for config in configs {
        write_trigger_definition(workspace_path, config);
    }

    // Prune orphan trigger.toml files (slug no longer live). Only the
    // trigger.toml is removed — sibling knowhow/scripts stay.
    let triggers_dir = workspace_path.join(DATA_DIR).join("triggers");
    let Ok(entries) = std::fs::read_dir(&triggers_dir) else {
        return; // no triggers dir yet → nothing to prune
    };
    for entry in entries.flatten() {
        let slug_dir = entry.path();
        if !slug_dir.is_dir() {
            continue;
        }
        let Some(slug) = slug_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if live_slugs.contains(slug) {
            continue;
        }
        if slug_dir.join("trigger.toml").exists() {
            remove_trigger_definition(workspace_path, slug);
        }
    }
}

/// Pattern (workspace-root-relative) that excludes every projected `trigger.toml`
/// from git — the projection is derived, not version-controlled.
const TRIGGER_TOML_EXCLUDE: &str = "data/triggers/*/trigger.toml";

/// Ensure the workspace repo ignores the projected `trigger.toml` files via the
/// repo-local `.git/info/exclude` (so the committed `.gitignore` is untouched).
/// Idempotent + best-effort: a non-standard `.git` (worktree file, missing info
/// dir) just degrades to the files showing as untracked — never an error.
pub fn ensure_trigger_toml_gitignored(workspace_path: &Path) {
    let exclude = workspace_path.join(".git").join("info").join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == TRIGGER_TOML_EXCLUDE) {
        return;
    }
    let Some(parent) = exclude.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str("# Lucidos: derived trigger definition projection (ADR 0019)\n");
    next.push_str(TRIGGER_TOML_EXCLUDE);
    next.push('\n');
    let _ = std::fs::write(&exclude, next);
}

#[cfg(test)]
#[path = "definition_tests.rs"]
mod tests;
