use axum::{http::HeaderMap, routing::get, Json, Router};

use super::AppState;
use crate::gateway_users::Reader;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/users/me", get(me))
        .route("/users", get(users))
        // Older pages use these paths. See "User rename wire aliases" in
        // docs/temporary-measures.md.
        .route("/operators/me", get(me))
        .route("/operators/people", get(legacy_users))
}

async fn me(headers: HeaderMap) -> Json<serde_json::Value> {
    Json(reader_devices(&headers))
}

pub(super) fn reader_devices(headers: &HeaderMap) -> serde_json::Value {
    let reader = Reader::of(
        crate::api::actor::user_actor(headers, None).as_ref(),
        &crate::gateway_users::current(),
    );
    serde_json::json!({ "devices": reader.devices() })
}

async fn users(headers: HeaderMap) -> Json<serde_json::Value> {
    Json(user_list(&headers))
}

pub(super) fn user_list(headers: &HeaderMap) -> serde_json::Value {
    let snapshot = crate::gateway_users::current();
    let you = crate::api::actor::user_actor(headers, None)
        .and_then(|actor| snapshot.user_of_origin(&actor))
        .map(|who| who.id);
    serde_json::json!({
        "users": snapshot.listings(),
        "you": you,
        "team": snapshot.user_count() >= 2,
    })
}

async fn legacy_users(headers: HeaderMap) -> Json<serde_json::Value> {
    let Json(mut body) = users(headers).await;
    if let Some(users) = body.as_object_mut().and_then(|body| body.remove("users")) {
        body["people"] = users;
    }
    Json(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_older_page_gets_the_user_list_under_its_original_key() {
        let Json(current) = users(HeaderMap::new()).await;
        let Json(legacy) = legacy_users(HeaderMap::new()).await;
        assert_eq!(legacy["people"], current["users"]);
        assert!(legacy.get("users").is_none());
        assert!(current.get("people").is_none());
        assert_eq!(legacy["team"], current["team"]);
        assert_eq!(legacy["you"], current["you"]);
    }
}
