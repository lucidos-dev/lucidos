use axum::{
    body::Bytes,
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use super::error::ApiError;
use super::AppState;
use crate::core::{themes, workspace_fonts};

fn data_dir(state: &AppState) -> std::path::PathBuf {
    state.workspace_path.join(crate::core::DATA_DIR)
}

async fn list_themes(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "themes": themes::list(&data_dir(&state)) }))
}

async fn theme_tokens() -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        themes::THEME_TOKEN_CATALOG_JSON,
    )
        .into_response()
}

async fn theme_parts() -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        themes::THEME_PARTS_CATALOG_JSON,
    )
        .into_response()
}

async fn resolve_theme(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let refused =
        |e: Box<dyn std::error::Error + Send + Sync>| ApiError::with_code(422, e.to_string());
    let definition = themes::parse_definition(&body).map_err(refused)?;
    let installed = workspace_fonts::list(&data_dir(&state));
    themes::check_installed_fonts(&definition.fonts, &installed).map_err(refused)?;
    Ok(Json(serde_json::json!({
        "modes": definition.modes(),
        "resolved": themes::resolve(&definition, &installed),
    })))
}

#[derive(Debug, Deserialize)]
struct ThemeQuery {
    id: String,
}

async fn get_theme(
    State(state): State<AppState>,
    Query(query): Query<ThemeQuery>,
) -> Result<Json<themes::Theme>, ApiError> {
    themes::validate_id(&query.id).map_err(|e| ApiError::bad_request(e.to_string()))?;
    match themes::get(&data_dir(&state), &query.id) {
        Ok(Some(theme)) => Ok(Json(theme)),
        Ok(None) => Err(ApiError::not_found(format!(
            "theme '{}' not found",
            query.id
        ))),
        Err(e) if e.is::<std::io::Error>() => Err(ApiError::internal(e.to_string())),
        Err(e) => Err(ApiError::with_code(422, e.to_string())),
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/themes", get(list_themes))
        .route("/themes/tokens", get(theme_tokens))
        .route("/themes/parts", get(theme_parts))
        .route("/themes/resolve", post(resolve_theme))
        .route("/theme", get(get_theme))
}
