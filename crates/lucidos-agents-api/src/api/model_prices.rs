//! `GET /api/v1/model-prices`: the *price table*, the shipped cards merged
//! with this workspace's override, each card with its origin (ADR 0525).

use std::sync::Arc;

use axum::{extract::State, routing::get, Json, Router};

use super::AppState;
use crate::llm::model_prices::{price_table_for, PriceTable};

async fn model_prices_route(State(state): State<AppState>) -> Json<Arc<PriceTable>> {
    Json(price_table_for(&state.workspace_path))
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/model-prices", get(model_prices_route))
}
