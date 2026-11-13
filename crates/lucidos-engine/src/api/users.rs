//! The users the gateway knows, and the calling device's user (ADR 0623).

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

/// GET /api/v1/users/me: the devices whose reads count as the caller's.
/// `devices` is null when the caller reads as the whole team: a gateway with
/// fewer than two users, or a device no user holds. A page reads its
/// Review group through it.
async fn me(headers: HeaderMap) -> Json<serde_json::Value> {
    let reader = Reader::of(
        crate::api::actor::user_actor(&headers, None).as_ref(),
        &crate::gateway_users::current(),
    );
    Json(serde_json::json!({ "devices": reader.devices() }))
}

/// GET /api/v1/users: every user the gateway knows, and `you`, the
/// caller's user or null. `team` is true with two or more users: only then
/// does a chat send take mentions (ADR 0649).
async fn users(headers: HeaderMap) -> Json<serde_json::Value> {
    let snapshot = crate::gateway_users::current();
    let you = crate::api::actor::user_actor(&headers, None)
        .and_then(|actor| snapshot.user_of_origin(&actor))
        .map(|who| who.id);
    Json(serde_json::json!({
        "users": snapshot.listings(),
        "you": you,
        "team": snapshot.user_count() >= 2,
    }))
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
