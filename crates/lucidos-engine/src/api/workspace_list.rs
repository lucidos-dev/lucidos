//! `GET /api/v1/workspaces`: every workspace this machine's gateway lists.
//!
//! The engine asks its gateway over loopback ([`super::gateway_control`]), so
//! the listing works for a sandboxed agent that cannot read the local token
//! (ADR 0645). A page served straight off an engine port reads it to reach a
//! peer on its own port, and `lucidos workspaces list` prints it.
//!
//! **It projects, it does not proxy.** Each row carries the slug, display
//! name, folder, engine port, health, autostart, origin and the machine-made
//! flag, and nothing else the gateway knows.
//!
//! Every failure is an error status, never an empty list: an empty list reads
//! as a machine with no workspaces, which hides a broken hop.

use axum::{http::StatusCode, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::error::ApiError;
use super::AppState;

/// Deadline for the whole lookup, both protocol attempts included.
pub(super) const GATEWAY_DEADLINE: Duration = Duration::from_secs(5);

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/workspaces", get(list_workspaces))
}

/// One row of the listing. Deserializing the gateway's row into this type is
/// the projection: serde drops every field not named here.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub(super) struct ListedWorkspace {
    pub(super) id: String,
    pub(super) name: String,
    path: String,
    port: u16,
    health: String,
    autostart: bool,
    /// Who made it, as the gateway's registry records it. An older gateway
    /// sends none, which reads as `unknown`: protected.
    #[serde(default = "unknown_origin")]
    pub(super) origin: String,
    /// The gateway's own verb split. It decides whether an agent may delete
    /// or restore it, and whether stop, start, autostart and rename skip the
    /// card.
    #[serde(default)]
    pub(super) machine_made: bool,
}

fn unknown_origin() -> String {
    "unknown".into()
}

async fn list_workspaces() -> Result<Json<serde_json::Value>, ApiError> {
    let Some(gateway_port) = super::base_path::gateway_port() else {
        return Err(list_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "this engine runs without a gateway, and the gateway keeps the list".to_string(),
        ));
    };
    let workspaces = list_from_gateway(&gateway_port, GATEWAY_DEADLINE).await?;
    Ok(Json(serde_json::json!({ "workspaces": workspaces })))
}

pub(super) async fn list_from_gateway(
    gateway_port: &str,
    deadline: Duration,
) -> Result<Vec<ListedWorkspace>, ApiError> {
    let fetch = super::gateway_control::fetch_control(gateway_port, "/workspaces");
    let body = match tokio::time::timeout(deadline, fetch).await {
        Ok(Ok(body)) => body,
        Ok(Err(why)) => return Err(list_error(StatusCode::BAD_GATEWAY, why)),
        Err(_) => {
            return Err(list_error(
                StatusCode::GATEWAY_TIMEOUT,
                format!("gateway on :{gateway_port} did not answer within {deadline:?}"),
            ))
        }
    };
    project(&body).map_err(|why| list_error(StatusCode::BAD_GATEWAY, why))
}

fn project(listing: &str) -> Result<Vec<ListedWorkspace>, String> {
    #[derive(Deserialize)]
    struct Listing {
        workspaces: Vec<ListedWorkspace>,
    }
    serde_json::from_str::<Listing>(listing)
        .map(|listing| listing.workspaces)
        .map_err(|e| format!("the gateway listing is not one this engine reads ({e})"))
}

fn list_error(status: StatusCode, detail: String) -> ApiError {
    crate::log!("[Workspaces] {}", detail);
    ApiError::new(status, format!("Could not list workspaces: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::super::gateway_control::test_support::stand_in_gateway;
    use super::*;

    #[test]
    fn a_row_carries_only_the_projected_fields() {
        let listing = r#"{"workspaces":[{
            "id":"dev","name":"development","path":"/home/user/workspaces/dev",
            "port":5173,"health":"healthy","autostart":true,
            "origin":"demo","machine_made":true,
            "last_error":"boom","unread_count":4,
            "last_successful_backup":{"at":null,"stale":true,"configured":false}
        }]}"#;
        let rows = project(listing).unwrap();
        let wire = serde_json::to_value(&rows).unwrap();
        assert_eq!(
            wire,
            serde_json::json!([{
                "id": "dev",
                "name": "development",
                "path": "/home/user/workspaces/dev",
                "port": 5173,
                "health": "healthy",
                "autostart": true,
                "origin": "demo",
                "machine_made": true,
            }])
        );
    }

    #[test]
    fn a_listing_we_cannot_read_is_an_error() {
        for body in ["", "not json", "{}", r#"{"workspaces":[{"name":"dev"}]}"#] {
            assert!(project(body).is_err(), "body: {body:?}");
        }
    }

    #[tokio::test]
    async fn reaches_the_gateway_over_loopback() {
        let port = stand_in_gateway(
            r#"{"workspaces":[{"id":"dev","name":"dev","path":"/home/user/workspaces/dev",
                "port":5173,"health":"booting","autostart":false}]}"#,
            StatusCode::OK,
        )
        .await;
        let rows = list_from_gateway(&port.to_string(), GATEWAY_DEADLINE)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].health, "booting");
        assert_eq!(rows[0].origin, "unknown");
        assert!(!rows[0].machine_made);
    }

    /// A refusing or absent gateway must not read as a machine with no
    /// workspaces.
    #[tokio::test]
    async fn a_refusing_or_absent_gateway_is_an_error_not_an_empty_list() {
        let refusing = stand_in_gateway("nope", StatusCode::FORBIDDEN).await;
        let err = list_from_gateway(&refusing.to_string(), GATEWAY_DEADLINE)
            .await
            .unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_GATEWAY);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let absent = listener.local_addr().expect("local addr").port();
        drop(listener);
        let err = list_from_gateway(&absent.to_string(), GATEWAY_DEADLINE)
            .await
            .unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_GATEWAY);
    }
}
