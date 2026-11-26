use axum::{routing::get, Json, Router};
use std::time::Duration;

use super::AppState;

const GATEWAY_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/workspace-label", get(workspace_label))
}

async fn workspace_label() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "label": resolve_label().await }))
}

async fn resolve_label() -> Option<String> {
    let gateway_port = crate::core::base_path::gateway_port()?;
    let id = crate::core::base_path::workspace_id()?;
    let our_port = crate::core::base_path::api_port()?;
    fetch_label(&gateway_port, &id, &our_port).await
}

async fn fetch_label(gateway_port: &str, id: &str, our_port: &str) -> Option<String> {
    fetch_label_within(GATEWAY_TIMEOUT, gateway_port, id, our_port).await
}

async fn fetch_label_within(
    deadline: Duration,
    gateway_port: &str,
    id: &str,
    our_port: &str,
) -> Option<String> {
    match tokio::time::timeout(deadline, ask_gateway(gateway_port, id, our_port)).await {
        Ok(label) => label,
        Err(_) => {
            crate::log_legacy!(
                "[WorkspaceLabel] gateway on :{} did not answer within {:?}; using the engine name",
                gateway_port,
                deadline
            );
            None
        }
    }
}

async fn ask_gateway(gateway_port: &str, id: &str, our_port: &str) -> Option<String> {
    match crate::engine::gateway_control::fetch_control(gateway_port, "/workspaces").await {
        Ok(body) => label_for(&body, id, our_port),
        Err(why) => {
            crate::log_legacy!("[WorkspaceLabel] {}; using the engine name", why);
            None
        }
    }
}

fn label_for(listing: &str, workspace_id: &str, our_port: &str) -> Option<String> {
    let listing = serde_json::from_str::<serde_json::Value>(listing).ok()?;
    let Some(row) = listing
        .get("workspaces")?
        .as_array()?
        .iter()
        .find(|w| w.get("id").and_then(|v| v.as_str()) == Some(workspace_id))
    else {
        crate::log_legacy!(
            "[WorkspaceLabel] gateway does not list '{}'; using the engine name",
            workspace_id
        );
        return None;
    };
    let row_port = match row.get("port")? {
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if row_port != our_port {
        crate::log_legacy!(
            "[WorkspaceLabel] '{}' is on :{} but this engine serves :{}; \
             not our row, using the engine name",
            workspace_id,
            row_port,
            our_port
        );
        return None;
    }
    row.get("name")?
        .as_str()
        .map(str::to_string)
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::gateway_control::test_support::stand_in_gateway;
    use axum::http::StatusCode;

    fn listing() -> &'static str {
        r#"{"workspaces":[
            {"id":"myws","name":"myws","port":5174,"health":"unhealthy","autostart":false},
            {"id":"dev","name":"development","port":5173,"health":"healthy","autostart":true}
        ]}"#
    }

    #[test]
    fn takes_the_display_name_for_our_own_slug() {
        assert_eq!(
            label_for(listing(), "dev", "5173").as_deref(),
            Some("development")
        );
    }

    #[test]
    fn never_answers_with_another_workspaces_name() {
        assert_eq!(label_for(listing(), "other", "5173"), None);
    }

    #[test]
    fn a_malformed_or_empty_listing_is_no_label() {
        for body in ["", "not json", "{}", r#"{"workspaces":"nope"}"#] {
            assert_eq!(label_for(body, "dev", "5173"), None, "body: {body:?}");
        }
    }

    #[tokio::test]
    async fn reaches_a_real_gateway_over_loopback_and_takes_its_own_name() {
        let port = stand_in_gateway(
            r#"{"workspaces":[
                {"id":"other","name":"Other","port":5174},
                {"id":"dev","name":"development","port":5173}
            ]}"#,
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            fetch_label(&port.to_string(), "dev", "5173")
                .await
                .as_deref(),
            Some("development")
        );
    }

    #[tokio::test]
    async fn a_slug_we_inherited_rather_than_earned_gets_no_label() {
        let port = stand_in_gateway(
            r#"{"workspaces":[{"id":"dev","name":"development","port":5173}]}"#,
            StatusCode::OK,
        )
        .await;
        assert_eq!(fetch_label(&port.to_string(), "dev", "5341").await, None);
    }

    #[tokio::test]
    async fn nothing_answering_on_the_port_is_no_label() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        assert_eq!(fetch_label(&port.to_string(), "dev", "5173").await, None);
    }

    #[tokio::test]
    async fn a_gateway_that_refuses_is_no_label() {
        let port = stand_in_gateway("nope", StatusCode::FORBIDDEN).await;
        assert_eq!(fetch_label(&port.to_string(), "dev", "5173").await, None);
    }

    #[tokio::test]
    async fn the_deadline_covers_both_protocol_attempts() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });

        let deadline = Duration::from_secs(1);
        let started = tokio::time::Instant::now();
        assert_eq!(
            fetch_label_within(deadline, &port.to_string(), "dev", "5173").await,
            None
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < deadline * 2,
            "one deadline covers both attempts, took {elapsed:?} against a {deadline:?} budget"
        );
    }

    #[test]
    fn a_row_is_ours_only_when_slug_and_port_both_match() {
        assert_eq!(
            label_for(listing(), "dev", "5173").as_deref(),
            Some("development")
        );
        assert_eq!(label_for(listing(), "dev", "5174"), None);
        assert_eq!(label_for(listing(), "dev", ""), None);
    }

    #[test]
    fn a_row_without_a_usable_name_is_no_label() {
        let bodies = [
            r#"{"workspaces":[{"id":"dev","name":"","port":5173}]}"#,
            r#"{"workspaces":[{"id":"dev","port":5173}]}"#,
            r#"{"workspaces":[{"id":"dev","name":42,"port":5173}]}"#,
        ];
        for body in bodies {
            assert_eq!(label_for(body, "dev", "5173"), None, "body: {body:?}");
        }
    }

    #[test]
    fn a_row_without_a_usable_port_is_no_label() {
        let bodies = [
            r#"{"workspaces":[{"id":"dev","name":"development"}]}"#,
            r#"{"workspaces":[{"id":"dev","name":"development","port":null}]}"#,
        ];
        for body in bodies {
            assert_eq!(label_for(body, "dev", "5173"), None, "body: {body:?}");
        }
    }

    #[test]
    fn a_port_that_is_not_a_number_is_no_label() {
        let body = r#"{"workspaces":[{"id":"dev","name":"development","port":"5173"}]}"#;
        assert_eq!(label_for(body, "dev", "5173"), None);
    }
}
