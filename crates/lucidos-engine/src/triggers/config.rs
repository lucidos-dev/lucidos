use crate::core::event_subscription::EventSubscription;
use crate::engine::command_guard::SideEffectCategory;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Supported script file extensions and their runtime labels.
pub const SUPPORTED_SCRIPT_EXTENSIONS: &[(&str, &str)] = &[("py", "Python"), ("sh", "Bash")];

/// Validate that a script path has a supported file extension.
/// Returns `Ok(())` if valid, `Err(message)` if unsupported or missing.
pub fn validate_script_extension(path: &str) -> Result<(), String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if SUPPORTED_SCRIPT_EXTENSIONS.iter().any(|(e, _)| *e == ext) {
        return Ok(());
    }
    let list = supported_extensions_display();
    if ext.is_empty() {
        Err(format!(
            "Script path must have a file extension (supported: {})",
            list
        ))
    } else {
        Err(format!(
            "Unsupported script extension '.{}' (supported: {})",
            ext, list
        ))
    }
}

/// Format the supported extensions list for error messages.
fn supported_extensions_display() -> String {
    SUPPORTED_SCRIPT_EXTENSIONS
        .iter()
        .map(|(e, _)| *e)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Outcome of a trigger's most recent completed firing. Surfaced on the
/// trigger row (OK / failed) so a threadless (script) trigger's health is
/// visible without opening its event stream. Wire values are `"ok"` /
/// `"failed"`; `None` on the config means "never run / legacy" (a
/// pre-status-field `TriggerExecuted` event carries no outcome).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerRunStatus {
    Ok,
    Failed,
}

impl TriggerRunStatus {
    /// Map a run outcome (`execute_user_task`'s `Result`) to a status.
    pub fn from_success(success: bool) -> Self {
        if success {
            Self::Ok
        } else {
            Self::Failed
        }
    }

    /// Wire string used in the `TriggerExecuted` payload and the trigger-list API.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
        }
    }
}

/// What a trigger executes — either an LLM intent or a deterministic script.
/// The tagged union enforces mutual exclusivity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TriggerRun {
    #[serde(rename = "intent", alias = "prompt")]
    Intent {
        #[serde(alias = "text")]
        intent: String,
    },
    #[serde(rename = "script")]
    Script { path: String },
}

/// In-memory representation of a trigger, rebuilt from events on startup.
#[derive(Debug, Clone, Serialize)]
pub struct TriggerConfig {
    pub id: String,
    pub name: String,
    /// Stable kebab-case identifier derived from `name` (or supplied explicitly
    /// at create time). Used as the directory segment for per-trigger
    /// know-how at `data/triggers/{slug}/knowhow/`. Legacy `TriggerCreated`
    /// payloads without this field derive it on read so existing workspaces
    /// keep working.
    pub slug: String,
    pub schedule: Vec<String>,
    pub timezone: String,
    pub run: TriggerRun,
    /// Event subscriptions. Empty when the trigger is schedule-only.
    pub on: Vec<EventSubscription>,
    pub paused: bool,
    pub last_run: Option<DateTime<Utc>>,
    /// Outcome of the most recent completed firing (OK / failed). `None` until
    /// the trigger has run at least once under an engine that records status
    /// (legacy `TriggerExecuted` events carry no outcome → stays `None`).
    /// Set in memory by `record_trigger_executed` and rebuilt from the
    /// `TriggerExecuted` payload on boot (see `replay.rs`), mirroring `last_run`.
    pub last_run_status: Option<TriggerRunStatus>,
    /// Directory name of the app that owns this trigger (e.g. `"trigger-workflow"`),
    /// stamped onto `NotificationCreated.app_id` so the popover can deep-link to
    /// the app. None for standalone triggers. For script triggers under
    /// `apps/<X>/...` without an explicit value, `owning_app_id` derives `<X>`.
    pub app_id: Option<String>,
    /// When true, threads spawned by this trigger surface in REVIEW on
    /// completion instead of going straight to ARCHIVE. Use for triggers
    /// whose output the user is expected to read — daily summaries, alerts,
    /// scheduled reports. Default false preserves the unattended-execution
    /// behavior expected of most cron triggers.
    pub go_to_review: bool,
    /// Optional id of the *trigger group* this trigger belongs to. Pure
    /// organizational label — does not affect firing. None renders the
    /// trigger under the implicit "Ungrouped" section in the panel.
    pub group_id: Option<String>,
    /// The trigger's **side-effect grant** (ADR 0002, Phase 5): the set of
    /// irreversible side-effect categories it's authorized to perform
    /// unattended. Empty (the default) means the trigger may NOT perform any
    /// irreversible side-effect — the command guard fails the trigger if its
    /// intent tries to. Only consulted when the `command_guard` preference is on.
    pub side_effect_grant: Vec<SideEffectCategory>,
    /// **Plugin provenance** (ADR 0019): the id of the *plugin* that
    /// auto-registered this trigger at install, or `None` for a user-created
    /// trigger. Plugin uninstall removes exactly the triggers carrying its id;
    /// plugin update re-syncs them by `(plugin_id, slug)`. A user trigger
    /// (`None`) is never touched by a plugin's lifecycle.
    pub plugin_id: Option<String>,
}

/// Convert a human-facing trigger name to a stable kebab-case slug, guaranteeing
/// a non-empty result by falling back to the first 8 chars of the trigger UUID
/// (dashes stripped) when the name slugifies to empty (e.g. `"!!!"`).
///
/// The slugification itself is [`crate::core::slug::slugify_kebab`], shared with
/// coding-agent branch naming; only the trigger-specific fallback lives here.
pub fn slugify_trigger_name_with_fallback(name: &str, uuid: &str) -> String {
    let s = crate::core::slug::slugify_kebab(name);
    if s.is_empty() {
        let no_dashes = uuid.replace('-', "");
        let suffix: String = no_dashes.chars().take(8).collect();
        format!("trigger-{}", suffix)
    } else {
        s
    }
}

/// Parse a trigger payload's `on` field into a Vec of subscriptions. Accepts:
///
/// 1. Array of objects: `[{"event_type": "X", "condition": {...}}, ...]` —
///    each entry maps to one [`EventSubscription`].
/// 2. Array of strings: `["X", "Y"]` — convenience shorthand; each string
///    becomes an entry with no condition.
/// 3. Absent or `null` — empty Vec (schedule-only trigger).
///
/// The pre-`20260516195912_migrate_trigger_on_to_subscription_list.sql` shape
/// (top-level `on: "X"` string + sibling top-level `condition`) is rewritten
/// by that migration before this reader sees it, so the legacy branches are
/// intentionally absent here. Unknown / malformed entries are skipped
/// silently so a single bad row can never wedge the in-memory config.
pub(crate) fn parse_event_subscriptions(on_field: Option<&Value>) -> Vec<EventSubscription> {
    let Some(arr) = on_field.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|entry| {
            if let Some(s) = entry.as_str() {
                let s = s.trim();
                (!s.is_empty()).then(|| EventSubscription {
                    event_type: s.to_string(),
                    condition: None,
                })
            } else {
                EventSubscription::from_object_entry(entry.as_object()?)
            }
        })
        .collect()
}

/// Parse a payload's `side_effect_grant` field into a deduped list of
/// [`SideEffectCategory`]. Accepts an array of snake_case category strings
/// (`["email", "external_api"]`); absent / null / non-array → empty. Unknown
/// entries are skipped silently so a forward-compat value (a category a newer
/// engine added) can't wedge the in-memory config on an older one.
pub(crate) fn parse_side_effect_grant(field: Option<&Value>) -> Vec<SideEffectCategory> {
    let Some(arr) = field.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<SideEffectCategory> = Vec::new();
    for entry in arr {
        if let Ok(cat) = serde_json::from_value::<SideEffectCategory>(entry.clone()) {
            if !out.contains(&cat) {
                out.push(cat);
            }
        }
    }
    out
}

impl TriggerConfig {
    /// Build a TriggerConfig from a TriggerCreated event payload.
    pub fn from_created_payload(payload: &Value) -> Result<Self, String> {
        let id = payload["trigger_id"]
            .as_str()
            .ok_or("Missing trigger_id")?
            .to_string();
        let name = payload["name"].as_str().ok_or("Missing name")?.to_string();
        let schedule = payload["schedule"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let timezone = payload["timezone"].as_str().unwrap_or("UTC").to_string();
        let run: TriggerRun = serde_json::from_value(payload["run"].clone())
            .map_err(|e| format!("Invalid run field: {}", e))?;
        let on = parse_event_subscriptions(payload.get("on"));

        let paused = read_paused_field(payload).unwrap_or(false);
        let app_id = payload
            .get("app_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let go_to_review = payload
            .get("go_to_review")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // group_id is optional — legacy events lack the field, which deserializes
        // to None (i.e. the trigger renders under "Ungrouped" in the panel).
        let group_id = payload
            .get("group_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        // Legacy `TriggerCreated` events lack `slug`; derive from name (with
        // UUID fallback) so existing workspaces keep resolving without a
        // backfill migration. New events from the API carry slug explicitly.
        let slug = payload
            .get("slug")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| slugify_trigger_name_with_fallback(&name, &id));
        let side_effect_grant = parse_side_effect_grant(payload.get("side_effect_grant"));
        // Plugin provenance (ADR 0019) — present only on plugin-auto-registered
        // triggers; absent on user-created ones (→ None, never plugin-managed).
        let plugin_id = payload
            .get("plugin_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);

        Ok(TriggerConfig {
            id,
            name,
            slug,
            schedule,
            timezone,
            run,
            on,
            paused,
            last_run: None,
            last_run_status: None,
            app_id,
            go_to_review,
            group_id,
            side_effect_grant,
            plugin_id,
        })
    }

    /// Directory name of the app that owns this trigger, used to stamp notifications.
    /// Prefers the explicit `app_id` field; for script triggers without one, falls back
    /// to the leading `apps/<dir>/` path segment so legacy app-scoped scripts still link
    /// back to their app. Returns None when the trigger is genuinely standalone.
    pub fn owning_app_id(&self) -> Option<String> {
        if let Some(ref aid) = self.app_id {
            return Some(aid.clone());
        }
        if let TriggerRun::Script { ref path } = self.run {
            return derive_app_id_from_script_path(path);
        }
        None
    }

    /// Compute the next scheduled run time (UTC) from cron expressions and timezone.
    /// Returns None if the trigger is paused, has no cron expressions, or no future match exists.
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        if self.paused || self.schedule.is_empty() {
            return None;
        }
        let schedules: Vec<cron::Schedule> = self
            .schedule
            .iter()
            .filter_map(|expr| {
                crate::engine::tools::scheduler::parse_standard_cron(expr)
                    .map_err(|e| {
                        log!(
                            "[Triggers] Corrupt cron expression '{}' in trigger {}: {}",
                            expr,
                            self.id,
                            e
                        );
                    })
                    .ok()
            })
            .collect();
        let tz: chrono_tz::Tz = self.timezone.parse().unwrap_or_else(|_| {
            log!(
                "[Triggers] Invalid timezone '{}' for trigger {}, using UTC",
                self.timezone,
                self.id
            );
            chrono_tz::UTC
        });
        crate::engine::tools::scheduler::next_occurrence_multi(&schedules, tz)
            .map(|dt| dt.with_timezone(&Utc))
    }

    /// Human-readable trigger type label.
    pub fn trigger_type_label(&self) -> &'static str {
        let has_cron = !self.schedule.is_empty();
        let has_event = !self.on.is_empty();
        match (has_cron, has_event) {
            (true, true) => "Hybrid",
            (false, true) => "Event",
            _ => "Schedule",
        }
    }

    /// Apply a partial update from a TriggerUpdated event payload.
    /// Only fields present in the payload are updated.
    ///
    /// Note: legacy `run.knowhow: [...]` payloads are silently dropped — Phase 1
    /// of the trigger-knowhow-discovery refactor removed the field, and serde
    /// ignores unknown fields on `TriggerRun`. The rest of the `run` object is
    /// applied normally.
    pub fn apply_update(&mut self, payload: &Value) {
        if let Some(name) = payload["name"].as_str() {
            self.name = name.to_string();
        }
        // Slug edits (e.g. trigger renamed) propagate so the per-trigger
        // know-how dir resolves against the new slug. Validation of edit
        // shape happens at the API boundary; corrupt payloads here are
        // ignored so a bad event can never wedge the in-memory config.
        if let Some(slug) = payload.get("slug").and_then(|v| v.as_str()) {
            if is_valid_trigger_slug(slug) {
                self.slug = slug.to_string();
            } else {
                log!(
                    "[Triggers] Ignored invalid slug '{}' in TriggerUpdated for {}",
                    slug,
                    self.id
                );
            }
        }
        if let Some(schedule) = payload["schedule"].as_array() {
            self.schedule = schedule
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(tz) = payload["timezone"].as_str() {
            self.timezone = tz.to_string();
        }
        if payload.get("run").is_some() && !payload["run"].is_null() {
            if let Ok(run) = serde_json::from_value(payload["run"].clone()) {
                self.run = run;
            }
        }
        // Full replacement. `on: null` / `on: []` clears.
        if payload.get("on").is_some() {
            self.on = parse_event_subscriptions(payload.get("on"));
        }
        if let Some(paused) = read_paused_field(payload) {
            self.paused = paused;
        }
        // app_id update: explicit null clears, string sets, absent leaves as-is
        if let Some(v) = payload.get("app_id") {
            if v.is_null() {
                self.app_id = None;
            } else if let Some(s) = v.as_str() {
                self.app_id = Some(s.to_string());
            }
        }
        if let Some(v) = payload.get("go_to_review").and_then(|v| v.as_bool()) {
            self.go_to_review = v;
        }
        // group_id update: explicit null clears, string sets, absent leaves as-is —
        // mirrors the app_id pattern so triggers can move between / out of groups.
        if let Some(v) = payload.get("group_id") {
            if v.is_null() {
                self.group_id = None;
            } else if let Some(s) = v.as_str() {
                self.group_id = Some(s.to_string());
            }
        }
        // side_effect_grant update: full replacement when present (array → the
        // new grant set, `[]` / null → clears it); absent leaves it as-is. The
        // HTTP layer always sends the whole array on a change, mirroring `on`.
        if payload.get("side_effect_grant").is_some() {
            self.side_effect_grant = parse_side_effect_grant(payload.get("side_effect_grant"));
        }
        // plugin_id (ADR 0019): a string sets/updates provenance (plugin update
        // re-sync re-stamps it); absent leaves it as-is. Deliberately NOT
        // clearable here — a user edit (no plugin_id in payload) must not strip a
        // plugin trigger's provenance, or uninstall could no longer reclaim it.
        if let Some(s) = payload.get("plugin_id").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                self.plugin_id = Some(s.to_string());
            }
        }
    }
}

/// Extract the owning app directory from a script path, if it lives under `apps/<X>/`.
/// Rejects path-traversal segments (`.`, `..`) and leading-dot dirs so a malformed
/// path can't become a fake app id on the frontend popover.
/// Examples:
/// - `"apps/trigger-workflow/triggers/scripts/run.py"` → `Some("trigger-workflow")`
/// - `"triggers/oura-import/scripts/run.py"` → `None`
/// - `"apps/../foo/bar"` / `"apps/.git/x"` / `"apps//x"` → `None`
pub(crate) fn derive_app_id_from_script_path(path: &str) -> Option<String> {
    let mut parts = path.split('/');
    if parts.next()? != "apps" {
        return None;
    }
    let dir = parts.next()?;
    if dir.is_empty() || dir.starts_with('.') {
        return None;
    }
    Some(dir.to_string())
}

/// True if `slug` is a well-formed trigger slug suitable as a directory name.
///
/// Length 1-64, ASCII lowercase + digits + dashes, must start AND end with
/// `[a-z0-9]` (so `--foo` and `foo--` are rejected). Used by both the API
/// boundary (HTTP 400 on reject) and the `apply_update` path (drop bad
/// in-flight edits).
pub fn is_valid_trigger_slug(slug: &str) -> bool {
    let len = slug.len();
    if !(1..=64).contains(&len) {
        return false;
    }
    let bytes = slug.as_bytes();
    let is_ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !is_ok(bytes[0]) || !is_ok(bytes[len - 1]) {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Read the paused state from an event payload.
/// Prefers the new `paused` field; falls back to the inverted legacy `enabled` field
/// so events persisted before the rename still apply correctly. Returns None when neither
/// field is present, letting callers decide their default.
fn read_paused_field(payload: &Value) -> Option<bool> {
    payload
        .get("paused")
        .and_then(|v| v.as_bool())
        .or_else(|| payload.get("enabled").and_then(|v| v.as_bool()).map(|e| !e))
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
