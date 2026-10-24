//! The main check's routes (ADR 0464): read where it stands, and close a red.

use super::actor::require_owner_device;
use super::*;
use crate::engine::thread_events::MessageOrigin;
use crate::engine::MainCheckCloseRefusal;
use axum::http::HeaderMap;
use sqlx::PgPool;

/// GET /api/v1/main-check: the main check as it stands. The same snapshot
/// rides `GET /api/v1/changes` and every `ChangesUpdated` frame.
pub(super) async fn get_main_check(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!(state.engine.main_check_snapshot()))
}

/// POST /api/v1/main-check/close: the maintainer closes a red main check. Its
/// fix thread stays an ordinary thread. What landed while red was open starts
/// the next check. An agent, a script or an app may not close it: the fix
/// thread owns a red until its change lands.
pub(super) async fn close_main_check(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = require_maintainer(&headers, &state.pool).await?;
    let answer = state
        .engine
        .close_main_check_red(Some(actor))
        .await
        .map_err(ApiError::internal)?;
    close_answer(answer)
}

/// The owner at a registered device, and nobody else. The machine-local token
/// and a verified agent subprocess are real credentials but not the
/// maintainer, so each gets the main check's own `403`.
async fn require_maintainer(headers: &HeaderMap, pool: &PgPool) -> Result<MessageOrigin, ApiError> {
    require_owner_device(headers, pool)
        .await
        .map_err(|refusal| {
            if refusal.status == StatusCode::FORBIDDEN {
                maintainer_only()
            } else {
                refusal
            }
        })
}

fn maintainer_only() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "Only the maintainer closes a red main check, from the Lucidos UI. Its fix thread owns it until a fix lands.",
    )
    .with_reason("maintainer_only")
}

/// What Close answers once the maintainer asked. Pure, so every answer's
/// status and slug are pinned.
fn close_answer(
    answer: Result<(), MainCheckCloseRefusal>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match answer {
        Ok(()) => Ok(Json(
            serde_json::json!({ "message": "Closed the red main check." }),
        )),
        Err(MainCheckCloseRefusal::NoRed) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "Main is not red, so there is nothing to close.",
        )
        .with_reason("main_check_not_red")),
        Err(MainCheckCloseRefusal::StateUnread) => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The main check could not read its state back after a restart. Try again shortly.",
        )
        .with_reason("main_check_state_unread")),
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/main-check", get(get_main_check))
        .route("/main-check/close", post(close_main_check))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::actor::{
        init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
        HEADER_DEVICE_ID,
    };
    use uuid::Uuid;

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
    fn close_answers_each_case_with_its_own_status() {
        let body = close_answer(Ok(())).expect("closed").0;
        assert_eq!(body["message"], "Closed the red main check.");

        let not_red = close_answer(Err(MainCheckCloseRefusal::NoRed)).expect_err("refused");
        assert_eq!(not_red.status, StatusCode::CONFLICT);
        assert_eq!(not_red.reason, Some("main_check_not_red"));

        let unread = close_answer(Err(MainCheckCloseRefusal::StateUnread)).expect_err("refused");
        assert_eq!(unread.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(unread.reason, Some("main_check_state_unread"));
    }

    /// ADR 0464: only the maintainer closes a red. The machine-local token is
    /// what a script holds, and it once passed as an engine-mode API caller.
    #[tokio::test]
    async fn only_the_owners_device_may_close_a_red() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        crate::core::DeviceStore::register(
            &pool,
            &bus,
            "owner-phone",
            Some("Mozilla/5.0"),
            None,
            None,
        )
        .await
        .unwrap();

        let script = headers_with(&[(
            lucidos_local_token::HEADER_LOCAL_TOKEN,
            crate::api::local_auth::publish_test_local_token(),
        )]);
        let refusal = require_maintainer(&script, &pool)
            .await
            .expect_err("a script");
        assert_eq!(refusal.status, StatusCode::FORBIDDEN);
        assert_eq!(refusal.reason, Some("maintainer_only"));

        init_agent_origin_secret("main-check-close-secret".into());
        let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None).unwrap();
        let agent = headers_with(&[
            (HEADER_AGENT_ORIGIN_TOKEN, &token),
            (HEADER_DEVICE_ID, "owner-phone"),
        ]);
        let refusal = require_maintainer(&agent, &pool)
            .await
            .expect_err("an agent");
        assert_eq!(refusal.status, StatusCode::FORBIDDEN);
        assert_eq!(refusal.reason, Some("maintainer_only"));

        let owner = headers_with(&[(HEADER_DEVICE_ID, "owner-phone")]);
        let actor = require_maintainer(&owner, &pool)
            .await
            .expect("the owner's device");
        assert!(matches!(actor, MessageOrigin::Device { .. }));

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
