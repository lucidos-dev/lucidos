use crate::core::event_subscription::EventSubscription;
use crate::engine::command_guard::SideEffectCategory;
use crate::engine::context_ceiling::ContextCeiling;
use crate::triggers::actor::{ActorField, ACTOR_FIELD_KEY};
use crate::triggers::continue_key::ContinueKeySpec;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SUPPORTED_SCRIPT_EXTENSIONS: &[(&str, &str)] = &[("py", "Python"), ("sh", "Bash")];

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

fn supported_extensions_display() -> String {
    SUPPORTED_SCRIPT_EXTENSIONS
        .iter()
        .map(|(e, _)| *e)
        .collect::<Vec<_>>()
        .join(", ")
}

pub use crate::engine::trigger_context::TriggerRunStatus;

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

/// INVARIANT: A program scope using the command guard's permission syntax. Historical
/// invocation records carry `legacy` and confer no authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerProgramGrant {
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy: Option<LegacyInvocationGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyInvocationGrant {
    pub fingerprint: String,
    pub category: SideEffectCategory,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggerConfig {
    pub id: String,
    pub name: String,
    /// INVARIANT: Stable kebab-case identifier derived from `name` (or supplied explicitly
    /// at create time). Used as the directory segment for per-trigger
    /// know-how at `data/triggers/{slug}/knowhow/`. Legacy `TriggerCreated`
    /// payloads without this field derive it on read so existing workspaces
    /// keep working.
    pub slug: String,
    pub schedule: Vec<String>,
    pub timezone: String,
    pub run: TriggerRun,
    pub on: Vec<EventSubscription>,
    pub paused: bool,
    pub last_run: Option<DateTime<Utc>>,
    /// INVARIANT: Outcome of the most recent completed firing (OK / failed). `None` until
    /// the trigger has run at least once under an engine that records status
    /// (legacy `TriggerExecuted` events carry no outcome → stays `None`).
    /// Set in memory by `record_trigger_executed` and rebuilt from the
    /// `TriggerExecuted` payload on boot (see `replay.rs`), mirroring `last_run`.
    pub last_run_status: Option<TriggerRunStatus>,
    pub app_id: Option<String>,
    pub go_to_review: bool,
    pub group_id: Option<String>,
    pub side_effect_grant: Vec<SideEffectCategory>,
    pub program_grants: Vec<TriggerProgramGrant>,
    pub folder_grants: Vec<super::folder_grant::TriggerFolderGrant>,
    pub program_denies: Vec<String>,
    pub folder_denies: Vec<std::path::PathBuf>,
    pub coding_agent_grant: bool,
    pub context_ceiling: Option<ContextCeiling>,
    pub mcp_servers: Vec<String>,
    pub plugin_id: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    /// INVARIANT: The backend this trigger's model runs on, when the model has more than
    /// one route. `None` (the default) lets the model's own *preferred
    /// provider* decide, then its first configured route.
    ///
    /// A pin naming an unconfigured backend REFUSES the fire, rather than
    /// running it somewhere else: a trigger fires unattended, so a silent move
    /// to another vendor is one nobody would notice. Intent-only, like `model`.
    pub provider: Option<String>,
    pub continue_key: Option<ContinueKeySpec>,
    pub actor: Option<ActorField>,
}

/// INVARIANT: True if `effort` is a reasoning tier a trigger may pin. Shares the closed set
/// with the `chat_reasoning_efforts` preference so the two vocabularies cannot
/// drift. Per-*model* availability is deliberately not checked here: the engine
/// clamps an unsupported tier at call time, and a stored trigger must not break
/// when a model's supported tiers change under it.
pub fn is_valid_reasoning_effort(effort: &str) -> bool {
    crate::core::preference_catalog::REASONING_EFFORTS.contains(&effort)
}

pub fn is_valid_provider(provider: &str) -> bool {
    crate::llm::ProviderKind::from_name(provider).is_some()
}

/// INVARIANT: Normalize a submitted or stored model / reasoning effort / provider pin:
/// trim, and read blank or the literal `null` as "Default" (`None`). Blank is
/// how the form's Default option travels. `null` is how a CLI flag spells
/// JSON null, and a model sometimes writes JSON null as that string too. No
/// model id, tier or provider is called `null`. Neither may ever be stored: the
/// chat route resolver reads any `Some` as a genuine override. Shared with the
/// `run_thread` spawn pins, so a trigger and a chat-spawned child agree.
///
/// Replay goes through here too, so an event stored before the `null` rule
/// reads back as unpinned, which is what its caller asked for.
///
/// Deliberately does NOT check the model against the registry, matching the
/// `chat_model` preference (`PrefValue::Text`) and `ChatRequest.model`. A model
/// row can be disabled or deleted long after a trigger was saved, routing
/// already degrades to the prefix heuristic, and a genuinely bad id surfaces as
/// an ordinary trigger-failure notification rather than a save the user cannot
/// make.
pub fn normalize_route_setting(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
        .map(str::to_string)
}

pub fn validate_trigger_reasoning_effort(raw: Option<&str>) -> Result<Option<String>, String> {
    match normalize_route_setting(raw) {
        Some(effort) if !is_valid_reasoning_effort(&effort) => Err(format!(
            "Invalid reasoning_effort '{}': expected one of none, low, medium, high, xhigh, max",
            effort
        )),
        normalized => Ok(normalized),
    }
}

pub fn validate_trigger_provider(
    registry: &crate::llm::ModelRegistry,
    model: Option<&str>,
    provider: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(provider) = normalize_route_setting(provider) else {
        return Ok(None);
    };
    let Some(kind) = crate::llm::ProviderKind::from_name(&provider) else {
        return Err(crate::llm::model_registry::unknown_provider_message(
            &provider,
        ));
    };
    let Some(model) = normalize_route_setting(model) else {
        return Err(format!(
            "A provider pin needs a model pin: '{provider}' says which backend serves \
             the trigger's model, so set model too"
        ));
    };
    match crate::llm::model_registry::route_on(registry, &model, kind) {
        Some(_) => Ok(Some(provider)),
        None => Err(format!(
            "Model '{model}' has no route on provider '{provider}'. Pin one of the \
             model's own providers, or leave provider unset"
        )),
    }
}

pub fn resolve_trigger_provider_update(
    registry: &crate::llm::ModelRegistry,
    existing: &TriggerConfig,
    new_model: Option<Option<&str>>,
    new_provider: Option<Option<&str>>,
) -> Result<Option<Option<String>>, String> {
    let model = match new_model {
        Some(m) => normalize_route_setting(m),
        None => existing.model.clone(),
    };
    match new_provider {
        Some(p) => validate_trigger_provider(registry, model.as_deref(), p).map(Some),
        None if existing.provider.is_some() && model != existing.model => Ok(Some(None)),
        None => Ok(None),
    }
}

fn read_trimmed_string(payload: &Value, key: &str) -> Option<String> {
    normalize_route_setting(payload.get(key).and_then(|v| v.as_str()))
}

const MAX_TRIGGER_SLUG_LEN: usize = 64;

const MAX_SLUG_COLLISION_ATTEMPTS: u32 = 99;

fn slug_from_trigger_id(trigger_id: &str) -> String {
    let no_dashes = trigger_id.replace('-', "");
    let suffix: String = no_dashes.chars().take(8).collect();
    format!("trigger-{}", suffix)
}

/// INVARIANT: Convert a human-facing trigger name to a stable kebab-case slug, guaranteeing
/// a non-empty result by falling back to the first 8 chars of the trigger UUID
/// (dashes stripped) when the name slugifies to empty (e.g. `"!!!"`).
///
/// The slugification itself is [`crate::core::slug::slugify_kebab`], shared with
/// coding-agent branch naming; only the trigger-specific fallback lives here.
///
/// **A pure function of the name, so two triggers called "Daily Summary" get the
/// same answer.** Any create path that mints a slug for a NEW trigger must go
/// through [`mint_unique_trigger_slug`] instead.
pub fn slugify_trigger_name_with_fallback(name: &str, uuid: &str) -> String {
    let s = crate::core::slug::slugify_kebab(name);
    if s.is_empty() {
        slug_from_trigger_id(uuid)
    } else {
        s
    }
}

/// INVARIANT: Mint a slug for a NEW trigger that no live trigger already owns.
///
/// `taken` is every slug currently in the registry. The base is
/// [`slugify_trigger_name_with_fallback`]; a collision appends `-2`, then `-3`,
/// and so on. Every result is cut to [`MAX_TRIGGER_SLUG_LEN`], so what this
/// mints always passes [`is_valid_trigger_slug`].
///
/// The slug is the `data/triggers/<slug>/` directory segment for both the
/// `trigger.toml` projection and the per-trigger knowhow. Two triggers sharing
/// one therefore overwrite each other's definition on every boot, read each
/// other's knowhow, and lose the survivor's projection when either is deleted.
/// The sibling precedent is the `foreign_slugs` refusal in
/// `resync_plugin_triggers`, which declines the slug rather than renaming it:
/// a plugin's slug is its shipped directory name, while this one is ours to pick.
pub fn mint_unique_trigger_slug(
    name: &str,
    trigger_id: &str,
    taken: &std::collections::HashSet<String>,
) -> String {
    let base = crate::core::slug::truncate_slug(
        &slugify_trigger_name_with_fallback(name, trigger_id),
        MAX_TRIGGER_SLUG_LEN,
    );
    if !taken.contains(&base) {
        return base;
    }
    for n in 2..=MAX_SLUG_COLLISION_ATTEMPTS {
        let suffix = format!("-{n}");
        let head = crate::core::slug::truncate_slug(&base, MAX_TRIGGER_SLUG_LEN - suffix.len());
        let candidate = format!("{head}{suffix}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    slug_from_trigger_id(trigger_id)
}

/// INVARIANT: Parse a trigger payload's `on` field into a Vec of subscriptions. Accepts:
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

pub(crate) fn parse_context_ceiling(
    field: Option<&Value>,
    trigger_id: &str,
) -> Option<ContextCeiling> {
    let value = field.filter(|v| !v.is_null())?;
    value
        .as_str()
        .ok_or_else(|| format!("a context ceiling is a string (got {value})"))
        .and_then(ContextCeiling::parse)
        .map_err(|e| {
            log_legacy!(
                "[Triggers] Ignored context_ceiling on {}: {}",
                trigger_id,
                e
            )
        })
        .ok()
}

pub fn normalize_mcp_servers<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids.into_iter().map(str::trim).filter(|id| !id.is_empty()) {
        if !out.iter().any(|seen| seen == id) {
            out.push(id.to_string());
        }
    }
    out
}

pub(crate) fn parse_mcp_servers(field: Option<&Value>) -> Vec<String> {
    let Some(arr) = field.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    normalize_mcp_servers(arr.iter().filter_map(|v| v.as_str()))
}

fn parse_program_denies(payload: &Value) -> Vec<String> {
    serde_json::from_value(payload["program_denies"].clone()).unwrap_or_default()
}

fn parse_folder_denies(payload: &Value) -> Vec<std::path::PathBuf> {
    serde_json::from_value(payload["folder_denies"].clone()).unwrap_or_default()
}

fn listed<'a>(payload: &'a Value, key: &str) -> Option<Vec<&'a str>> {
    payload
        .get(key)
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
}

pub(crate) fn parse_program_grants(payload: &Value) -> Vec<TriggerProgramGrant> {
    payload["program_grants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|grant| serde_json::from_value(grant.clone()).ok())
        .collect()
}

impl TriggerConfig {
    pub fn holds_a_grant(&self) -> bool {
        self.coding_agent_grant
            || !self.side_effect_grant.is_empty()
            || !self.program_grants.is_empty()
            || !self.folder_grants.is_empty()
    }

    pub fn denies_program(&self, pattern: &str) -> bool {
        self.program_denies.iter().any(|denied| denied == pattern)
    }

    pub fn denied_folder(&self, folder: &std::path::Path) -> Option<&std::path::Path> {
        self.folder_denies
            .iter()
            .map(std::path::PathBuf::as_path)
            .find(|denied| folder.starts_with(denied))
    }

    pub fn deny(&mut self, programs: &[String], folders: &[std::path::PathBuf]) {
        for pattern in programs {
            if !self.program_denies.contains(pattern) {
                self.program_denies.push(pattern.clone());
            }
        }
        for folder in folders {
            if !self.folder_denies.contains(folder) {
                self.folder_denies.push(folder.clone());
            }
        }
        self.settle_denies(true);
    }

    /// INVARIANT: Keep each entry on one side only. `denies_win` says which side the
    /// newest write set, so it keeps the entries both sides hold. Two folders
    /// conflict when either holds the other: a grant over a denied folder
    /// would let its writes through, so the deny would never fire.
    fn settle_denies(&mut self, denies_win: bool) {
        fn nested(a: &std::path::Path, b: &std::path::Path) -> bool {
            a.starts_with(b) || b.starts_with(a)
        }
        if denies_win {
            let denies = (self.program_denies.clone(), self.folder_denies.clone());
            self.program_grants
                .retain(|grant| !denies.0.contains(&grant.pattern));
            self.folder_grants
                .retain(|grant| !denies.1.iter().any(|denied| nested(&grant.folder, denied)));
        } else {
            let grants = (self.program_grants.clone(), self.folder_grants.clone());
            self.program_denies
                .retain(|denied| !grants.0.iter().any(|grant| &grant.pattern == denied));
            self.folder_denies
                .retain(|denied| !grants.1.iter().any(|grant| nested(&grant.folder, denied)));
        }
    }

    pub fn widens_a_grant(&self, next: &TriggerConfig) -> bool {
        (next.coding_agent_grant && !self.coding_agent_grant)
            || next
                .side_effect_grant
                .iter()
                .any(|c| !self.side_effect_grant.contains(c))
            || next
                .program_grants
                .iter()
                .any(|grant| !self.program_grants.contains(grant))
            || next
                .folder_grants
                .iter()
                .any(|grant| !self.folder_grants.contains(grant))
    }

    /// INVARIANT: Whether an agent's or app frame's edit to `next` must drop its grants.
    /// The owner granted this trigger's work, so a change to its run, its
    /// events or its MCP servers needs the owner again.
    pub fn unowned_edit_clears_grants(&self, next: &TriggerConfig) -> bool {
        fn servers(c: &TriggerConfig) -> std::collections::BTreeSet<&str> {
            c.mcp_servers.iter().map(String::as_str).collect()
        }
        self.holds_a_grant()
            && (next.run != self.run || next.on != self.on || servers(next) != servers(self))
    }

    pub fn definition_digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let servers: std::collections::BTreeSet<&str> =
            self.mcp_servers.iter().map(String::as_str).collect();
        let definition = serde_json::json!({
            "run": self.run,
            "on": self.on,
            "cron": self.schedule,
            "mcp_servers": servers,
            "app": self.owning_app_id(),
        });
        let canonical = crate::core::snapshot::manifest::canonical_json(&definition)
            .expect("a trigger definition is plain JSON");
        crate::core::hex::hex_lower(&Sha256::digest(canonical))
    }

    pub fn clear_grants_in(payload: &mut Value) {
        payload["coding_agent_grant"] = Value::Bool(false);
        payload["side_effect_grant"] = Value::Array(Vec::new());
        payload["program_grants"] = Value::Array(Vec::new());
        payload["folder_grants"] = Value::Array(Vec::new());
    }

    pub fn grants_cleared_notice(&self) -> String {
        format!(
            "IMPORTANT, tell the user now: this edit changed what the trigger \"{}\" runs, so \
             Lucidos removed its grants (coding agents and allowed side-effects). Until the \
             user grants them again, a run that needs one fails with a notification. Ask the \
             user to open the trigger and tick them again if they trust the new version. You \
             cannot tick them for them.",
            self.name
        )
    }

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
        let timezone = payload["timezone"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(crate::triggers::definition::default_timezone);
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
        let group_id = payload
            .get("group_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        // INVARIANT: Legacy `TriggerCreated` events lack `slug`; derive from name (with
        // UUID fallback) so existing workspaces keep resolving without a
        // backfill migration. New events from the API carry slug explicitly.
        // The slug becomes a path segment under `data/triggers/`, so a stored
        // `../../x` would write and delete outside the workspace data dir. The
        // API rejects one, but any event row reaches here.
        //
        // Path safety only, NOT the canonical shape `apply_update` demands. A
        // plugin trigger takes its slug from the installed directory segment,
        // which carries no shape rule. Rejecting `daily_reflect` here would
        // substitute a name-derived slug and break the three-way lockstep
        // `resync_plugin_triggers` needs. The next plugin update would then read
        // the trigger as undeclared and delete it.
        let slug = payload
            .get("slug")
            .and_then(|v| v.as_str())
            .filter(|s| {
                let ok = is_path_safe_trigger_slug(s);
                if !ok {
                    log_legacy!(
                        "[Triggers] Ignored unsafe slug '{}' in TriggerCreated for {}",
                        s,
                        id
                    );
                }
                ok
            })
            .map(String::from)
            .unwrap_or_else(|| slugify_trigger_name_with_fallback(&name, &id));
        let side_effect_grant = parse_side_effect_grant(payload.get("side_effect_grant"));
        let coding_agent_grant = payload
            .get("coding_agent_grant")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let context_ceiling = parse_context_ceiling(payload.get("context_ceiling"), &id);
        let mcp_servers = parse_mcp_servers(payload.get("mcp_servers"));
        let plugin_id = payload
            .get("plugin_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let model = read_trimmed_string(payload, "model");
        let reasoning_effort = read_trimmed_string(payload, "reasoning_effort")
            .filter(|e| is_valid_reasoning_effort(e));
        let provider = read_trimmed_string(payload, "provider").filter(|p| is_valid_provider(p));
        let continue_key = ContinueKeySpec::from_stored(payload.get("continue_key"), &id);
        let actor = ActorField::from_stored(payload.get(ACTOR_FIELD_KEY), &id);

        let mut config = TriggerConfig {
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
            program_grants: parse_program_grants(payload),
            folder_grants: serde_json::from_value(payload["folder_grants"].clone())
                .unwrap_or_default(),
            program_denies: parse_program_denies(payload),
            folder_denies: parse_folder_denies(payload),
            coding_agent_grant,
            context_ceiling,
            mcp_servers,
            plugin_id,
            model,
            reasoning_effort,
            provider,
            continue_key,
            actor,
        };
        config.settle_denies(true);
        Ok(config)
    }

    pub fn owning_app_id(&self) -> Option<String> {
        if let Some(ref aid) = self.app_id {
            return Some(aid.clone());
        }
        if let TriggerRun::Script { ref path } = self.run {
            return derive_app_id_from_script_path(path);
        }
        None
    }

    pub(crate) fn timezone_or_utc(&self) -> chrono_tz::Tz {
        self.timezone.parse().unwrap_or_else(|_| {
            log_legacy!(
                "[Triggers] Invalid timezone '{}' for trigger {}, using UTC",
                self.timezone,
                self.id
            );
            chrono_tz::UTC
        })
    }

    fn parsed_schedules(&self) -> Vec<cron::Schedule> {
        self.schedule
            .iter()
            .filter_map(|expr| {
                crate::scheduler::cron_rules::parse_standard_cron(expr)
                    .map_err(|e| {
                        log_legacy!(
                            "[Triggers] Corrupt cron expression '{}' in trigger {}: {}",
                            expr,
                            self.id,
                            e
                        );
                    })
                    .ok()
            })
            .collect()
    }

    pub fn next_runs(&self, n: usize) -> Vec<DateTime<Utc>> {
        self.next_runs_after(Utc::now(), n)
    }

    pub fn next_runs_after(&self, now: DateTime<Utc>, n: usize) -> Vec<DateTime<Utc>> {
        if self.paused || self.schedule.is_empty() {
            return Vec::new();
        }
        let schedules = self.parsed_schedules();
        crate::scheduler::cron_rules::next_occurrences_after(
            &schedules,
            self.timezone_or_utc(),
            now,
            n,
        )
        .into_iter()
        .map(|dt| dt.with_timezone(&Utc))
        .collect()
    }

    /// INVARIANT: A diagnosis when this trigger's schedule can never fire, for the errored
    /// state in the panel and the boot warning.
    ///
    /// Deliberately independent of `paused`, unlike [`Self::next_runs`]: a paused
    /// trigger with a dead schedule is still misconfigured, and resuming it would
    /// change nothing. Also deliberately all-or-nothing: one live expression means
    /// the trigger genuinely fires, so flagging it because a *sibling* expression
    /// is dead would put a red chip on a working trigger. Create and update now
    /// reject a dead expression outright, so the only way to reach this state is a
    /// trigger stored before the guard existed.
    pub fn schedule_error(&self) -> Option<String> {
        if self.schedule.is_empty() {
            return None;
        }
        let tz = self.timezone_or_utc();
        let mut first_problem: Option<String> = None;
        for expr in &self.schedule {
            match crate::scheduler::cron_rules::parse_standard_cron(expr) {
                Ok(schedule) => {
                    if schedule.upcoming(tz).next().is_some() {
                        return None;
                    }
                    first_problem.get_or_insert_with(|| {
                        format!(
                            "'{}' can never fire: {}",
                            expr,
                            crate::scheduler::cron_rules::diagnose_never_fires(&schedule)
                        )
                    });
                }
                Err(e) => {
                    first_problem.get_or_insert_with(|| {
                        format!("'{}' is not a valid cron expression: {}", expr, e)
                    });
                }
            }
        }
        first_problem
    }

    pub fn trigger_type_label(&self) -> &'static str {
        let has_cron = !self.schedule.is_empty();
        let has_event = !self.on.is_empty();
        match (has_cron, has_event) {
            (true, true) => "Hybrid",
            (false, true) => "Event",
            _ => "Schedule",
        }
    }

    pub fn to_created_payload(&self) -> Value {
        serde_json::json!({
            "trigger_id": self.id,
            "name": self.name,
            "slug": self.slug,
            "schedule": self.schedule,
            "timezone": self.timezone,
            "run": self.run,
            "on": self.on,
            "paused": self.paused,
            "app_id": self.app_id,
            "go_to_review": self.go_to_review,
            "group_id": self.group_id,
            "side_effect_grant": self.side_effect_grant,
            "program_grants": self.program_grants,
            "folder_grants": self.folder_grants,
            "program_denies": self.program_denies,
            "folder_denies": self.folder_denies,
            "coding_agent_grant": self.coding_agent_grant,
            "context_ceiling": self.context_ceiling,
            "mcp_servers": self.mcp_servers,
            "plugin_id": self.plugin_id,
            "model": self.model,
            "reasoning_effort": self.reasoning_effort,
            "provider": self.provider,
            "continue_key": self.continue_key,
            ACTOR_FIELD_KEY: self.actor,
        })
    }

    pub fn apply_update(&mut self, payload: &Value) {
        if let Some(name) = payload["name"].as_str() {
            self.name = name.to_string();
        }
        if let Some(slug) = payload.get("slug").and_then(|v| v.as_str()) {
            if is_valid_trigger_slug(slug) {
                self.slug = slug.to_string();
            } else {
                log_legacy!(
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
        if payload.get("on").is_some() {
            self.on = parse_event_subscriptions(payload.get("on"));
        }
        if let Some(paused) = read_paused_field(payload) {
            self.paused = paused;
        }
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
        if let Some(v) = payload.get("group_id") {
            if v.is_null() {
                self.group_id = None;
            } else if let Some(s) = v.as_str() {
                self.group_id = Some(s.to_string());
            }
        }
        // INVARIANT: side_effect_grant update: full replacement when present (array → the
        // new grant set, `[]` / null → clears it); absent leaves it as-is. The
        // HTTP layer always sends the whole array on a change, mirroring `on`.
        if payload.get("side_effect_grant").is_some() {
            self.side_effect_grant = parse_side_effect_grant(payload.get("side_effect_grant"));
            self.program_grants.retain(|grant| {
                grant
                    .legacy
                    .as_ref()
                    .is_none_or(|legacy| self.side_effect_grant.contains(&legacy.category))
            });
        }
        if let Some(grants) = payload.get("folder_grants") {
            self.folder_grants = serde_json::from_value(grants.clone()).unwrap_or_default();
        }
        if payload.get("program_grants").is_some() {
            self.program_grants = parse_program_grants(payload);
        }
        if let Some(revoked) = payload
            .get("revoke_program_grants")
            .and_then(Value::as_array)
        {
            self.program_grants.retain(|grant| {
                !revoked
                    .iter()
                    .any(|pattern| pattern.as_str() == Some(grant.pattern.as_str()))
            });
        }
        if let Some(revoked) = payload
            .get("revoke_folder_grants")
            .and_then(Value::as_array)
        {
            self.folder_grants.retain(|grant| {
                let folder = grant.folder.to_string_lossy();
                !revoked
                    .iter()
                    .any(|listed| listed.as_str() == Some(&*folder))
            });
        }
        let denies_set =
            payload.get("program_denies").is_some() || payload.get("folder_denies").is_some();
        if payload.get("program_denies").is_some() {
            self.program_denies = parse_program_denies(payload);
        }
        if payload.get("folder_denies").is_some() {
            self.folder_denies = parse_folder_denies(payload);
        }
        if let Some(revoked) = listed(payload, "revoke_program_denies") {
            self.program_denies
                .retain(|denied| !revoked.contains(&denied.as_str()));
        }
        if let Some(revoked) = listed(payload, "revoke_folder_denies") {
            self.folder_denies
                .retain(|denied| !revoked.contains(&&*denied.to_string_lossy()));
        }
        self.settle_denies(denies_set);
        if let Some(v) = payload.get("coding_agent_grant").and_then(|v| v.as_bool()) {
            self.coding_agent_grant = v;
        }
        if payload.get("context_ceiling").is_some() {
            self.context_ceiling = parse_context_ceiling(payload.get("context_ceiling"), &self.id);
        }
        if payload.get("mcp_servers").is_some() {
            self.mcp_servers = parse_mcp_servers(payload.get("mcp_servers"));
        }
        if payload.get("continue_key").is_some() {
            self.continue_key = ContinueKeySpec::from_stored(payload.get("continue_key"), &self.id);
        }
        if payload.get(ACTOR_FIELD_KEY).is_some() {
            self.actor = ActorField::from_stored(payload.get(ACTOR_FIELD_KEY), &self.id);
        }
        if let Some(v) = payload.get("model") {
            if v.is_null() {
                self.model = None;
            } else if v.is_string() {
                self.model = read_trimmed_string(payload, "model");
            }
        }
        if let Some(v) = payload.get("provider") {
            if v.is_null() {
                self.provider = None;
            } else if v.is_string() {
                self.provider =
                    read_trimmed_string(payload, "provider").filter(|p| is_valid_provider(p));
            }
        }
        if let Some(v) = payload.get("reasoning_effort") {
            if v.is_null() {
                self.reasoning_effort = None;
            } else if v.is_string() {
                match read_trimmed_string(payload, "reasoning_effort") {
                    Some(e) if !is_valid_reasoning_effort(&e) => log_legacy!(
                        "[Triggers] Ignored invalid reasoning_effort '{}' in TriggerUpdated for {}",
                        e,
                        self.id
                    ),
                    other => self.reasoning_effort = other,
                }
            }
        }
        if let Some(s) = payload.get("plugin_id").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                self.plugin_id = Some(s.to_string());
            }
        }
    }
}

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

pub fn is_valid_trigger_slug(slug: &str) -> bool {
    let len = slug.len();
    if !(1..=MAX_TRIGGER_SLUG_LEN).contains(&len) {
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

fn is_path_safe_trigger_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 255
        && slug != "."
        && slug != ".."
        && !slug.contains(['/', '\\', '\0'])
}

fn read_paused_field(payload: &Value) -> Option<bool> {
    payload
        .get("paused")
        .and_then(|v| v.as_bool())
        .or_else(|| payload.get("enabled").and_then(|v| v.as_bool()).map(|e| !e))
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
