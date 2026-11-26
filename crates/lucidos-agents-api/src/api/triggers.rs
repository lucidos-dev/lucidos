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
    pub slug: String,
    pub cron_expressions: Vec<String>,
    pub timezone: String,
    pub paused: bool,
    pub last_run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_status: Option<TriggerRunStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_runs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_error: Option<String>,
    pub run: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<EventSubscription>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub go_to_review: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// INVARIANT: Historical category grants. They confer no authority.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub side_effect_grant: Vec<SideEffectCategory>,
    pub program_grants: Vec<TriggerProgramGrantInfo>,
    pub folder_grants: Vec<TriggerFolderGrantInfo>,
    pub program_denies: Vec<String>,
    pub folder_denies: Vec<String>,
    pub coding_agent_grant: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_ceiling: Option<ContextCeiling>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continue_key: Option<ContinueKeySpec>,
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
            program_denies: config.program_denies.clone(),
            folder_denies: config
                .folder_denies
                .iter()
                .map(|folder| folder.to_string_lossy().into_owned())
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
    pub name: Option<String>,
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
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub cron_expressions: Vec<String>,
    #[serde(default)]
    pub on: Vec<EventSubscription>,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub go_to_review: bool,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub side_effect_grant: Vec<SideEffectCategory>,
    #[serde(default)]
    pub coding_agent_grant: bool,
    /// INVARIANT: The context ceiling copied onto each fired thread, read by
    /// `ContextCeiling::parse`. Omitted or null = the workspace preference. An
    /// agent may not set one above the workspace's.
    #[serde(default)]
    pub context_ceiling: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub continue_key: Option<serde_json::Value>,
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
    #[serde(default)]
    pub on: Option<Vec<EventSubscription>>,
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub app_id: Option<Option<String>>,
    #[serde(default)]
    pub go_to_review: Option<bool>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub group_id: Option<Option<String>>,
    #[serde(default)]
    pub side_effect_grant: Option<Vec<SideEffectCategory>>,
    #[serde(default)]
    pub revoke_program_grants: Vec<String>,
    #[serde(default)]
    pub revoke_folder_grants: Vec<String>,
    #[serde(default)]
    pub revoke_program_denies: Vec<String>,
    #[serde(default)]
    pub revoke_folder_denies: Vec<String>,
    #[serde(default)]
    pub coding_agent_grant: Option<bool>,
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub context_ceiling: Option<Option<String>>,
    #[serde(default)]
    pub mcp_servers: Option<Vec<String>>,
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub model: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub reasoning_effort: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub provider: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub continue_key: Option<Option<serde_json::Value>>,
    #[serde(default, deserialize_with = "crate::api::deserialize_some")]
    pub actor: Option<Option<serde_json::Value>>,
}

pub(crate) fn parse_context_ceiling_arg(
    raw: Option<&str>,
) -> Result<Option<ContextCeiling>, String> {
    raw.map(ContextCeiling::parse).transpose()
}

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

pub(super) async fn list_triggers(State(state): State<AppState>) -> Json<TriggersListResponse> {
    Json(read_triggers(&state).await)
}

pub(super) async fn read_triggers(state: &AppState) -> TriggersListResponse {
    let configs = state.scheduler.lock().await.list_trigger_configs();
    let triggers: Vec<TriggerInfo> = configs.iter().map(TriggerInfo::from_config).collect();
    TriggersListResponse { triggers }
}

pub(super) async fn list_historical_triggers(
    State(state): State<AppState>,
) -> Result<Json<HistoricalTriggersResponse>, (StatusCode, String)> {
    read_historical_triggers(&state).await.map(Json)
}

pub(super) async fn read_historical_triggers(
    state: &AppState,
) -> Result<HistoricalTriggersResponse, (StatusCode, String)> {
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
    Ok(HistoricalTriggersResponse { triggers })
}

fn is_unowned_caller(headers: &HeaderMap) -> bool {
    let origin = crate::api::actor::user_actor(headers, None);
    super::intelligence_ladder::is_agent_caller(headers, origin.as_ref())
        || headers.contains_key(super::app_reach::APP_ID_HEADER)
}

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

fn refuse_unowned_revoke(headers: &HeaderMap, what: &str) -> Result<(), ApiError> {
    if is_unowned_caller(headers) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            format!("Only the owner can revoke a trigger's {what}."),
        ));
    }
    if crate::api::actor::owner_attestation(headers)
        == crate::api::actor::OwnerAttestation::Unattested
    {
        return Err(unattested());
    }
    Ok(())
}

fn refuse_unowned_revokes(
    headers: &HeaderMap,
    request: &UpdateTriggerCronRequest,
) -> Result<(), ApiError> {
    if !request.revoke_folder_grants.is_empty() {
        refuse_unowned_revoke(headers, "folder grants")?;
    }
    if !request.revoke_program_denies.is_empty() || !request.revoke_folder_denies.is_empty() {
        refuse_unowned_revoke(headers, "denies")?;
    }
    Ok(())
}

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

pub(super) async fn create_trigger(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateTriggerCronRequest>,
) -> Result<Json<ApiResult>, ApiError> {
    let name = request.name.trim();
    if name.is_empty() {
        return Ok(ApiResult::err("Trigger name is required"));
    }

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

pub(super) async fn update_trigger(
    State(state): State<AppState>,
    Query(query): Query<TriggerIdQuery>,
    headers: HeaderMap,
    Json(request): Json<UpdateTriggerCronRequest>,
) -> Result<Json<ApiResult>, ApiError> {
    let task_id = query.id;

    let existing = match state.scheduler.lock().await.get_trigger_config(&task_id) {
        Some(c) => c,
        None => return Ok(ApiResult::err(format!("Trigger '{}' not found", task_id))),
    };

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

    refuse_unowned_revokes(&headers, &request)?;

    let normalized_on: Option<Vec<EventSubscription>> =
        request.on.map(EventSubscription::normalize_list);

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

    if let Some(ref subs) = normalized_on {
        update_payload["on"] =
            serde_json::to_value(subs).expect("EventSubscription serialization is infallible");
    }
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
        update_payload["revoke_folder_grants"] = serde_json::json!(request.revoke_folder_grants);
    }
    if !request.revoke_program_denies.is_empty() || !request.revoke_folder_denies.is_empty() {
        update_payload["revoke_program_denies"] = serde_json::json!(request.revoke_program_denies);
        update_payload["revoke_folder_denies"] = serde_json::json!(request.revoke_folder_denies);
    }
    if let Some(grant) = request.coding_agent_grant {
        update_payload["coding_agent_grant"] = serde_json::json!(grant);
    }
    if let Some(raw) = &request.context_ceiling {
        let ceiling = match parse_context_ceiling_arg(raw.as_deref()) {
            Ok(ceiling) => ceiling,
            Err(e) => return Ok(ApiResult::err(e)),
        };
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

    Ok(ApiResult::ok_for_trigger(
        validated.as_ref().map(CronPreview::from_validated),
        event_warnings,
    ))
}

pub(super) async fn delete_trigger(
    State(state): State<AppState>,
    Query(query): Query<TriggerIdQuery>,
    headers: HeaderMap,
) -> Json<ApiResult> {
    let task_id = query.id;

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

#[derive(Serialize)]
pub struct TriggerRunResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
    pub message: String,
}

/// INVARIANT: Fire an existing trigger once, right now, outside its schedule.
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

    #[test]
    fn validate_script_path_keeps_the_extension_rule() {
        assert!(validate_script_path("scripts/daily-summary.sh").is_ok());
        assert!(validate_script_path("scripts/daily-summary.py").is_ok());
        let err = validate_script_path("scripts/daily-summary.txt").unwrap_err();
        assert!(err.contains("Unsupported script extension"), "{err}");
    }

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
        let taken: std::collections::HashSet<String> =
            ["daily-summary".to_string()].into_iter().collect();
        let slug = crate::triggers::mint_unique_trigger_slug("Daily Summary", "uuid-2", &taken);
        assert_eq!(slug, "daily-summary-2");
    }

    #[test]
    fn validate_update_slug_accepts_well_formed() {
        assert_eq!(
            validate_update_slug("renamed-trigger").unwrap(),
            "renamed-trigger"
        );
        assert_eq!(validate_update_slug("  abc  ").unwrap(), "abc");
    }

    #[test]
    fn validate_update_slug_rejects_invalid() {
        let err = validate_update_slug("Has Spaces").unwrap_err();
        assert!(err.contains("Invalid slug"));
        let err = validate_update_slug("-leading").unwrap_err();
        assert!(err.contains("Invalid slug"));
    }

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
            assert!(refuse_unowned_revoke(caller, "folder grants").is_err());
        }
        assert!(refuse_unowned_revoke(&owner, "denies").is_ok());
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

    #[test]
    fn only_the_owner_lifts_a_revoke_whichever_field_carries_it() {
        use crate::core::subprocess_origin::{
            init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
            HEADER_DEVICE_ID,
        };
        init_agent_origin_secret("coding-agent-grant-secret".into());
        let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None).unwrap();
        let agent = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
        let app = headers_with(&[(super::super::app_reach::APP_ID_HEADER, "habit-tracker")]);
        let typed = headers_with(&[(HEADER_DEVICE_ID, "owner-phone")]);
        let owner = crate::api::gateway_stamp::test_support::stamped_device_headers("owner-phone");
        let request_with = |field: &str| -> UpdateTriggerCronRequest {
            let mut body = serde_json::Map::new();
            body.insert(field.to_string(), serde_json::json!(["/outside"]));
            serde_json::from_value(serde_json::Value::Object(body)).unwrap()
        };
        for field in [
            "revoke_folder_grants",
            "revoke_program_denies",
            "revoke_folder_denies",
        ] {
            let request = request_with(field);
            for caller in [&agent, &app, &typed] {
                assert!(refuse_unowned_revokes(caller, &request).is_err(), "{field}");
            }
            assert!(refuse_unowned_revokes(&owner, &request).is_ok(), "{field}");
        }
        let nothing: UpdateTriggerCronRequest =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(refuse_unowned_revokes(&agent, &nothing).is_ok());
    }

    #[test]
    fn trigger_info_always_serves_the_coding_agent_grant() {
        let config = TriggerConfig::from_created_payload(&serde_json::json!({
            "trigger_id": "t", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
            "run": { "type": "intent", "intent": "x" },
        }))
        .unwrap();
        let info = serde_json::to_value(TriggerInfo::from_config(&config)).unwrap();
        assert_eq!(info["coding_agent_grant"], serde_json::json!(false));
    }

    #[test]
    fn trigger_info_serves_the_app_id_the_engine_stamps() {
        let config = TriggerConfig::from_created_payload(&serde_json::json!({
            "trigger_id": "t", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
            "run": { "type": "script", "path": "apps/trigger-workflow/triggers/scripts/nightly.py" },
        }))
        .unwrap();
        assert_eq!(config.app_id, None);
        let info = serde_json::to_value(TriggerInfo::from_config(&config)).unwrap();
        assert_eq!(info["app_id"], "trigger-workflow");
    }

    async fn engine_state(name: &str) -> Option<(AppState, tempfile::TempDir, String)> {
        if !crate::test_support::in_engine_child(&format!("api::triggers::tests::{name}")).await {
            return None;
        }
        let (_pool, db) = crate::test_support::setup_test_db().await;
        let (engine, workspace) = crate::test_support::boot_mock_engine(&db).await;
        let state = crate::api::permission_requests::tests::permission_test_state(
            engine,
            workspace.path().to_path_buf(),
        )
        .await;
        Some((state, workspace, db))
    }

    fn owner_headers() -> HeaderMap {
        crate::api::gateway_stamp::test_support::stamped_device_headers("owner-phone")
    }

    fn create_request(extra: serde_json::Value) -> CreateTriggerCronRequest {
        let mut body = serde_json::json!({
            "name": "Morning",
            "run": { "type": "intent", "intent": "x" },
            "cron_expressions": ["0 0 8 * * *"],
        });
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        serde_json::from_value(body).unwrap()
    }

    async fn update(state: &AppState, id: &str, body: serde_json::Value) -> ApiResult {
        update_trigger(
            State(state.clone()),
            Query(TriggerIdQuery { id: id.to_string() }),
            owner_headers(),
            Json(serde_json::from_value(body).unwrap()),
        )
        .await
        .unwrap()
        .0
    }

    #[tokio::test]
    async fn create_checks_event_names_and_registers_before_answering() {
        let name = "create_checks_event_names_and_registers_before_answering";
        let Some((state, _workspace, db)) = engine_state(name).await else {
            return;
        };
        crate::test_support::seed_device(&state.pool, "owner-phone", Some("Mozilla/5.0"), None)
            .await;

        let refused = create_trigger(
            State(state.clone()),
            owner_headers(),
            Json(create_request(
                serde_json::json!({ "on": [{ "event_type": "TextStreamed" }] }),
            )),
        )
        .await
        .unwrap()
        .0;
        assert!(
            refused.error.is_some(),
            "a name that never fires is refused"
        );
        assert!(state.engine.trigger_configs.read().unwrap().is_empty());

        let created = create_trigger(
            State(state.clone()),
            owner_headers(),
            Json(create_request(
                serde_json::json!({ "app_id": "trigger-workflow" }),
            )),
        )
        .await
        .unwrap()
        .0;
        assert!(created.success, "{:?}", created.error);
        let app_id = {
            let configs = state.engine.trigger_configs.read().unwrap();
            let config = configs
                .values()
                .find(|c| c.name == "Morning")
                .expect("the registry holds the trigger before the 200 lands");
            config.app_id.clone()
        };
        assert_eq!(app_id.as_deref(), Some("trigger-workflow"));
        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn update_checks_what_it_supplies_and_registers_before_answering() {
        let name = "update_checks_what_it_supplies_and_registers_before_answering";
        let Some((state, _workspace, db)) = engine_state(name).await else {
            return;
        };
        let held = TriggerConfig::from_created_payload(&serde_json::json!({
            "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"], "timezone": "UTC",
            "run": { "type": "intent", "intent": "x" },
            "on": [{ "event_type": "TextStreamed" }],
        }))
        .unwrap();
        state
            .engine
            .trigger_configs
            .write()
            .unwrap()
            .insert("t1".to_string(), held);
        let read = |f: &dyn Fn(&TriggerConfig) -> bool| {
            f(state
                .engine
                .trigger_configs
                .read()
                .unwrap()
                .get("t1")
                .unwrap())
        };

        let renamed = update(&state, "t1", serde_json::json!({ "name": "Renamed" })).await;
        assert!(
            renamed.success,
            "a rename must not re-refuse the old list: {:?}",
            renamed.error
        );

        let supplied = serde_json::json!({ "on": [{ "event_type": "TextStreamed" }] });
        assert!(update(&state, "t1", supplied).await.error.is_some());

        let restored = serde_json::json!({ "side_effect_grant": ["email"] });
        assert!(update(&state, "t1", restored).await.error.is_some());
        let cleared = update(&state, "t1", serde_json::json!({ "side_effect_grant": [] })).await;
        assert!(cleared.success, "{:?}", cleared.error);

        let paused = update(&state, "t1", serde_json::json!({ "paused": true })).await;
        assert!(paused.success, "{:?}", paused.error);
        assert!(read(&|c| c.paused), "the registry holds the pause at once");

        let emptied = update(&state, "t1", serde_json::json!({ "on": [] })).await;
        assert!(emptied.success, "{:?}", emptied.error);
        assert!(read(&|c| c.on.is_empty()), "an empty list clears");
        crate::test_support::teardown_test_db(&db).await;
    }
}
