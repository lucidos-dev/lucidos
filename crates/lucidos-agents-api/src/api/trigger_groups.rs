use super::*;

use crate::engine::event_bus::SystemEvent;
use crate::engine::trigger_group_writes::emit_and_apply_group_event;
use crate::engine::trigger_group_writes::{CreateTriggerGroupError, RenameTriggerGroupError};
use crate::triggers::{TriggerGroup, TriggerGroupProjection};

#[derive(Serialize)]
pub struct TriggerGroupInfo {
    pub id: String,
    pub name: String,
    pub order: i32,
    pub created: String,
    pub member_count: usize,
}

#[derive(Serialize)]
pub struct TriggerGroupsListResponse {
    pub groups: Vec<TriggerGroupInfo>,
}

#[derive(Deserialize)]
pub struct CreateTriggerGroupRequest {
    pub name: String,
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

#[derive(Serialize)]
struct DeleteBlockedResponse {
    error: &'static str,
    member_count: usize,
    member_trigger_ids: Vec<String>,
}

async fn emit_and_apply_reorder(
    bus: &crate::engine::event_bus::EventBus,
    groups: &std::sync::Arc<std::sync::RwLock<TriggerGroupProjection>>,
    group_id: &str,
    order: i32,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
) -> Result<(), String> {
    let payload = serde_json::json!({ "group_id": group_id, "order": order });
    emit_and_apply_group_event(
        groups,
        bus,
        SystemEvent::TriggerGroupReordered {
            group_id: group_id.to_string(),
            payload,
            actor,
        },
    )
    .await
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

pub(super) async fn list_trigger_groups(
    State(state): State<AppState>,
) -> Json<TriggerGroupsListResponse> {
    Json(read_trigger_groups(&state))
}

pub(super) fn read_trigger_groups(state: &AppState) -> TriggerGroupsListResponse {
    let counts = member_counts(state);
    let mut groups: Vec<TriggerGroup> = {
        let g = state.engine.trigger_groups.read().unwrap();
        g.values().cloned().collect()
    };
    groups.sort_by(|a, b| a.order.cmp(&b.order).then(a.created.cmp(&b.created)));
    let groups: Vec<TriggerGroupInfo> = groups
        .iter()
        .map(|g| to_info(g, *counts.get(&g.id).unwrap_or(&0)))
        .collect();
    TriggerGroupsListResponse { groups }
}

pub(super) async fn create_trigger_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateTriggerGroupRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let actor = super::actor::user_actor(&headers, None);
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
            log_legacy!(
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

/// INVARIANT: `PUT /api/v1/trigger-groups?id=`: partial update of name and/or order.
///
/// Rename goes through the serialized helper, so the unique-name invariant
/// survives a parallel rename or create against the same target name. Reorder
/// goes through [`emit_and_apply_reorder`], which is what makes the new order
/// visible to the very next `GET`.
pub(super) async fn update_trigger_group(
    State(state): State<AppState>,
    Query(query): Query<TriggerGroupIdQuery>,
    headers: HeaderMap,
    Json(request): Json<UpdateTriggerGroupRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let group_id = query.id;
    let actor = super::actor::user_actor(&headers, None);

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
                log_legacy!(
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
        if let Err(e) = emit_and_apply_reorder(
            &state.engine.event_bus,
            &state.engine.trigger_groups,
            &group_id,
            new_order,
            actor,
        )
        .await
        {
            log_legacy!(
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
    let actor = super::actor::user_actor(&headers, None);
    if let Err(e) = emit_and_apply_group_event(
        &state.engine.trigger_groups,
        &state.engine.event_bus,
        SystemEvent::TriggerGroupDeleted {
            group_id: group_id.clone(),
            payload,
            actor,
        },
    )
    .await
    {
        log_legacy!("[TriggerGroups] Failed to emit TriggerGroupDeleted: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to emit delete: {}", e) })),
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

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

    let actor = super::actor::user_actor(&headers, None);
    for (group_id, order) in to_change {
        if let Err(e) = emit_and_apply_reorder(
            &state.engine.event_bus,
            &state.engine.trigger_groups,
            &group_id,
            order,
            actor.clone(),
        )
        .await
        {
            log_legacy!(
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
    }

    StatusCode::NO_CONTENT.into_response()
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::EventBus;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use std::sync::{Arc, RwLock};

    fn registry(order: i32) -> Arc<RwLock<TriggerGroupProjection>> {
        let mut projection = TriggerGroupProjection::default();
        projection.apply(&crate::triggers::TriggerGroupEventRow {
            seq: 0,
            event_type: "TriggerGroupCreated".into(),
            payload: serde_json::json!({"group_id":"group-1","name":"Morning","order":order}),
            created: chrono::Utc::now(),
        });
        Arc::new(RwLock::new(projection))
    }

    #[tokio::test]
    async fn a_single_group_reorder_reaches_the_registry() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let groups = registry(10);

        emit_and_apply_reorder(&bus, &groups, "group-1", 30, None)
            .await
            .expect("the reorder announces");

        assert_eq!(
            groups.read().unwrap()["group-1"].order,
            30,
            "the next GET reads this registry, so it has to hold the new order"
        );

        let announced: Vec<(String,)> = sqlx::query_as(
            "SELECT event_type FROM events WHERE event_type = 'TriggerGroupReordered'",
        )
        .fetch_all(&pool)
        .await
        .expect("read the audit rows");
        assert_eq!(announced.len(), 1, "the move is announced exactly once");

        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn delete_and_reorder_answer_a_bodyless_204() {
        use axum::response::IntoResponse;
        let name = "api::trigger_groups::tests::delete_and_reorder_answer_a_bodyless_204";
        if !crate::test_support::in_engine_child(name).await {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, workspace) = crate::test_support::boot_mock_engine(&db).await;
        let state = crate::api::permission_requests::tests::permission_test_state(
            engine,
            workspace.path().to_path_buf(),
        )
        .await;
        let morning = state
            .engine
            .create_trigger_group_serialized("Morning", None, None)
            .await
            .unwrap()
            .group_id;
        let evening = state
            .engine
            .create_trigger_group_serialized("Evening", None, None)
            .await
            .unwrap()
            .group_id;

        let reordered = reorder_trigger_groups(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            axum::Json(ReorderTriggerGroupsRequest {
                ordering: vec![TriggerGroupOrderingEntry {
                    id: morning.clone(),
                    order: 5000,
                }],
            }),
        )
        .await
        .into_response();
        let deleted = delete_trigger_group(
            axum::extract::State(state),
            axum::extract::Query(TriggerGroupIdQuery { id: evening }),
            HeaderMap::new(),
        )
        .await
        .into_response();

        for response in [reordered, deleted] {
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert!(body.is_empty(), "a 204 must carry no body: {body:?}");
        }
        teardown_test_db(&db).await;
    }
}
