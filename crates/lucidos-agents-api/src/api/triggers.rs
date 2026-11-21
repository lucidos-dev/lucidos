use super::owner_door::unattested;
use super::*;

use crate::core::prefs;
use crate::engine::command_guard::SideEffectCategory;
use crate::engine::context_ceiling::ContextCeiling;
use crate::engine::tools::scheduler::validate_trigger_run;
use crate::engine::trigger_writes::{RequestedTriggerGrants, TriggerWrite};
use crate::triggers::{
    is_valid_trigger_slug, normalize_mcp_servers, normalize_route_setting,
    resolve_trigger_provider_update, validate_trigger_provider, validate_trigger_reasoning_effort,
    ActorField, ContinueKeySpec, EventSubscription, TriggerConfig, TriggerRun, TriggerRunStatus,
    ACTOR_FIELD_KEY,
};

#[derive(Serialize)]
pub struct TriggerInfo {
    pub id: String,
    pub name: String,
    /// Stable kebab-case slug. Directory segment for per-trigger know-how at
    /// `data/triggers/{slug}/knowhow/`.
    pub slug: String,
    pub cron_expressions: Vec<String>,
    pub timezone: String,
    pub paused: bool,
    pub last_run: Option<String>,
    /// Outcome of the most recent completed firing (`ok` / `failed`), surfaced
    /// on the trigger row. Absent until the trigger has run at least once under
    /// an engine that records status (legacy runs → timestamp only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_status: Option<TriggerRunStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<String>,
    /// The next few upcoming fire times (RFC3339, UTC), merged across every cron
    /// expression. `next_run` is its first entry; the panel shows the rest so a
    /// wrong schedule is visible before it costs anything (a "monthly" trigger
    /// listing three dates a year apart gives itself away).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_runs: Vec<String>,
    /// Set when the trigger's cron schedule can never fire, e.g. `0 0 9 31 2 *`.
    /// Renders as an error on the row instead of the "No more runs" chip a spent
    /// one-shot gets. Only reachable for triggers stored before the create /
    /// update guard existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_error: Option<String>,
    pub run: serde_json::Value,
    /// Event subscriptions. Empty for schedule-only triggers; each entry pairs
    /// an event type with an optional per-entry payload filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<EventSubscription>,
    /// Owning app directory name (e.g. `"trigger-workflow"`), used to deep-link
    /// notifications back to the right app. None for standalone triggers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// When true, threads spawned by this trigger surface in REVIEW on
    /// completion instead of going straight to ARCHIVE.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub go_to_review: bool,
    /// Owning *trigger group* id (UUID string) used by the panel to sort the
    /// trigger under the right section. None places the trigger under the
    /// "Ungrouped" section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// Historical category grants. They confer no authority.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub side_effect_grant: Vec<SideEffectCategory>,
    pub program_grants: Vec<TriggerProgramGrantInfo>,
    /// Folders outside the workspace this trigger's subprocesses may write.
    pub folder_grants: Vec<TriggerFolderGrantInfo>,
    /// The trigger's **coding-agent grant**: whether its runs may start a
    /// coding-agent thread. Always present, so a client never reads a missing
    /// field as a grant.
    pub coding_agent_grant: bool,
    /// The context ceiling copied onto each fired thread (ADR 0629). Absent =
    /// the workspace preference `coding_agent_context_ceiling`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_ceiling: Option<ContextCeiling>,
    /// The MCP server ids this trigger's fires use (ADR 0430). Empty = none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
    /// Plugin provenance (ADR 0019) — the id of the *plugin* that auto-registered
    /// this trigger, or absent for a user-created one. Drives the Triggers
    /// panel's "from \<plugin\>" chip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    /// The **trigger model** this trigger's intent fires on. Absent = the
    /// account `trigger_model` preference (the "Default" option in the form).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Reasoning effort for this trigger's intent fires. Absent = the account's
    /// tier for the model it runs on, else that model's default effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// The backend the pinned model runs on. Absent = the model's own
    /// preferred provider, then its first configured route.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The continue key spec (ADR 0623). Absent = every fire starts a thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continue_key: Option<ContinueKeySpec>,
    /// The actor field (ADR 0623). Absent = fires credit nobody.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<ActorField>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggerProgramGrantInfo {
    pub pattern: String,
    pub migrated_invocation: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggerFolderGrantInfo {
    /// The path `revoke_folder_grants` takes back.
    pub folder: String,
}

impl TriggerInfo {
    fn from_config(config: &TriggerConfig) -> Self {
        let run = serde_json::to_value(&config.run).unwrap_or_else(|e| {
            log_legacy!(
                "[Triggers] Failed to serialize run for trigger '{}': {}",
                config.id,
                e
            );
            serde_json::Value::Null
        });
        // One pass over the schedule feeds both fields: `next_run` is just the
        // head of the preview, so the two can never disagree.
        let next_runs = config.next_runs(crate::scheduler::cron_rules::CRON_PREVIEW_COUNT);
        Self {
            id: config.id.clone(),
            name: config.name.clone(),
            slug: config.slug.clone(),
            cron_expressions: config.schedule.clone(),
            timezone: config.timezone.clone(),
            paused: config.paused,
            last_run: config.last_run.map(|t| t.to_rfc3339()),
            last_run_status: config.last_run_status,
            next_run: next_runs.first().map(|t| t.to_rfc3339()),
            next_runs: next_runs.iter().map(|t| t.to_rfc3339()).collect(),
            schedule_error: config.schedule_error(),
            run,
            on: config.on.clone(),
            // Surface the resolved (explicit-or-derived) app id so the frontend
            // matches what the engine will stamp on notifications from this trigger.
            app_id: config.owning_app_id(),
            go_to_review: config.go_to_review,
            group_id: config.group_id.clone(),
            side_effect_grant: config.side_effect_grant.clone(),
            program_grants: config
                .program_grants
                .iter()
                .filter(|grant| grant.legacy.is_none())
                .map(|grant| TriggerProgramGrantInfo {
                    pattern: grant.pattern.clone(),
                    migrated_invocation: false,
                })
                .collect(),
            folder_grants: config
                .folder_grants
                .iter()
                .map(|grant| TriggerFolderGrantInfo {
                    folder: grant.folder.to_string_lossy().into_owned(),
                })
                .collect(),
            coding_agent_grant: config.coding_agent_grant,
            context_ceiling: config.context_ceiling,
            mcp_servers: config.mcp_servers.clone(),
            plugin_id: config.plugin_id.clone(),
            model: config.model.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
            provider: config.provider.clone(),
            continue_key: config.continue_key.clone(),
            actor: config.actor.clone(),
        }
    }
}

#[derive(Serialize)]
pub struct TriggersListResponse {
    pub triggers: Vec<TriggerInfo>,
}

#[derive(Serialize)]
pub struct HistoricalTriggerInfo {
    pub id: String,
    /// Snapshot from the most recent thread spawned by this trigger. None when
    /// no `TriggerStarted` event ever carried a name (legacy data).
    pub name: Option<String>,
    /// `last_activity` of the most recent thread spawned by this trigger
    /// (RFC3339, UTC). Frontend uses it to disambiguate same-named entries.
    pub last_activity: String,
}

#[derive(Serialize)]
pub struct HistoricalTriggersResponse {
    pub triggers: Vec<HistoricalTriggerInfo>,
}

#[derive(Deserialize)]
pub struct CreateTriggerCronRequest {
    #[serde(flatten)]
    pub requested_grants: RequestedTriggerGrants,
    pub name: String,
    pub run: serde_json::Value,
    /// Stable kebab-case slug for the trigger. Directory segment for
    /// per-trigger know-how at `data/triggers/{slug}/knowhow/`. Optional —
    /// derived from `name` (with UUID fallback) when omitted.
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub cron_expressions: Vec<String>,
    /// Event subscriptions. Empty for schedule-only triggers; each entry pairs
    /// an event type with an optional per-entry payload filter (no `condition`
    /// = fire on every matching event).
    #[serde(default)]
    pub on: Vec<EventSubscription>,
    /// Owning app directory name (e.g. `"trigger-workflow"`). Stamped onto
    /// notifications emitted by this trigger so the popover can deep-link
    /// to the app. Optional; standalone triggers omit it.
    #[serde(default)]
    pub app_id: Option<String>,
    /// When true, threads spawned by this trigger surface in REVIEW on
    /// completion instead of going straight to ARCHIVE. Default false.
    #[serde(default)]
    pub go_to_review: bool,
    /// Optional *trigger group* id. Pure organizational label — the trigger
    /// fires identically regardless of group. The handler validates the id
    /// against the in-memory group registry and rejects unknown values.
    #[serde(default)]
    pub group_id: Option<String>,
    /// Retired input. A non-empty list is refused.
    #[serde(default)]
    pub side_effect_grant: Vec<SideEffectCategory>,
    /// The **coding-agent grant**. Default false. Only a person may set it:
    /// see [`refuse_unowned_grant`].
    #[serde(default)]
    pub coding_agent_grant: bool,
    /// The context ceiling copied onto each fired thread, read by
    /// `ContextCeiling::parse`. Omitted or null = the workspace preference. An
    /// agent may not set one above the workspace's.
    #[serde(default)]
    pub context_ceiling: Option<String>,
    /// The MCP server ids this trigger's fires use (ADR 0430). Only these
    /// servers' tools run silently in its threads. An id no server has yet is
    /// accepted, and each fire reports it.
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    /// The **trigger model** this trigger's intent fires on. Omitted, null, or
    /// blank = the account `trigger_model` preference. Not checked against the model registry:
    /// see [`normalize_route_setting`].
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning effort for this trigger's intent fires. Omitted, null, or blank
    /// = the account's tier for the model, else its default effort. Must be
    /// one of `none|low|medium|high|xhigh|max`; anything else is a 400.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// The backend the pinned model runs on, when it has more than one route.
    /// Omitted, null, or blank = the model's own preferred provider. Requires a
    /// model pin and must name one of that model's routes.
    #[serde(default)]
    pub provider: Option<String>,
    /// The continue key spec (ADR 0623), checked by [`ContinueKeySpec::parse`].
    /// Omitted, null or `[]` = every fire starts a new thread.
    #[serde(default)]
    pub continue_key: Option<serde_json::Value>,
    /// The actor field (ADR 0623), checked by [`ActorField::parse`]. Omitted,
    /// null or blank = fires credit nobody.
    #[serde(default)]
    pub actor: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct UpdateTriggerCronRequest {
    #[serde(flatten)]
    pub requested_grants: RequestedTriggerGrants,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub run: Option<serde_json::Value>,
    #[serde(default)]
    pub cron_expressions: Option<Vec<String>>,
    #[serde(default)]
    pub paused: Option<bool>,
    /// Full replacement for the event subscription list. None = field absent
    /// (don't change), Some(empty) = clear all subscriptions, Some(non-empty)
    /// = replace with the new set. Partial edits aren't supported — send the
    /// whole list.
    #[serde(default)]
    pub on: Option<Vec<EventSubscription>>,
    /// None = field absent (don't change), Some(None) = explicitly null (clear), Some(Some(v)) = set.
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub app_id: Option<Option<String>>,
    #[serde(default)]
    pub go_to_review: Option<bool>,
    /// Optional slug edit (e.g. trigger renamed). Validated against
    /// [`is_valid_trigger_slug`] — invalid values reject the request with 400.
    #[serde(default)]
    pub slug: Option<String>,
    /// None = field absent (don't change), Some(None) = explicitly null (clear),
    /// Some(Some(v)) = set. The handler validates `v` against the in-memory
    /// trigger-group registry.
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub group_id: Option<Option<String>>,
    /// Send an empty array to clear the historical categories.
    #[serde(default)]
    pub side_effect_grant: Option<Vec<SideEffectCategory>>,
    /// Revoke these program scopes.
    #[serde(default)]
    pub revoke_program_grants: Vec<String>,
    /// Revoke these folder grants, by the `folder` this trigger lists. Only
    /// the owner may.
    #[serde(default)]
    pub revoke_folder_grants: Vec<String>,
    /// None = field absent (don't change). Only a person may set `true`: see
    /// [`refuse_unowned_grant`].
    #[serde(default)]
    pub coding_agent_grant: Option<bool>,
    /// None = field absent (don't change), Some(None) = explicitly null (back to
    /// the workspace preference), Some(Some(v)) = set, checked like create.
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub context_ceiling: Option<Option<String>>,
    /// Full replacement for the MCP server ids. None = field absent (don't
    /// change), Some(empty) = clear, Some(non-empty) = replace.
    #[serde(default)]
    pub mcp_servers: Option<Vec<String>>,
    /// None = field absent (don't change), Some(None) = explicitly null (back to
    /// the account `trigger_model` preference), Some(Some(v)) = set. A blank or
    /// `"null"` string clears too, per [`normalize_route_setting`].
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub model: Option<Option<String>>,
    /// Same triple state as [`Self::model`], validated against
    /// `none|low|medium|high|xhigh|max`.
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub reasoning_effort: Option<Option<String>>,
    /// Same triple state as [`Self::model`]. A change of model without one
    /// clears the old pin: see [`resolve_trigger_provider_update`].
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub provider: Option<Option<String>>,
    /// Full replacement for the continue key spec. None = field absent (don't
    /// change), null or `[]` = clear, a spec = replace.
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub continue_key: Option<Option<serde_json::Value>>,
    /// Same triple state as [`Self::continue_key`], for the actor field.
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub actor: Option<Option<serde_json::Value>>,
}

/// Check a submitted context ceiling. Absent or null reads as `None`, the
/// workspace preference.
pub(crate) fn parse_context_ceiling_arg(
    raw: Option<&str>,
) -> Result<Option<ContextCeiling>, String> {
    raw.map(ContextCeiling::parse).transpose()
}

/// Validate a slug submitted in a `TriggerUpdated` request. Trims whitespace
/// then runs [`is_valid_trigger_slug`]; returns the trimmed value on success.
/// Pure helper, exposed for unit tests.
pub(crate) fn validate_update_slug(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if !is_valid_trigger_slug(trimmed) {
        return Err(format!(
            "Invalid slug '{}': must be 1-64 chars of [a-z0-9-], starting and ending with [a-z0-9]",
            trimmed
        ));
    }
    Ok(trimmed.to_string())
}

/// Validate an explicit slug submitted with a create, if there is one.
///
/// `None` means the caller left it out, and the writer mints one under the
/// lock that serializes the emit. Minting here instead would read the taken
/// set before the emit, so two concurrent creates of one name could both pick
/// the same `data/triggers/<slug>/` directory.
///
/// An explicit slug is taken as given, even when it collides. The caller named
/// the directory they mean to write to, and renaming it silently would point
/// them at a different one.
pub(crate) fn validate_explicit_create_slug(
    explicit: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(s) = explicit.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if !is_valid_trigger_slug(s) {
        return Err(format!(
            "Invalid slug '{}': must be 1-64 chars of [a-z0-9-], starting and ending with [a-z0-9]",
            s
        ));
    }
    Ok(Some(s.to_string()))
}

#[derive(Deserialize)]
pub(super) struct TriggerIdQuery {
    id: String,
}

/// List all triggers from scheduler's in-memory state (event-sourced).
pub(super) async fn list_triggers(State(state): State<AppState>) -> Json<TriggersListResponse> {
    let configs = state.scheduler.lock().await.list_trigger_configs();
    let triggers: Vec<TriggerInfo> = configs.iter().map(TriggerInfo::from_config).collect();
    Json(TriggersListResponse { triggers })
}

/// Every trigger that has ever spawned a thread, deleted or live.
pub(super) async fn list_historical_triggers(
    State(state): State<AppState>,
) -> Result<Json<HistoricalTriggersResponse>, (StatusCode, String)> {
    let rows = state
        .engine
        .event_store()
        .list_historical_triggers()
        .await
        .map_err(|e| {
            log_legacy!("[API] Failed to list historical triggers: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to list historical triggers: {}", e),
            )
        })?;
    let triggers = rows
        .into_iter()
        .map(|(id, name, last_activity)| HistoricalTriggerInfo {
            id,
            name,
            last_activity: last_activity.to_rfc3339(),
        })
        .collect();
    Ok(Json(HistoricalTriggersResponse { triggers }))
}

/// An agent or an app frame: a caller that may not widen a trigger's grants.
fn is_unowned_caller(headers: &HeaderMap) -> bool {
    let origin = crate::api::actor::user_actor(headers, None);
    super::intelligence_ladder::is_agent_caller(headers, origin.as_ref())
        || headers.contains_key(super::app_reach::APP_ID_HEADER)
}

/// Refuse a write that `widens` a trigger's grants from anyone but a person.
/// The owner decides what each trigger may do unattended (ADR 0641).
/// Any caller may narrow or keep the current grant.
pub(super) fn refuse_unowned_grant(headers: &HeaderMap, widens: bool) -> Result<(), ApiError> {
    if !widens {
        return Ok(());
    }
    if is_unowned_caller(headers) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Only the owner can widen a trigger's grants. Ask the owner to turn on 'Allow coding \
             agents' or answer the program grant request. Restoring older work on a granted \
             trigger needs the owner too.",
        ));
    }
    if crate::api::actor::owner_attestation(headers)
        == crate::api::actor::OwnerAttestation::Unattested
    {
        return Err(unattested());
    }
    Ok(())
}

/// Refuse a folder grant revoke from anyone but the owner. The owner decided
/// where this trigger may write, so only the owner takes it back.
fn refuse_unowned_folder_revoke(headers: &HeaderMap) -> Result<(), ApiError> {
    if is_unowned_caller(headers) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Only the owner can revoke a trigger's folder grants.",
        ));
    }
    if crate::api::actor::owner_attestation(headers)
        == crate::api::actor::OwnerAttestation::Unattested
    {
        return Err(unattested());
    }
    Ok(())
}

/// Refuse an agent's trigger model pick off the chat intelligence ladder,
/// judged as the trigger will fire on it. The owner's pick passes (ADR 0525).
async fn refuse_agent_chat_pick(
    state: &AppState,
    headers: &HeaderMap,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<(), String> {
    let origin = crate::api::actor::user_actor(headers, None);
    if !super::intelligence_ladder::is_agent_caller(headers, origin.as_ref()) {
        return Ok(());
    }
    crate::runtime::intelligence_ladder::check_agent_pick(
        &state.pool,
        &state.engine.model_routes(),
        crate::runtime::intelligence_ladder::LadderSurface::Chat,
        model,
        effort,
    )
    .await
}

fn trigger_grant_host(headers: &HeaderMap) -> Option<uuid::Uuid> {
    super::permission_requests::calling_agent_thread(headers).ok()
}

/// Create a new trigger
pub(super) async fn create_trigger(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateTriggerCronRequest>,
) -> Result<Json<ApiResult>, ApiError> {
    // Validate name
    let name = request.name.trim();
    if name.is_empty() {
        return Ok(ApiResult::err("Trigger name is required"));
    }

    // Validate run field
    let run: TriggerRun = match serde_json::from_value(request.run.clone()) {
        Ok(r) => r,
        Err(e) => return Ok(ApiResult::err(format!("Invalid 'run' field: {}", e))),
    };

    if let Err(e) = validate_trigger_run(&run) {
        return Ok(ApiResult::err(e));
    }

    let has_cron = !request.cron_expressions.is_empty();
    let subscriptions = EventSubscription::normalize_list(request.on);
    let has_event = !subscriptions.is_empty();
    if !has_cron && !has_event {
        return Ok(ApiResult::err(
            "At least one cron expression or an event subscription is required",
        ));
    }

    // Every entry in the `on` array. A trigger armed on a name the engine never
    // emits looks armed and never fires.
    let mut event_warnings = match crate::core::event_subscription::check_subscriptions(
        &state.pool,
        &subscriptions,
        crate::core::event_subscription::SubscriptionSurface::Trigger,
    )
    .await
    {
        Ok(warnings) => warnings,
        Err(msg) => return Ok(ApiResult::err(msg)),
    };

    // Read timezone from preferences before validating cron: the guard reports its
    // next-run preview in the trigger's own timezone. A missing preference defaults
    // to UTC; a read failure surfaces instead of silently scheduling the wrong zone.
    let timezone = match prefs::TIMEZONE.try_stored(&state.pool).await {
        Ok(Some(tz)) => tz,
        Ok(None) => crate::triggers::definition::default_timezone(),
        Err(e) => {
            return Ok(ApiResult::err(format!(
                "Failed to read timezone preference: {}",
                e
            )))
        }
    };

    // Validate cron expressions if provided. This rejects a schedule that can
    // never fire (Feb 31 and friends), not just a malformed one.
    let validated = match crate::scheduler::cron_rules::validate_cron_expressions(
        request
            .cron_expressions
            .iter()
            .map(|s| s.trim().to_string())
            .collect(),
        crate::scheduler::cron_rules::cron_tz_or_utc(&timezone, "POST /triggers"),
    ) {
        Ok(v) => v,
        Err(e) => return Ok(ApiResult::err(e)),
    };
    let cron_expressions = validated.expressions.clone();

    let run_value = match serde_json::to_value(&run) {
        Ok(v) => v,
        Err(e) => return Ok(ApiResult::err(format!("Failed to serialize 'run': {}", e))),
    };
    let trigger_id_str = Uuid::new_v4().to_string();

    // An EXPLICIT slug is validated here and rides in the payload. An omitted
    // one is minted by the writer, under the lock that serializes the emit.
    // Two concurrent creates of one name cannot then mint the same directory.
    let explicit_slug = match validate_explicit_create_slug(request.slug.as_deref()) {
        Ok(s) => s,
        Err(e) => return Ok(ApiResult::err(e)),
    };

    let mut payload = serde_json::json!({
        "trigger_id": trigger_id_str,
        "name": name,
        "schedule": cron_expressions,
        "timezone": timezone,
        "run": run_value,
    });
    if let Some(slug) = explicit_slug {
        payload["slug"] = serde_json::json!(slug);
    }
    if !subscriptions.is_empty() {
        payload["on"] = serde_json::to_value(&subscriptions)
            .expect("EventSubscription serialization is infallible");
    }
    if let Some(ref aid) = request.app_id {
        let trimmed = aid.trim();
        if !trimmed.is_empty() {
            payload["app_id"] = serde_json::json!(trimmed);
        }
    }
    if request.go_to_review {
        payload["go_to_review"] = serde_json::json!(true);
    }
    // Validate trigger-group membership against the in-memory registry so a
    // dangling group_id can't survive the round-trip. Unknown id → 400.
    if let Some(ref gid) = request.group_id {
        let trimmed = gid.trim();
        if !trimmed.is_empty() {
            let known = state
                .engine
                .trigger_groups
                .read()
                .unwrap()
                .contains_key(trimmed);
            if !known {
                return Ok(ApiResult::err(format!("Unknown group_id '{}'", trimmed)));
            }
            payload["group_id"] = serde_json::json!(trimmed);
        }
    }
    let widens = request.coding_agent_grant || !request.side_effect_grant.is_empty();
    refuse_unowned_grant(&headers, widens)?;
    if !request.side_effect_grant.is_empty() {
        return Ok(ApiResult::err(
            "Category grants are retired. Use the trigger's program grant permission request.",
        ));
    }
    if request.coding_agent_grant {
        payload["coding_agent_grant"] = serde_json::json!(true);
    }
    let ceiling = match parse_context_ceiling_arg(request.context_ceiling.as_deref()) {
        Ok(ceiling) => ceiling,
        Err(e) => return Ok(ApiResult::err(e)),
    };
    if let Err(e) =
        super::threads::context_ceiling::refuse_agent_trigger_raise(&state, &headers, ceiling).await
    {
        return Ok(ApiResult::err(e));
    }
    if let Some(ceiling) = ceiling {
        payload["context_ceiling"] = serde_json::json!(ceiling);
    }
    let mcp_servers = normalize_mcp_servers(request.mcp_servers.iter().map(String::as_str));
    if !mcp_servers.is_empty() {
        payload["mcp_servers"] = serde_json::json!(mcp_servers);
    }
    let continue_key = match ContinueKeySpec::parse(request.continue_key.as_ref())
        .and_then(|spec| ContinueKeySpec::check_run(spec.as_ref(), &run).map(|()| spec))
    {
        Ok(spec) => spec,
        Err(e) => return Ok(ApiResult::err(e)),
    };
    if let Some(spec) = continue_key {
        payload["continue_key"] = serde_json::json!(spec);
    }
    match ActorField::parse(request.actor.as_ref())
        .and_then(|field| ActorField::check_run(field.as_ref(), &run).map(|()| field))
    {
        Ok(Some(field)) => payload[ACTOR_FIELD_KEY] = serde_json::json!(field),
        Ok(None) => {}
        Err(e) => return Ok(ApiResult::err(e)),
    }
    // Model / reasoning effort: only stamped when the user picked something
    // other than Default, so a trigger on the account defaults keeps exactly the
    // payload it had before the fields existed.
    if let Some(model) = normalize_route_setting(request.model.as_deref()) {
        payload["model"] = serde_json::json!(model);
    }
    match validate_trigger_reasoning_effort(request.reasoning_effort.as_deref()) {
        Ok(Some(effort)) => payload["reasoning_effort"] = serde_json::json!(effort),
        Ok(None) => {}
        Err(e) => return Ok(ApiResult::err(e)),
    }
    match validate_trigger_provider(
        state.engine.model_registry(),
        request.model.as_deref(),
        request.provider.as_deref(),
    ) {
        Ok(Some(provider)) => payload["provider"] = serde_json::json!(provider),
        Ok(None) => {}
        Err(e) => return Ok(ApiResult::err(e)),
    }
    if let Err(e) = refuse_agent_chat_pick(
        &state,
        &headers,
        payload["model"].as_str(),
        payload["reasoning_effort"].as_str(),
    )
    .await
    {
        return Ok(ApiResult::err(e));
    }

    // Through the trigger write chokepoint: the registry must hold the new
    // trigger before this 200 lands, or the client's next
    // `GET /api/v1/triggers` can miss what it just created.
    let actor = match crate::api::actor::require_user_actor(&headers, &state.pool, None).await {
        Ok(a) => Some(a),
        Err(e) => return Ok(ApiResult::err(e.message)),
    };
    match state
        .engine
        .trigger_registry_writer()
        .write_with_requested_grants(
            TriggerWrite::Created,
            &trigger_id_str,
            payload,
            actor,
            request.requested_grants,
            trigger_grant_host(&headers),
        )
        .await
    {
        Ok(true) => {
            event_warnings.push("Trigger grants await the owner's answer in this thread.".into())
        }
        Ok(false) => {}
        Err(e) => {
            log_legacy!("[Triggers] TriggerCreated emit failed: {e}");
            return Ok(ApiResult::err(format!("Failed to create trigger: {e}")));
        }
    }

    Ok(ApiResult::ok_for_trigger(
        Some(CronPreview::from_validated(&validated)),
        event_warnings,
    ))
}

/// Update an existing trigger
pub(super) async fn update_trigger(
    State(state): State<AppState>,
    Query(query): Query<TriggerIdQuery>,
    headers: HeaderMap,
    Json(request): Json<UpdateTriggerCronRequest>,
) -> Result<Json<ApiResult>, ApiError> {
    let task_id = query.id;

    // Fetch existing trigger from in-memory state
    let existing = match state.scheduler.lock().await.get_trigger_config(&task_id) {
        Some(c) => c,
        None => return Ok(ApiResult::err(format!("Trigger '{}' not found", task_id))),
    };

    // Validate cron expressions if provided, against the trigger's own timezone
    // (an update cannot change it). This rejects a schedule that can never fire
    // (Feb 31 and friends), not just a malformed one.
    let validated = match request.cron_expressions {
        Some(ref exprs) => match crate::scheduler::cron_rules::validate_cron_expressions(
            exprs.iter().map(|s| s.trim().to_string()).collect(),
            crate::scheduler::cron_rules::cron_tz_or_utc(&existing.timezone, "PUT /triggers"),
        ) {
            Ok(v) => Some(v),
            Err(e) => return Ok(ApiResult::err(e)),
        },
        None => None,
    };

    // Validate run field if changing
    if let Some(ref run_val) = request.run {
        match serde_json::from_value::<TriggerRun>(run_val.clone()) {
            Ok(parsed_run) => {
                if let Err(e) = validate_trigger_run(&parsed_run) {
                    return Ok(ApiResult::err(e));
                }
            }
            Err(_) => return Ok(ApiResult::err("Invalid 'run' field")),
        }
    }

    // Build update payload with only changed fields
    let trigger_id_str = task_id.clone();
    let mut update_payload = serde_json::json!({ "trigger_id": trigger_id_str });

    if let Some(ref n) = request.name {
        update_payload["name"] = serde_json::json!(n.trim());
    }
    if let Some(ref v) = validated {
        update_payload["schedule"] = serde_json::json!(v.expressions);
    }
    if let Some(ref run_val) = request.run {
        if let Ok(run) = serde_json::from_value::<TriggerRun>(run_val.clone()) {
            match serde_json::to_value(&run) {
                Ok(v) => {
                    update_payload["run"] = v;
                }
                Err(e) => return Ok(ApiResult::err(format!("Failed to serialize 'run': {}", e))),
            }
        }
    }
    if let Some(paused) = request.paused {
        update_payload["paused"] = serde_json::json!(paused);
    }

    let normalized_on: Option<Vec<EventSubscription>> =
        request.on.map(EventSubscription::normalize_list);

    // Only what this request supplies. An `on:` list left absent keeps whatever
    // the trigger already had, and re-refusing it would strand a trigger the
    // user can no longer rename.
    let mut event_warnings = match crate::core::event_subscription::check_subscriptions(
        &state.pool,
        normalized_on.as_deref().unwrap_or_default(),
        crate::core::event_subscription::SubscriptionSurface::Trigger,
    )
    .await
    {
        Ok(warnings) => warnings,
        Err(msg) => return Ok(ApiResult::err(msg)),
    };

    // None = absent (keep existing); Some(empty) = clear all; Some(non-empty) = replace.
    // The new shape always serializes as an array — apply_update reads it back
    // via parse_event_subscriptions and treats `[]` as clear.
    if let Some(ref subs) = normalized_on {
        update_payload["on"] =
            serde_json::to_value(subs).expect("EventSubscription serialization is infallible");
    }
    // Same null-vs-absent semantics for app_id: explicit null clears the link
    // (e.g. trigger moved out of an app), absent leaves it alone.
    if let Some(v) = &request.app_id {
        let normalized = v
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        update_payload["app_id"] = serde_json::json!(normalized);
    }
    if let Some(v) = request.go_to_review {
        update_payload["go_to_review"] = serde_json::json!(v);
    }
    if let Some(slug_raw) = request.slug.as_ref() {
        match validate_update_slug(slug_raw) {
            Ok(slug) => update_payload["slug"] = serde_json::json!(slug),
            Err(e) => return Ok(ApiResult::err(e)),
        }
    }
    // group_id update: same null-vs-absent semantics as app_id. Setting a value
    // validates against the in-memory registry; null clears membership; absent
    // leaves the trigger in its current group.
    if let Some(v) = &request.group_id {
        let normalized = v
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(ref gid) = normalized {
            let known = state
                .engine
                .trigger_groups
                .read()
                .unwrap()
                .contains_key(gid);
            if !known {
                return Ok(ApiResult::err(format!("Unknown group_id '{}'", gid)));
            }
        }
        update_payload["group_id"] = serde_json::json!(normalized);
    }
    // The compatibility API can revoke old categories. Only the empty
    // replacement is accepted, so a stale update cannot restore one.
    if let Some(ref grant) = request.side_effect_grant {
        if !grant.is_empty() {
            return Ok(ApiResult::err(
                "Category grants are retired. To clear historical categories, send an empty list.",
            ));
        }
        update_payload["side_effect_grant"] = serde_json::json!([]);
    }
    if !request.revoke_program_grants.is_empty() {
        update_payload["revoke_program_grants"] = serde_json::json!(request.revoke_program_grants);
    }
    if !request.revoke_folder_grants.is_empty() {
        refuse_unowned_folder_revoke(&headers)?;
        update_payload["revoke_folder_grants"] = serde_json::json!(request.revoke_folder_grants);
    }
    if let Some(grant) = request.coding_agent_grant {
        update_payload["coding_agent_grant"] = serde_json::json!(grant);
    }
    if let Some(raw) = &request.context_ceiling {
        let ceiling = match parse_context_ceiling_arg(raw.as_deref()) {
            Ok(ceiling) => ceiling,
            Err(e) => return Ok(ApiResult::err(e)),
        };
        // Clearing back to the workspace's is never a raise past it.
        if let Err(e) =
            super::threads::context_ceiling::refuse_agent_trigger_raise(&state, &headers, ceiling)
                .await
        {
            return Ok(ApiResult::err(e));
        }
        update_payload["context_ceiling"] = serde_json::json!(ceiling);
    }
    if let Some(ref ids) = request.mcp_servers {
        update_payload["mcp_servers"] =
            serde_json::json!(normalize_mcp_servers(ids.iter().map(String::as_str)));
    }
    if let Some(raw) = &request.continue_key {
        match ContinueKeySpec::parse(raw.as_ref()) {
            Ok(spec) => update_payload["continue_key"] = serde_json::json!(spec),
            Err(e) => return Ok(ApiResult::err(e)),
        }
    }
    if let Some(raw) = &request.actor {
        match ActorField::parse(raw.as_ref()) {
            Ok(field) => update_payload[ACTOR_FIELD_KEY] = serde_json::json!(field),
            Err(e) => return Ok(ApiResult::err(e)),
        }
    }
    // Model / reasoning effort: same null-vs-absent semantics as app_id, so a
    // rename-only save leaves the trigger's model alone while an explicit null
    // (or the form's blank Default) clears it back to the account preference.
    if let Some(v) = &request.model {
        update_payload["model"] = serde_json::json!(normalize_route_setting(v.as_deref()));
    }
    if let Some(v) = &request.reasoning_effort {
        match validate_trigger_reasoning_effort(v.as_deref()) {
            Ok(normalized) => update_payload["reasoning_effort"] = serde_json::json!(normalized),
            Err(e) => return Ok(ApiResult::err(e)),
        }
    }
    if request.model.is_some() || request.reasoning_effort.is_some() {
        let model = match &request.model {
            Some(v) => normalize_route_setting(v.as_deref()),
            None => existing.model.clone(),
        };
        let effort = match &request.reasoning_effort {
            Some(v) => normalize_route_setting(v.as_deref()),
            None => existing.reasoning_effort.clone(),
        };
        if let Err(e) =
            refuse_agent_chat_pick(&state, &headers, model.as_deref(), effort.as_deref()).await
        {
            return Ok(ApiResult::err(e));
        }
    }
    match resolve_trigger_provider_update(
        state.engine.model_registry(),
        &existing,
        request.model.as_ref().map(Option::as_deref),
        request.provider.as_ref().map(Option::as_deref),
    ) {
        Ok(Some(provider)) => update_payload["provider"] = serde_json::json!(provider),
        Ok(None) => {}
        Err(e) => return Ok(ApiResult::err(e)),
    }

    // Ensure trigger still has at least one firing mechanism after update
    let updated_crons = validated
        .as_ref()
        .map(|v| &v.expressions)
        .unwrap_or(&existing.schedule);
    let updated_on = normalized_on.as_ref().unwrap_or(&existing.on);
    if updated_crons.is_empty() && updated_on.is_empty() {
        return Ok(ApiResult::err(
            "Trigger must have at least one cron expression or an event subscription",
        ));
    }
    let mut next = existing.clone();
    next.apply_update(&update_payload);
    if let Err(e) = ContinueKeySpec::check_run(next.continue_key.as_ref(), &next.run)
        .and_then(|()| ActorField::check_run(next.actor.as_ref(), &next.run))
    {
        return Ok(ApiResult::err(e));
    }
    refuse_unowned_grant(&headers, existing.widens_a_grant(&next))?;
    let edits_granted = existing.unowned_edit_clears_grants(&next);
    // Editing work that runs with existing grants requires attestation.
    if edits_granted
        && crate::api::actor::owner_attestation(&headers)
            == crate::api::actor::OwnerAttestation::Unattested
        && !is_unowned_caller(&headers)
    {
        return Err(unattested());
    }
    let clears_grants = edits_granted && is_unowned_caller(&headers);
    if clears_grants {
        TriggerConfig::clear_grants_in(&mut update_payload);
        event_warnings.push(existing.grants_cleared_notice());
    }

    // Read-your-writes: the pause this request carries has to be visible to the
    // very next request. `POST /api/v1/triggers/run` reads the registry to
    // decide whether to refuse, so a subscriber-only apply lets a trigger the
    // user just paused take a real off-schedule fire.
    let actor = crate::api::actor::user_actor(&headers, None);
    match state
        .engine
        .trigger_registry_writer()
        .write_with_requested_grants(
            TriggerWrite::Updated,
            &trigger_id_str,
            update_payload,
            actor,
            request.requested_grants,
            trigger_grant_host(&headers),
        )
        .await
    {
        Ok(true) => {
            event_warnings.push("Trigger grants await the owner's answer in this thread.".into())
        }
        Ok(false) => {}
        Err(e) => {
            log_legacy!("[Triggers] TriggerUpdated emit failed: {e}");
            return Ok(ApiResult::err(format!("Failed to update trigger: {e}")));
        }
    }
    if clears_grants {
        crate::scheduler::user_tasks::notify_trigger_grants_cleared(&state.engine, &existing).await;
    }

    // Only an update that actually rewrote the schedule has a preview to report;
    // one that only renamed the trigger says nothing about its cron.
    Ok(ApiResult::ok_for_trigger(
        validated.as_ref().map(CronPreview::from_validated),
        event_warnings,
    ))
}

/// Delete a trigger
pub(super) async fn delete_trigger(
    State(state): State<AppState>,
    Query(query): Query<TriggerIdQuery>,
    headers: HeaderMap,
) -> Json<ApiResult> {
    let task_id = query.id;

    // Check trigger exists in in-memory state
    if state
        .scheduler
        .lock()
        .await
        .get_trigger_config(&task_id)
        .is_none()
    {
        return ApiResult::err(format!("Trigger '{}' not found", task_id));
    }

    let actor = crate::api::actor::user_actor(&headers, None);
    if let Err(e) = state
        .engine
        .emit_trigger_write(
            TriggerWrite::Deleted,
            &task_id,
            serde_json::json!({ "trigger_id": &task_id }),
            actor,
        )
        .await
    {
        log_legacy!("[Triggers] TriggerDeleted emit failed: {}", e);
        return ApiResult::err(format!("Failed to delete trigger: {e}"));
    }

    ApiResult::ok()
}

/// Result of `POST /api/v1/triggers/run`.
///
/// `success: true` with `status: "already-running"` is the honest answer when
/// admission coalesced the fire away: the request was valid and nothing new
/// started. A bare `{"success": true}` would read as "it started", which is the
/// class of lie this whole operation replaces.
#[derive(Serialize)]
pub struct TriggerRunResponse {
    pub success: bool,
    /// `started` | `queued` | `already-running`. Absent on a refusal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
    /// Human-readable summary; on a refusal this is the reason.
    pub message: String,
}

/// Fire an existing trigger once, right now, outside its schedule.
///
/// No actor is stamped, deliberately: an **off-schedule run** is
/// indistinguishable downstream from a scheduled fire, and an actor on the
/// resulting events would be exactly the tell that breaks that. The handler
/// emits no `SystemEvent` of its own either; the run's `TriggerExecuted` /
/// `TriggerCompleted` come from the queue executor like any other fire. See
/// `engine_impl::trigger_runs`, which owns every refusal so the LLM, CLI and
/// HTTP surfaces cannot drift.
pub(super) async fn run_trigger(
    State(state): State<AppState>,
    Query(query): Query<TriggerIdQuery>,
) -> Json<TriggerRunResponse> {
    Json(
        match state.engine.run_trigger_off_schedule(&query.id).await {
            Ok(outcome) => TriggerRunResponse {
                success: true,
                status: Some(outcome.status()),
                message: outcome.message(),
            },
            Err(refusal) => TriggerRunResponse {
                success: false,
                status: None,
                message: refusal.message(),
            },
        },
    )
}

/// Routes for the `/triggers*` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/triggers",
            get(list_triggers)
                .post(create_trigger)
                .put(update_trigger)
                .delete(delete_trigger),
        )
        .route("/triggers/historical", get(list_historical_triggers))
        .route("/triggers/run", post(run_trigger))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tools::scheduler::{parse_run_arg, validate_script_path};

    /// A secret read has no wire form, so a create or update naming it 4xxs.
    #[test]
    fn a_grant_naming_a_secret_read_is_refused() {
        let grant = serde_json::json!({ "side_effect_grant": ["secret_read"] });
        assert!(serde_json::from_value::<UpdateTriggerCronRequest>(grant.clone()).is_err());
        let mut create = serde_json::json!({
            "name": "T",
            "run": { "type": "intent", "intent": "x" },
            "cron_expressions": ["0 0 8 * * *"],
        });
        assert!(serde_json::from_value::<CreateTriggerCronRequest>(create.clone()).is_ok());
        create["side_effect_grant"] = grant["side_effect_grant"].clone();
        assert!(serde_json::from_value::<CreateTriggerCronRequest>(create).is_err());
    }

    fn headers_with(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    /// Only the attested owner widens a trigger's grants. An agent, an app
    /// frame or a typed device id may narrow them, or send back what the
    /// trigger holds.
    #[test]
    fn only_a_person_widens_a_grant() {
        use crate::core::subprocess_origin::{
            init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
            HEADER_DEVICE_ID,
        };
        init_agent_origin_secret("coding-agent-grant-secret".into());
        let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None).unwrap();
        let agent = headers_with(&[
            (HEADER_AGENT_ORIGIN_TOKEN, &token),
            (HEADER_DEVICE_ID, "owner-phone"),
        ]);
        let app = headers_with(&[
            (super::super::app_reach::APP_ID_HEADER, "habit-tracker"),
            (HEADER_DEVICE_ID, "owner-phone"),
        ]);
        let typed = headers_with(&[(HEADER_DEVICE_ID, "owner-phone")]);
        let owner = crate::api::gateway_stamp::test_support::stamped_device_headers("owner-phone");

        let held = TriggerConfig::from_created_payload(&serde_json::json!({
            "trigger_id": "t", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
            "run": { "type": "intent", "intent": "x" },
            "side_effect_grant": ["email"], "coding_agent_grant": true,
        }))
        .unwrap();
        let widens = |update: serde_json::Value| {
            let mut next = held.clone();
            next.apply_update(&update);
            held.widens_a_grant(&next)
        };
        let wider = [
            serde_json::json!({ "side_effect_grant": ["email", "cloud_cli"] }),
            serde_json::json!({ "side_effect_grant": ["external_api"] }),
        ];
        let same_or_narrower = [
            serde_json::json!({ "side_effect_grant": ["email"] }),
            serde_json::json!({ "side_effect_grant": [] }),
            serde_json::json!({ "coding_agent_grant": true }),
            serde_json::json!({ "coding_agent_grant": false }),
        ];
        for caller in [&agent, &app, &typed] {
            for update in &wider {
                assert!(
                    refuse_unowned_grant(caller, widens(update.clone())).is_err(),
                    "{update}"
                );
            }
            for update in &same_or_narrower {
                assert!(
                    refuse_unowned_grant(caller, widens(update.clone())).is_ok(),
                    "{update}"
                );
            }
        }
        for update in &wider {
            assert!(refuse_unowned_grant(&owner, widens(update.clone())).is_ok());
        }
    }

    // --- Script paths at the create / update boundary ---

    /// A traversal path used to pass create and update, because the extension
    /// check was the only one there. The trigger answered 200 with a cron
    /// preview, showed as armed, and every fire died on the execution guard.
    #[test]
    fn validate_script_path_rejects_a_traversal() {
        for path in [
            "../../evil.sh",
            "../evil.py",
            "/etc/cron/evil.sh",
            "\\windows\\evil.sh",
        ] {
            let err = validate_script_path(path).unwrap_err();
            assert!(err.contains("Invalid script path"), "{path}: {err}");
        }
    }

    /// The extension rule still runs, and an in-tree path still passes. The
    /// boundary refuses exactly what the fire-time guard refuses, no more.
    #[test]
    fn validate_script_path_keeps_the_extension_rule() {
        assert!(validate_script_path("scripts/daily-summary.sh").is_ok());
        assert!(validate_script_path("scripts/daily-summary.py").is_ok());
        let err = validate_script_path("scripts/daily-summary.txt").unwrap_err();
        assert!(err.contains("Unsupported script extension"), "{err}");
    }

    /// The `create_trigger` and `update_trigger` tools parse `run` through
    /// `parse_run_arg`. It once only deserialized, so a traversal path reached
    /// `TriggerCreated` from the agent side.
    #[test]
    fn the_trigger_tools_refuse_a_traversal_script_path() {
        for path in ["../x.sh", "/etc/x.sh"] {
            let err =
                parse_run_arg(&serde_json::json!({ "type": "script", "path": path })).unwrap_err();
            assert!(err.contains("Invalid script path"), "{path}: {err}");
        }
        assert!(
            parse_run_arg(&serde_json::json!({ "type": "script", "path": "scripts/x.sh" })).is_ok()
        );
        assert!(parse_run_arg(&serde_json::json!({ "type": "intent", "intent": "hi" })).is_ok());
    }

    // --- Slug resolution at the create boundary ---

    #[test]
    fn an_explicit_create_slug_is_validated_and_kept() {
        let slug = validate_explicit_create_slug(Some("send-daily-summary")).unwrap();
        assert_eq!(slug.as_deref(), Some("send-daily-summary"));
    }

    #[test]
    fn an_omitted_create_slug_leaves_the_mint_to_the_writer() {
        assert_eq!(validate_explicit_create_slug(None).unwrap(), None);
    }

    #[test]
    fn a_blank_create_slug_is_treated_as_omitted() {
        assert_eq!(validate_explicit_create_slug(Some("   ")).unwrap(), None);
    }

    #[test]
    fn an_invalid_explicit_create_slug_is_refused() {
        let err = validate_explicit_create_slug(Some("Bad Slug")).unwrap_err();
        assert!(err.contains("Invalid slug"));
    }

    #[test]
    fn a_second_trigger_of_the_same_name_gets_its_own_slug() {
        // Both would be `daily-summary`, which is one directory under
        // `data/triggers/`, so the second must not collide with the first.
        let taken: std::collections::HashSet<String> =
            ["daily-summary".to_string()].into_iter().collect();
        let slug = crate::triggers::mint_unique_trigger_slug("Daily Summary", "uuid-2", &taken);
        assert_eq!(slug, "daily-summary-2");
    }

    // --- Slug edits at the update boundary ---

    #[test]
    fn validate_update_slug_accepts_well_formed() {
        assert_eq!(
            validate_update_slug("renamed-trigger").unwrap(),
            "renamed-trigger"
        );
        // Trims whitespace.
        assert_eq!(validate_update_slug("  abc  ").unwrap(), "abc");
    }

    #[test]
    fn validate_update_slug_rejects_invalid() {
        let err = validate_update_slug("Has Spaces").unwrap_err();
        assert!(err.contains("Invalid slug"));
        let err = validate_update_slug("-leading").unwrap_err();
        assert!(err.contains("Invalid slug"));
    }

    // --- Request shape: slug field deserializes ---

    #[test]
    fn create_request_accepts_slug_field() {
        let req: CreateTriggerCronRequest = serde_json::from_value(serde_json::json!({
            "name": "Test",
            "run": { "type": "intent", "intent": "x" },
            "slug": "test-slug",
            "cron_expressions": ["0 0 8 * * *"],
        }))
        .unwrap();
        assert_eq!(req.slug.as_deref(), Some("test-slug"));
    }

    #[test]
    fn update_request_accepts_slug_field() {
        let req: UpdateTriggerCronRequest = serde_json::from_value(serde_json::json!({
            "slug": "renamed-trigger",
        }))
        .unwrap();
        assert_eq!(req.slug.as_deref(), Some("renamed-trigger"));
    }

    // --- Request shape: the provider pin ---

    #[test]
    fn create_request_accepts_a_provider_pin() {
        let req: CreateTriggerCronRequest = serde_json::from_value(serde_json::json!({
            "name": "Test",
            "run": { "type": "intent", "intent": "x" },
            "cron_expressions": ["0 0 8 * * *"],
            "model": "claude-opus-5",
            "provider": "anthropic",
        }))
        .unwrap();
        assert_eq!(req.provider.as_deref(), Some("anthropic"));
    }

    /// Absent keeps the pin and null clears it, so the two must not collapse.
    #[test]
    fn update_request_tells_an_absent_provider_from_a_null_one() {
        let absent: UpdateTriggerCronRequest =
            serde_json::from_value(serde_json::json!({ "name": "x" })).unwrap();
        assert_eq!(absent.provider, None);
        let null: UpdateTriggerCronRequest =
            serde_json::from_value(serde_json::json!({ "provider": null })).unwrap();
        assert_eq!(null.provider, Some(None));
        let set: UpdateTriggerCronRequest =
            serde_json::from_value(serde_json::json!({ "provider": "vertex" })).unwrap();
        assert_eq!(set.provider, Some(Some("vertex".to_string())));
    }

    #[test]
    fn trigger_info_serves_the_provider_pin() {
        let config = TriggerConfig::from_created_payload(&serde_json::json!({
            "trigger_id": "t", "name": "T", "schedule": [], "timezone": "UTC",
            "run": { "type": "intent", "intent": "x" },
            "on": [{ "event_type": "DayEnded" }],
            "model": "claude-opus-5", "provider": "anthropic",
        }))
        .unwrap();
        let info = serde_json::to_value(TriggerInfo::from_config(&config)).unwrap();
        assert_eq!(info["provider"], "anthropic");
    }

    #[test]
    fn only_the_owner_revokes_a_folder_grant() {
        use crate::core::subprocess_origin::{
            init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
            HEADER_DEVICE_ID,
        };
        init_agent_origin_secret("coding-agent-grant-secret".into());
        let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None).unwrap();
        let agent = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
        let app = headers_with(&[(super::super::app_reach::APP_ID_HEADER, "habit-tracker")]);
        let owner = crate::api::gateway_stamp::test_support::stamped_device_headers("owner-phone");
        let typed = headers_with(&[(HEADER_DEVICE_ID, "owner-phone")]);
        for caller in [&agent, &app, &typed] {
            assert!(refuse_unowned_folder_revoke(caller).is_err());
        }
        assert!(refuse_unowned_folder_revoke(&owner).is_ok());
    }

    #[test]
    fn trigger_info_lists_each_folder_grant_by_its_revocable_path() {
        let config = TriggerConfig::from_created_payload(&serde_json::json!({
            "trigger_id": "t", "name": "T", "schedule": [], "timezone": "UTC",
            "run": { "type": "intent", "intent": "x" },
            "on": [{ "event_type": "DayEnded" }],
            "folder_grants": [{ "folder": "/outside/reports", "device": 1, "inode": 10 }],
        }))
        .unwrap();
        let info = serde_json::to_value(TriggerInfo::from_config(&config)).unwrap();
        assert_eq!(
            info["folder_grants"],
            serde_json::json!([{ "folder": "/outside/reports" }])
        );
        let request: UpdateTriggerCronRequest = serde_json::from_value(
            serde_json::json!({ "revoke_folder_grants": ["/outside/reports"] }),
        )
        .unwrap();
        assert_eq!(request.revoke_folder_grants, vec!["/outside/reports"]);
    }

    #[test]
    fn trigger_info_lists_program_grants_without_invocation_fingerprints() {
        let config = TriggerConfig::from_created_payload(&serde_json::json!({
            "trigger_id": "t", "name": "T", "schedule": [], "timezone": "UTC",
            "run": { "type": "intent", "intent": "x" },
            "on": [{ "event_type": "DayEnded" }],
            "program_grants": [
                { "pattern": "Bash(curl:*)" },
                { "pattern": "Python", "legacy": { "fingerprint": "example", "category": "email" } }
            ],
            "side_effect_grant": ["email"]
        }))
        .unwrap();
        let info = serde_json::to_value(TriggerInfo::from_config(&config)).unwrap();
        assert_eq!(
            info["program_grants"],
            serde_json::json!([
                { "pattern": "Bash(curl:*)", "migrated_invocation": false }
            ])
        );
    }
}
