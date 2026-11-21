//! `GET /api/v1/threads/:thread_id/switch-estimate?model=&effort=`: what
//! switching a thread to that model and effort would cost. A missing `model`
//! or `effort` leaves that axis unchanged.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::switch_estimate::{
    last_turn_settings, switch_estimate, SwitchEstimate, SwitchSubject, SwitchTarget,
};
use crate::runtime::CodingAgent;

#[derive(Debug, Deserialize)]
pub(in crate::api) struct SwitchEstimateQuery {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
}

pub(in crate::api) async fn get_switch_estimate(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Query(query): Query<SwitchEstimateQuery>,
) -> Result<Json<SwitchEstimate>, ApiError> {
    let thread_id = Uuid::parse_str(&thread_id)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))?;
    let (is_coding_agent, coding_agent): (bool, Option<String>) = sqlx::query_as(
        "SELECT is_coding_agent, coding_agent FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::db)?
    .ok_or_else(|| ApiError::not_found(format!("Thread {thread_id} does not exist")))?;
    let subject = if is_coding_agent {
        SwitchSubject::CodingAgent(
            coding_agent
                .as_deref()
                .map_or(CodingAgent::ClaudeCode, CodingAgent::parse),
        )
    } else {
        SwitchSubject::LucidosAgent
    };
    let target = SwitchTarget {
        model: query.model.filter(|m| !m.is_empty()),
        effort: query.effort.filter(|e| !e.is_empty()),
    };
    let current = match subject {
        SwitchSubject::CodingAgent(_) => state
            .engine
            .cc_thread_runs_on(thread_id)
            .await
            .map_err(ApiError::db)?,
        SwitchSubject::LucidosAgent => last_turn_settings(&state.pool, thread_id)
            .await
            .map_err(ApiError::db)?,
    };
    switch_estimate(&state.pool, thread_id, subject, current, &target)
        .await
        .map(Json)
        .map_err(ApiError::db)
}
