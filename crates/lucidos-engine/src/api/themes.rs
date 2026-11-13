//! `/api/v1/themes`: list themes, serve one, serve the theme token and theme part
//! catalogs, and resolve a draft theme without saving it.
//!
//! Writes have no route here. A theme is a file under `data/themes/`, so
//! `PUT` / `DELETE /api/v1/data/themes/<id>.json` save and remove one, and
//! `data_api` validates the body before it writes.

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

/// GET /api/v1/themes: every theme, built-ins first, each resolved per mode.
async fn list_themes(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "themes": themes::list(&data_dir(&state)) }))
}

/// GET /api/v1/themes/tokens: every token a theme can tune, with its default per
/// mode and its derivation. What a theme-building plugin lists.
async fn theme_tokens() -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        themes::THEME_TOKEN_CATALOG_JSON,
    )
        .into_response()
}

/// GET /api/v1/themes/parts: every theme part, the properties it takes with
/// their grammar and caps, its part tokens, and the protected surfaces no theme
/// can style (ADR 0307).
async fn theme_parts() -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        themes::THEME_PARTS_CATALOG_JSON,
    )
        .into_response()
}

/// POST /api/v1/themes/resolve: resolve a draft theme without saving it. The
/// body is a theme file. The answer is what the theme would paint, or a 422 with
/// the refusal a save would give. Nothing is written, so an editor such as
/// Theme Studio reuses the engine's merge, grammar and derivation live.
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

/// GET /api/v1/theme?id=: one theme, resolved. 404 for an unknown id, 422 for a
/// workspace file that fails validation, with the reason, and 500 when the
/// file cannot be read.
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
        // A read failure is the engine's; anything else is the file's.
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
