use super::*;

use crate::core::PreferenceStore;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::triggers::{
    is_valid_trigger_slug, slugify_trigger_name_with_fallback, validate_script_extension,
    TriggerConfig, TriggerRun,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<String>,
    pub run: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<serde_json::Value>,
    /// Owning app directory name (e.g. `"trigger-workflow"`), used to deep-link
    /// notifications back to the right app. None for standalone triggers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// When true, threads spawned by this trigger surface in REVIEW on
    /// completion instead of going straight to HISTORY.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub go_to_review: bool,
}

impl TriggerInfo {
    fn from_config(config: &TriggerConfig) -> Self {
        let run = serde_json::to_value(&config.run).unwrap_or_else(|e| {
            log!(
                "[Triggers] Failed to serialize run for trigger '{}': {}",
                config.id,
                e
            );
            serde_json::Value::Null
        });
        Self {
            id: config.id.clone(),
            name: config.name.clone(),
            slug: config.slug.clone(),
            cron_expressions: config.schedule.clone(),
            timezone: config.timezone.clone(),
            paused: config.paused,
            last_run: config.last_run.map(|t| t.to_rfc3339()),
            next_run: config.next_run().map(|t| t.to_rfc3339()),
            run,
            on: config.on.clone(),
            condition: config.condition.clone(),
            // Surface the resolved (explicit-or-derived) app id so the frontend
            // matches what the engine will stamp on notifications from this trigger.
            app_id: config.owning_app_id(),
            go_to_review: config.go_to_review,
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
    pub name: String,
    pub run: serde_json::Value,
    /// Stable kebab-case slug for the trigger. Directory segment for
    /// per-trigger know-how at `data/triggers/{slug}/knowhow/`. Optional —
    /// derived from `name` (with UUID fallback) when omitted.
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub cron_expressions: Vec<String>,
    #[serde(default)]
    pub on_event: Option<String>,
    #[serde(default)]
    pub condition: Option<serde_json::Value>,
    /// Owning app directory name (e.g. `"trigger-workflow"`). Stamped onto
    /// notifications emitted by this trigger so the popover can deep-link
    /// to the app. Optional; standalone triggers omit it.
    #[serde(default)]
    pub app_id: Option<String>,
    /// When true, threads spawned by this trigger surface in REVIEW on
    /// completion instead of going straight to HISTORY. Default false.
    #[serde(default)]
    pub go_to_review: bool,
}

#[derive(Deserialize)]
pub struct UpdateTriggerCronRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub run: Option<serde_json::Value>,
    #[serde(default)]
    pub cron_expressions: Option<Vec<String>>,
    #[serde(default)]
    pub paused: Option<bool>,
    /// None = field absent (don't change), Some(None) = explicitly null (clear), Some(Some(v)) = set.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_nullable::<String, _>"
    )]
    pub on_event: Option<Option<String>>,
    /// None = field absent (don't change), Some(None) = explicitly null (clear), Some(Some(v)) = set.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_nullable::<serde_json::Value, _>"
    )]
    pub condition: Option<Option<serde_json::Value>>,
    /// None = field absent (don't change), Some(None) = explicitly null (clear), Some(Some(v)) = set.
    #[serde(
        default,
        deserialize_with = "deserialize_optional_nullable::<String, _>"
    )]
    pub app_id: Option<Option<String>>,
    #[serde(default)]
    pub go_to_review: Option<bool>,
    /// Optional slug edit (e.g. trigger renamed). Validated against
    /// [`is_valid_trigger_slug`] — invalid values reject the request with 400.
    #[serde(default)]
    pub slug: Option<String>,
}

/// Deserialize a field that can be absent, null, or a value.
/// Absent → None, null → Some(None), value → Some(Some(value))
fn deserialize_optional_nullable<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(deserializer)?))
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

/// Resolve the slug for a new trigger: validate an explicit submission, or
/// derive from `name` (UUID-shortened fallback when name slugifies to empty).
/// Pure helper, exposed for unit tests.
pub(crate) fn resolve_create_slug(
    explicit: Option<&str>,
    name: &str,
    trigger_id: &str,
) -> Result<String, String> {
    if let Some(s) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        if !is_valid_trigger_slug(s) {
            return Err(format!(
                "Invalid slug '{}': must be 1-64 chars of [a-z0-9-], starting and ending with [a-z0-9]",
                s
            ));
        }
        return Ok(s.to_string());
    }
    Ok(slugify_trigger_name_with_fallback(name, trigger_id))
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
            log!("[API] Failed to list historical triggers: {}", e);
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

/// Create a new trigger
pub(super) async fn create_trigger(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateTriggerCronRequest>,
) -> Json<ApiResult> {
    // Validate name
    let name = request.name.trim();
    if name.is_empty() {
        return ApiResult::err("Trigger name is required");
    }

    // Validate run field
    let run: TriggerRun = match serde_json::from_value(request.run.clone()) {
        Ok(r) => r,
        Err(e) => return ApiResult::err(format!("Invalid 'run' field: {}", e)),
    };

    // Validate script extension if script trigger
    if let TriggerRun::Script { ref path } = run {
        if let Err(e) = validate_script_extension(path) {
            return ApiResult::err(e);
        }
    }

    // Validate: at least one of cron or on_event must be provided
    let has_cron = !request.cron_expressions.is_empty();
    let on_event = request
        .on_event
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let has_event = on_event.is_some();
    if !has_cron && !has_event {
        return ApiResult::err(
            "At least one cron expression or an event type (on_event) is required",
        );
    }

    // Validate cron expressions if provided
    for expr in &request.cron_expressions {
        let expr = expr.trim();
        if crate::engine::tools::scheduler::parse_standard_cron(expr).is_err() {
            return ApiResult::err(format!("Invalid cron expression: '{}'", expr));
        }
    }
    let cron_expressions: Vec<String> = request
        .cron_expressions
        .iter()
        .map(|s| s.trim().to_string())
        .collect();

    // Read timezone from preferences (default to UTC)
    let timezone = PreferenceStore::get(&state.pool, "timezone")
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "UTC".to_string());

    let run_value = match serde_json::to_value(&run) {
        Ok(v) => v,
        Err(e) => return ApiResult::err(format!("Failed to serialize 'run': {}", e)),
    };
    let trigger_id_str = Uuid::new_v4().to_string();

    // Slug: explicit (validated) or derived from name (UUID fallback).
    let slug = match resolve_create_slug(request.slug.as_deref(), name, &trigger_id_str) {
        Ok(s) => s,
        Err(e) => return ApiResult::err(e),
    };

    let mut payload = serde_json::json!({
        "trigger_id": trigger_id_str,
        "name": name,
        "slug": slug,
        "schedule": cron_expressions,
        "timezone": timezone,
        "run": run_value,
    });
    if let Some(ref ev) = on_event {
        payload["on"] = serde_json::json!(ev);
    }
    if let Some(ref cond) = request.condition {
        payload["condition"] = cond.clone();
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

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    if let Err(e) = state
        .engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::TriggerCreated {
            trigger_id: trigger_id_str.clone(),
            payload,
            actor,
        }))
        .await
    {
        log!("[Triggers] Failed to emit TriggerCreated event: {}", e);
    }

    ApiResult::ok()
}

/// Update an existing trigger
pub(super) async fn update_trigger(
    State(state): State<AppState>,
    Query(query): Query<TriggerIdQuery>,
    headers: HeaderMap,
    Json(request): Json<UpdateTriggerCronRequest>,
) -> Json<ApiResult> {
    let task_id = query.id;

    // Fetch existing trigger from in-memory state
    let existing = match state.scheduler.lock().await.get_trigger_config(&task_id) {
        Some(c) => c,
        None => return ApiResult::err(format!("Trigger '{}' not found", task_id)),
    };

    // Validate cron expressions if provided
    if let Some(ref exprs) = request.cron_expressions {
        for expr in exprs {
            let expr = expr.trim();
            if crate::engine::tools::scheduler::parse_standard_cron(expr).is_err() {
                return ApiResult::err(format!("Invalid cron expression: '{}'", expr));
            }
        }
    }

    // Validate run field if changing
    if let Some(ref run_val) = request.run {
        match serde_json::from_value::<TriggerRun>(run_val.clone()) {
            Ok(parsed_run) => {
                if let TriggerRun::Script { ref path } = parsed_run {
                    if let Err(e) = validate_script_extension(path) {
                        return ApiResult::err(e);
                    }
                }
            }
            Err(_) => return ApiResult::err("Invalid 'run' field"),
        }
    }

    // Build update payload with only changed fields
    let trigger_id_str = task_id.clone();
    let mut update_payload = serde_json::json!({ "trigger_id": trigger_id_str });

    if let Some(ref n) = request.name {
        update_payload["name"] = serde_json::json!(n.trim());
    }
    if let Some(ref exprs) = request.cron_expressions {
        let updated_crons: Vec<String> = exprs.iter().map(|s| s.trim().to_string()).collect();
        update_payload["schedule"] = serde_json::json!(updated_crons);
    }
    if let Some(ref run_val) = request.run {
        if let Ok(run) = serde_json::from_value::<TriggerRun>(run_val.clone()) {
            match serde_json::to_value(&run) {
                Ok(v) => {
                    update_payload["run"] = v;
                }
                Err(e) => return ApiResult::err(format!("Failed to serialize 'run': {}", e)),
            }
        }
    }
    if let Some(paused) = request.paused {
        update_payload["paused"] = serde_json::json!(paused);
    }

    // Normalize on_event: trim whitespace, treat whitespace-only as null (clear).
    let normalized_on: Option<Option<String>> = request.on_event.as_ref().map(|v| {
        v.as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    });

    // None = absent (keep existing), Some(None) = explicit null (clear), Some(Some(v)) = set
    if let Some(v) = &normalized_on {
        update_payload["on"] = serde_json::json!(v);
    }
    if let Some(v) = &request.condition {
        update_payload["condition"] = serde_json::json!(v);
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
            Err(e) => return ApiResult::err(e),
        }
    }

    // Ensure trigger still has at least one firing mechanism after update
    let updated_crons = request
        .cron_expressions
        .as_ref()
        .unwrap_or(&existing.schedule);
    let updated_on = match &normalized_on {
        Some(v) => v.clone(),
        None => existing.on.clone(),
    };
    if updated_crons.is_empty() && updated_on.is_none() {
        return ApiResult::err("Trigger must have at least one cron expression or an event type");
    }

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    if let Err(e) = state
        .engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::TriggerUpdated {
            trigger_id: trigger_id_str.clone(),
            payload: update_payload,
            actor,
        }))
        .await
    {
        log!("[Triggers] Failed to emit TriggerUpdated event: {}", e);
    }

    ApiResult::ok()
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

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    if let Err(e) = state
        .engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::TriggerDeleted {
            trigger_id: task_id.clone(),
            payload: serde_json::json!({ "trigger_id": &task_id }),
            actor,
        }))
        .await
    {
        log!("[Triggers] Failed to emit TriggerDeleted event: {}", e);
    }

    ApiResult::ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Slug resolution at the create boundary ---

    #[test]
    fn resolve_create_slug_accepts_explicit_valid() {
        let slug = resolve_create_slug(Some("send-daily-summary"), "Anything", "uuid").unwrap();
        assert_eq!(slug, "send-daily-summary");
    }

    #[test]
    fn resolve_create_slug_derives_from_name_when_omitted() {
        let slug = resolve_create_slug(None, "Send Daily Summary", "uuid-1234").unwrap();
        assert_eq!(slug, "send-daily-summary");
    }

    #[test]
    fn resolve_create_slug_derives_from_name_when_blank() {
        // Whitespace-only treated like absent.
        let slug = resolve_create_slug(Some("   "), "Send Daily Summary", "uuid").unwrap();
        assert_eq!(slug, "send-daily-summary");
    }

    #[test]
    fn resolve_create_slug_falls_back_to_uuid_when_name_empty_after_slugify() {
        let slug = resolve_create_slug(None, "!!!", "abcdef-1234-5678").unwrap();
        assert_eq!(slug, "trigger-abcdef12");
    }

    #[test]
    fn resolve_create_slug_rejects_explicit_invalid() {
        let err = resolve_create_slug(Some("Bad Slug"), "Anything", "uuid").unwrap_err();
        assert!(err.contains("Invalid slug"));
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
}
