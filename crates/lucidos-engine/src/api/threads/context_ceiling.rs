//! `PUT /api/v1/threads/:thread_id/context-ceiling`: set a thread's *context
//! ceiling* (ADR 0629), `{"ceiling": "200k"}`. It records `ContextCeilingSet`
//! under the caller and answers the ceiling now in force. The engine decides
//! who may set what (`context_ceiling::set_ceiling`).

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::context_ceiling::{
    refuse_trigger_ceiling, set_ceiling, CeilingRefusal, CeilingSet, CeilingSetter, ContextCeiling,
};
use crate::engine::thread_events::EventMeta;

#[derive(serde::Deserialize)]
pub(in crate::api) struct ContextCeilingBody {
    ceiling: String,
}

#[derive(serde::Serialize)]
pub(in crate::api) struct ContextCeilingResponse {
    ceiling: ContextCeiling,
}

/// Who a request sets a ceiling as: an agent, from the thread its origin
/// token names, or a person.
pub(in crate::api) fn ceiling_setter(headers: &HeaderMap) -> CeilingSetter {
    let origin = crate::api::actor::user_actor(headers, None);
    if !super::super::intelligence_ladder::is_agent_caller(headers, origin.as_ref()) {
        return CeilingSetter::Person;
    }
    CeilingSetter::Agent(match crate::api::actor::subprocess_origin(headers) {
        crate::api::actor::SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => source_thread_id,
        crate::api::actor::SubprocessOrigin::NotSubprocess => None,
    })
}

fn forbidden(refusal: CeilingRefusal) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, refusal.message).with_reason(refusal.reason)
}

/// Refuse a trigger ceiling the caller may not write, for the trigger routes.
pub(in crate::api) async fn refuse_agent_trigger_raise(
    state: &AppState,
    headers: &HeaderMap,
    ceiling: Option<ContextCeiling>,
) -> Result<(), String> {
    let Some(ceiling) = ceiling else {
        return Ok(());
    };
    match refuse_trigger_ceiling(&state.pool, ceiling_setter(headers), ceiling).await {
        Some(refusal) => Err(refusal.message),
        None => Ok(()),
    }
}

pub(in crate::api) async fn set_context_ceiling(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<ContextCeilingBody>, JsonRejection>,
) -> Result<Json<ContextCeilingResponse>, ApiError> {
    let thread_id = Uuid::parse_str(&thread_id)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))?;
    let Json(body) = body.map_err(|e| {
        ApiError::bad_request(format!("The body must be {{\"ceiling\": \"200k\"}}: {e}"))
    })?;
    let ceiling = ContextCeiling::parse(&body.ceiling).map_err(ApiError::bad_request)?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let outcome = set_ceiling(
        &state.engine.event_bus,
        ceiling_setter(&headers),
        thread_id,
        ceiling,
        EventMeta::with_actor(Some(actor)),
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    match outcome {
        CeilingSet::Set => Ok(Json(ContextCeilingResponse { ceiling })),
        CeilingSet::Refused(refusal) => Err(forbidden(refusal)),
        CeilingSet::NoThread => Err(ApiError::not_found(format!(
            "Thread {thread_id} does not exist"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::actor::{
        init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
        HEADER_DEVICE_ID,
    };

    /// Both ceiling routes ask this who calls: a person may raise and
    /// withdraw, an agent may not (ADR 0629).
    #[test]
    fn a_subprocess_sets_as_an_agent_and_a_device_as_a_person() {
        init_agent_origin_secret("context-ceiling-test-secret".to_string());
        let thread = Uuid::new_v4();
        let token =
            mint_agent_origin_token(Some(thread), 0, None).expect("the secret is installed above");
        let mut agent = HeaderMap::new();
        agent.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
        assert_eq!(ceiling_setter(&agent), CeilingSetter::Agent(Some(thread)));

        let mut device = HeaderMap::new();
        device.insert(HEADER_DEVICE_ID, "device-abc".parse().unwrap());
        assert_eq!(ceiling_setter(&device), CeilingSetter::Person);
    }
}
