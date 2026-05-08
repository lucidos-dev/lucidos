use super::*;
use crate::engine::ContextSection;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Request body for saving a context
#[derive(Debug, Deserialize)]
pub(super) struct SaveContextRequest {
    pub label: String,
    pub model: Option<String>,
    pub sections: Vec<ContextSection>,
}

/// Summary response for list endpoint (without sections content)
#[derive(Debug, Serialize)]
pub(super) struct SavedContextSummary {
    pub id: Uuid,
    pub label: String,
    pub model: Option<String>,
    pub total_chars: i32,
    pub created_at: DateTime<Utc>,
}

/// Full response for get endpoint (with sections)
#[derive(Debug, Serialize)]
pub(super) struct SavedContextFull {
    pub id: Uuid,
    pub label: String,
    pub model: Option<String>,
    pub total_chars: i32,
    pub sections: Vec<ContextSection>,
    pub created_at: DateTime<Utc>,
}

/// Query params for single saved context
#[derive(Debug, Deserialize)]
pub(super) struct SavedContextQuery {
    pub id: Uuid,
}

/// POST /api/saved-contexts - Save a context
pub(super) async fn save_context(
    State(state): State<AppState>,
    Json(body): Json<SaveContextRequest>,
) -> Response {
    // Calculate total_chars from sections
    let total_chars: usize = body.sections.iter().map(|s| s.char_count).sum();

    // Serialize sections to JSON
    let sections_json = match serde_json::to_value(&body.sections) {
        Ok(json) => json,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    serde_json::json!({ "error": format!("Failed to serialize sections: {}", e) }),
                ),
            )
                .into_response();
        }
    };

    // Insert into database
    let result = sqlx::query_as::<_, (Uuid, String, Option<String>, i32, DateTime<Utc>)>(
        r#"
        INSERT INTO saved_contexts (label, model, total_chars, sections)
        VALUES ($1, $2, $3, $4)
        RETURNING id, label, model, total_chars, created_at
        "#,
    )
    .bind(&body.label)
    .bind(&body.model)
    .bind(total_chars as i32)
    .bind(&sections_json)
    .fetch_one(&state.pool)
    .await;

    match result {
        Ok((id, label, model, total_chars, created_at)) => {
            let summary = SavedContextSummary {
                id,
                label,
                model,
                total_chars,
                created_at,
            };
            Json(summary).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/saved-contexts - List saved contexts (metadata only)
pub(super) async fn list_saved_contexts(State(state): State<AppState>) -> Response {
    let result = sqlx::query_as::<_, (Uuid, String, Option<String>, i32, DateTime<Utc>)>(
        r#"
        SELECT id, label, model, total_chars, created_at
        FROM saved_contexts
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&state.pool)
    .await;

    match result {
        Ok(rows) => {
            let summaries: Vec<SavedContextSummary> = rows
                .into_iter()
                .map(
                    |(id, label, model, total_chars, created_at)| SavedContextSummary {
                        id,
                        label,
                        model,
                        total_chars,
                        created_at,
                    },
                )
                .collect();
            Json(summaries).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/saved-context?id=<uuid> - Get a single saved context with full sections
pub(super) async fn get_saved_context(
    State(state): State<AppState>,
    Query(query): Query<SavedContextQuery>,
) -> Response {
    let result = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            Option<String>,
            i32,
            serde_json::Value,
            DateTime<Utc>,
        ),
    >(
        r#"
        SELECT id, label, model, total_chars, sections, created_at
        FROM saved_contexts
        WHERE id = $1
        "#,
    )
    .bind(query.id)
    .fetch_optional(&state.pool)
    .await;

    match result {
        Ok(Some((id, label, model, total_chars, sections_json, created_at))) => {
            // Deserialize sections from JSONB
            let sections: Vec<ContextSection> = match serde_json::from_value(sections_json) {
                Ok(s) => s,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": format!("Failed to deserialize sections: {}", e) })),
                    ).into_response();
                }
            };

            let full = SavedContextFull {
                id,
                label,
                model,
                total_chars,
                sections,
                created_at,
            };
            Json(full).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Context not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// DELETE /api/saved-context?id=<uuid> - Delete a saved context
pub(super) async fn delete_saved_context(
    State(state): State<AppState>,
    Query(query): Query<SavedContextQuery>,
) -> Json<ApiResult> {
    let result = sqlx::query(
        r#"
        DELETE FROM saved_contexts
        WHERE id = $1
        "#,
    )
    .bind(query.id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(res) => {
            if res.rows_affected() == 0 {
                ApiResult::err("Context not found")
            } else {
                ApiResult::ok()
            }
        }
        Err(e) => ApiResult::err(e.to_string()),
    }
}
