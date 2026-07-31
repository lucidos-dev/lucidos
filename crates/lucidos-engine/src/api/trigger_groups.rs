//! HTTP handlers for `/trigger-groups` — CRUD and atomic batch reorder for the
//! user-visible folder concept that organizes triggers in the panel.
//!
//! Groups are first-class, event-sourced, in-memory entities; the engine holds
//! the registry under `LucidosEngine.trigger_groups` (same Arc the scheduler
//! shares for replay). These handlers emit `TriggerGroup*` events through
//! EventBus; the scheduler's subscriber updates the registry, and SSE
//! consumers see the change live.

use super::*;

use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::trigger_group_writes::{CreateTriggerGroupError, RenameTriggerGroupError};
use crate::triggers::TriggerGroup;

#[derive(Serialize)]
pub struct TriggerGroupInfo {
    pub id: String,
    pub name: String,
    pub order: i32,
    pub created: String,
    /// Number of triggers whose `group_id` references this group. Computed at
    /// response time from the in-memory trigger registry — no projection table.
    /// The panel uses it to show the "(N)" badge on each section header.
    pub member_count: usize,
}

#[derive(Serialize)]
pub struct TriggerGroupsListResponse {
    pub groups: Vec<TriggerGroupInfo>,
}

#[derive(Deserialize)]
pub struct CreateTriggerGroupRequest {
    pub name: String,
    /// Optional sort position; defaults to `max(existing.order) + 10` so a
    /// fresh group sinks to the bottom without renumbering anything.
    #[serde(default)]
    pub order: Option<i32>,
}

#[derive(Deserialize)]
pub struct UpdateTriggerGroupRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub order: Option<i32>,
}

#[derive(Deserialize)]
pub struct ReorderTriggerGroupsRequest {
    pub ordering: Vec<TriggerGroupOrderingEntry>,
}

#[derive(Deserialize)]
pub struct TriggerGroupOrderingEntry {
    pub id: String,
    pub order: i32,
}

#[derive(Deserialize)]
pub(super) struct TriggerGroupIdQuery {
    id: String,
}

/// Body returned by `DELETE /trigger-groups?id=` when the group still has
/// member triggers. The LLM reads `member_count` and `member_trigger_ids` to
/// self-correct (move or delete the members first, then retry).
#[derive(Serialize)]
struct DeleteBlockedResponse {
    error: &'static str,
    member_count: usize,
    member_trigger_ids: Vec<String>,
}

fn to_info(group: &TriggerGroup, member_count: usize) -> TriggerGroupInfo {
    TriggerGroupInfo {
        id: group.id.clone(),
        name: group.name.clone(),
        order: group.order,
        created: group.created.to_rfc3339(),
        member_count,
    }
}

fn member_counts(state: &AppState) -> std::collections::HashMap<String, usize> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let configs = state.engine.trigger_configs.read().unwrap();
    for config in configs.values() {
        if let Some(ref gid) = config.group_id {
            *counts.entry(gid.clone()).or_insert(0) += 1;
        }
    }
    counts
}

fn member_trigger_ids(state: &AppState, group_id: &str) -> Vec<String> {
    let configs = state.engine.trigger_configs.read().unwrap();
    configs
        .values()
        .filter(|c| c.group_id.as_deref() == Some(group_id))
        .map(|c| c.id.clone())
        .collect()
}

/// `GET /trigger-groups` — list groups sorted by `order` asc, then `created` asc.
pub(super) async fn list_trigger_groups(
    State(state): State<AppState>,
) -> Json<TriggerGroupsListResponse> {
    let counts = member_counts(&state);
    let mut groups: Vec<TriggerGroup> = {
        let g = state.engine.trigger_groups.read().unwrap();
        g.values().cloned().collect()
    };
    groups.sort_by(|a, b| a.order.cmp(&b.order).then(a.created.cmp(&b.created)));
    let groups: Vec<TriggerGroupInfo> = groups
        .iter()
        .map(|g| to_info(g, *counts.get(&g.id).unwrap_or(&0)))
        .collect();
    Json(TriggerGroupsListResponse { groups })
}

/// `POST /trigger-groups` — create a new group. Rejects empty / duplicate names.
pub(super) async fn create_trigger_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateTriggerGroupRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match state
        .engine
        .create_trigger_group_serialized(&request.name, request.order, actor)
        .await
    {
        Ok(created) => {
            let info = TriggerGroupInfo {
                id: created.group_id,
                name: created.name,
                order: created.order,
                created: created.created.to_rfc3339(),
                member_count: 0,
            };
            (StatusCode::OK, Json(serde_json::to_value(info).unwrap()))
        }
        Err(CreateTriggerGroupError::EmptyName) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Group name is required" })),
        ),
        Err(CreateTriggerGroupError::DuplicateName { existing_name }) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!("A group named '{}' already exists", existing_name)
            })),
        ),
        Err(CreateTriggerGroupError::EmitFailed(msg)) => {
            log!(
                "[TriggerGroups] Failed to emit TriggerGroupCreated: {}",
                msg
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to emit event: {}", msg) })),
            )
        }
    }
}

/// `PUT /trigger-groups?id=` — partial update of name and/or order. Rename
/// goes through the serialized helper so the unique-name invariant survives
/// a parallel rename or create against the same target name; reorder applies
/// lazily via the broadcast subscriber because order isn't an invariant any
/// reader dedups against.
pub(super) async fn update_trigger_group(
    State(state): State<AppState>,
    Query(query): Query<TriggerGroupIdQuery>,
    headers: HeaderMap,
    Json(request): Json<UpdateTriggerGroupRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let group_id = query.id;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    if let Some(ref new_name) = request.name {
        match state
            .engine
            .rename_trigger_group_serialized(&group_id, new_name, actor.clone())
            .await
        {
            Ok(_) => {}
            Err(RenameTriggerGroupError::EmptyName) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "Group name cannot be empty" })),
                );
            }
            Err(RenameTriggerGroupError::NotFound) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": format!("Group '{}' not found", group_id) })),
                );
            }
            Err(RenameTriggerGroupError::DuplicateName { existing_name }) => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": format!("A group named '{}' already exists", existing_name)
                    })),
                );
            }
            Err(RenameTriggerGroupError::EmitFailed(msg)) => {
                log!(
                    "[TriggerGroups] Failed to emit TriggerGroupRenamed: {}",
                    msg
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("Failed to emit rename: {}", msg) })),
                );
            }
        }
    }

    let existing = {
        let g = state.engine.trigger_groups.read().unwrap();
        match g.get(&group_id) {
            Some(g) => g.clone(),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": format!("Group '{}' not found", group_id) })),
                );
            }
        }
    };

    let new_order = request.order.unwrap_or(existing.order);
    if new_order != existing.order {
        let payload = serde_json::json!({ "group_id": group_id, "order": new_order });
        if let Err(e) = state
            .engine
            .event_bus
            .emit(BusEvent::System(SystemEvent::TriggerGroupReordered {
                group_id: group_id.clone(),
                payload,
                actor,
            }))
            .await
        {
            log!(
                "[TriggerGroups] Failed to emit TriggerGroupReordered: {}",
                e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to emit reorder: {}", e) })),
            );
        }
    }

    let counts = member_counts(&state);
    let info = TriggerGroupInfo {
        id: group_id.clone(),
        name: existing.name,
        order: new_order,
        created: existing.created.to_rfc3339(),
        member_count: *counts.get(&group_id).unwrap_or(&0),
    };
    (StatusCode::OK, Json(serde_json::to_value(info).unwrap()))
}

/// `DELETE /trigger-groups?id=` — refuse with `409` when the group still has
/// member triggers. The response body lists `member_count` and
/// `member_trigger_ids` so the LLM can move them and retry.
///
/// Returns `impl IntoResponse` (not a `(StatusCode, Json)` tuple) so the success
/// path can emit a *bodyless* `204 No Content`. A 204 carrying a JSON body is
/// malformed HTTP — Chromium tolerates it but WebKit/iOS rejects the response
/// with `TypeError: Load failed`, which surfaced as "Failed to delete group:
/// Load failed" on iOS and only ever failed on the `mobile-webkit` e2e project.
pub(super) async fn delete_trigger_group(
    State(state): State<AppState>,
    Query(query): Query<TriggerGroupIdQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let group_id = query.id;

    {
        let g = state.engine.trigger_groups.read().unwrap();
        if !g.contains_key(&group_id) {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("Group '{}' not found", group_id) })),
            )
                .into_response();
        }
    }

    let members = member_trigger_ids(&state, &group_id);
    if !members.is_empty() {
        let body = DeleteBlockedResponse {
            error: "non_empty",
            member_count: members.len(),
            member_trigger_ids: members,
        };
        return (
            StatusCode::CONFLICT,
            Json(serde_json::to_value(body).unwrap()),
        )
            .into_response();
    }

    let payload = serde_json::json!({ "group_id": group_id });
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    if let Err(e) = state
        .engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::TriggerGroupDeleted {
            group_id: group_id.clone(),
            payload,
            actor,
        }))
        .await
    {
        log!("[TriggerGroups] Failed to emit TriggerGroupDeleted: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to emit delete: {}", e) })),
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

/// `POST /trigger-groups/reorder` — atomic batch reorder. Validates every id
/// up front, then emits one `TriggerGroupReordered` per group whose `order`
/// actually changes. Unknown ids reject the whole batch with `400`.
///
/// Returns `impl IntoResponse` so the success path is a *bodyless* `204` — see
/// `delete_trigger_group` for why a 204-with-body breaks WebKit/iOS.
pub(super) async fn reorder_trigger_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ReorderTriggerGroupsRequest>,
) -> impl IntoResponse {
    let to_change: Vec<(String, i32)> = {
        let g = state.engine.trigger_groups.read().unwrap();
        let mut acc = Vec::with_capacity(request.ordering.len());
        for entry in &request.ordering {
            let current = match g.get(&entry.id) {
                Some(g) => g,
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": format!("Unknown group_id '{}'", entry.id)
                        })),
                    )
                        .into_response();
                }
            };
            if current.order != entry.order {
                acc.push((entry.id.clone(), entry.order));
            }
        }
        acc
    };

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    for (group_id, order) in to_change {
        let payload = serde_json::json!({ "group_id": group_id, "order": order });
        if let Err(e) = state
            .engine
            .event_bus
            .emit(BusEvent::System(SystemEvent::TriggerGroupReordered {
                group_id: group_id.clone(),
                payload: payload.clone(),
                actor: actor.clone(),
            }))
            .await
        {
            log!(
                "[TriggerGroups] Failed to emit TriggerGroupReordered for {}: {}",
                group_id,
                e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to emit reorder: {}", e) })),
            )
                .into_response();
        }
        crate::scheduler::handle_trigger_group_event(
            "TriggerGroupReordered",
            &group_id,
            &payload,
            &state.engine.trigger_groups,
        );
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Routes for the `/trigger-groups*` surface — user-visible folders that
/// organize triggers in the panel. Pure label, no firing. The reorder
/// endpoint is a batch op so the panel can persist a drag-to-reorder with
/// one round-trip.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/trigger-groups",
            get(list_trigger_groups)
                .post(create_trigger_group)
                .put(update_trigger_group)
                .delete(delete_trigger_group),
        )
        .route("/trigger-groups/reorder", post(reorder_trigger_groups))
}
