use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::DATA_DIR;
use crate::engine::command_guard::SideEffectCategory;
use crate::engine::context_ceiling::ContextCeiling;
use crate::triggers::config::{TriggerConfig, TriggerRun};
use crate::triggers::EventSubscription;

pub fn default_timezone() -> String {
    chrono_tz::UTC.name().to_string()
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TriggerDefinition {
    pub name: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    /// INVARIANT: Projected so the file shows it. A shipped value is never registered:
    /// only the owner grants a trigger side-effects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub side_effect_grant: Vec<SideEffectCategory>,
    /// INVARIANT: Projected so the file shows it. A shipped value is never registered:
    /// only the owner grants a trigger coding agents.
    #[serde(default, skip_serializing_if = "is_false")]
    pub coding_agent_grant: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_ceiling: Option<ContextCeiling>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
    /// INVARIANT: The **trigger model** this trigger's intent fires on. Omitted (None) =
    /// the account `trigger_model` preference. Scalar, so it must stay above the
    /// table fields below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_key: Option<crate::triggers::ContinueKeySpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<crate::triggers::ActorField>,
    // INVARIANT: Table / array-of-tables fields LAST (TOML ordering constraint):
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<EventSubscription>,
    pub run: TriggerRun,
}

impl TriggerDefinition {
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
            coding_agent_grant: c.coding_agent_grant,
            context_ceiling: c.context_ceiling,
            mcp_servers: c.mcp_servers.clone(),
            model: c.model.clone(),
            reasoning_effort: c.reasoning_effort.clone(),
            provider: c.provider.clone(),
            continue_key: c.continue_key.clone(),
            actor: c.actor.clone(),
            on: c.on.clone(),
            run: c.run.clone(),
        }
    }

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
            "context_ceiling": self.context_ceiling,
            "mcp_servers": self.mcp_servers,
            "plugin_id": plugin_id,
            "model": self.model,
            "reasoning_effort": self.reasoning_effort,
            "provider": self.provider,
            "continue_key": self.continue_key,
            crate::triggers::actor::ACTOR_FIELD_KEY: self.actor,
        })
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

fn trigger_toml_abspath(workspace_path: &Path, slug: &str) -> PathBuf {
    workspace_path
        .join(DATA_DIR)
        .join("triggers")
        .join(slug)
        .join("trigger.toml")
}

pub fn write_trigger_definition(workspace_path: &Path, config: &TriggerConfig) {
    if config.slug.is_empty() {
        return;
    }
    let def = TriggerDefinition::from_config(config);
    let toml = match def.to_toml() {
        Ok(t) => t,
        Err(e) => {
            crate::log_legacy!(
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
            crate::log_legacy!("[TriggerDefn] mkdir {:?} failed: {}", parent, e);
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, toml) {
        crate::log_legacy!("[TriggerDefn] write {:?} failed: {}", path, e);
    }
}

pub fn remove_trigger_definition(workspace_path: &Path, slug: &str) {
    if slug.is_empty() {
        return;
    }
    let path = trigger_toml_abspath(workspace_path, slug);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            crate::log_legacy!("[TriggerDefn] remove {:?} failed: {}", path, e);
            return;
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
}

/// INVARIANT: Rebuild the whole projection from the current trigger set: write every
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

    // INVARIANT: Prune orphan trigger.toml files (slug no longer live). Only the
    // trigger.toml is removed — sibling knowhow/scripts stay.
    let triggers_dir = workspace_path.join(DATA_DIR).join("triggers");
    let Ok(entries) = std::fs::read_dir(&triggers_dir) else {
        return;
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

const TRIGGER_TOML_EXCLUDE: &str = "data/triggers/*/trigger.toml";

/// INVARIANT: Ensure the workspace repo ignores the projected `trigger.toml` files via the
/// repo-local `.git/info/exclude` (so the committed `.gitignore` is untouched).
/// Idempotent + best-effort: a non-standard `.git` (worktree file, missing info
/// dir) just degrades to the files showing as untracked — never an error.
///
/// A read that FAILED is not an empty file. A non-UTF-8 byte or any I/O error
/// leaves the file alone, because the write below replaces the whole thing.
pub fn ensure_trigger_toml_gitignored(workspace_path: &Path) {
    let exclude = workspace_path.join(".git").join("info").join("exclude");
    let existing = match std::fs::read_to_string(&exclude) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            crate::log_legacy!(
                "[TriggerDefn] read {:?} failed, leaving it untouched: {}",
                exclude,
                e
            );
            return;
        }
    };
    if existing.lines().any(|l| l.trim() == TRIGGER_TOML_EXCLUDE) {
        return;
    }
    let Some(parent) = exclude.parent() else {
        return;
    };
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
    if let Err(e) = std::fs::write(&exclude, next) {
        crate::log_legacy!("[TriggerDefn] write {:?} failed: {}", exclude, e);
    }
}

#[cfg(test)]
#[path = "definition_tests.rs"]
mod tests;
