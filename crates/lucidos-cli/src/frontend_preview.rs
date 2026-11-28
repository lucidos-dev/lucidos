use crate::http::{client as http_client, ConnectRetry};
use crate::workspace::{BoxError, Workspace};

fn endpoint(ws: &Workspace, suffix: &str) -> String {
    format!("{}/api/v1/frontend-preview{}", ws.base_url(), suffix)
}

pub(crate) fn resolve_thread_id(flag: Option<&str>) -> Result<String, BoxError> {
    if let Some(id) = flag.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(id.to_string());
    }
    match std::env::var("LUCIDOS_THREAD_ID") {
        Ok(id) if !id.trim().is_empty() => Ok(id.trim().to_string()),
        _ => Err("No thread to preview: pass --thread-id <uuid>, or run this inside a coding-agent worktree where $LUCIDOS_THREAD_ID is set.".into()),
    }
}

pub(crate) fn describe(body: &serde_json::Value, origin: Option<&str>) -> String {
    if body.get("running").and_then(|v| v.as_bool()) != Some(true) {
        return "No frontend preview is running.".to_string();
    }
    let thread = body.get("thread_id").and_then(|v| v.as_str());
    let path = body.get("path").and_then(|v| v.as_str());
    let location = match (origin, path) {
        (Some(origin), Some(path)) => format!("{origin}{path}"),
        (None, Some(path)) => {
            format!("{path} (gateway origin unavailable; open through your gateway)")
        }
        (_, None) => "an unreported gateway path".to_string(),
    };
    match thread {
        Some(t) => format!("Frontend preview running for thread {t} at {location}"),
        None => format!("Frontend preview running at {location}"),
    }
}

fn routed_origin(base: &str) -> Option<String> {
    let url = reqwest::Url::parse(base).ok()?;
    // A workspace prefix identifies a routed call; a root URL can be the engine.
    (!url.path().trim_matches('/').is_empty()).then(|| url.origin().ascii_serialization())
}

fn gateway_matches(health: &serde_json::Value, path: &str) -> bool {
    health
        .get("frontend_preview_key")
        .and_then(|key| key.as_str())
        .is_some_and(|key| {
            key.len() == 16
                && key.bytes().all(|b| b.is_ascii_hexdigit())
                && path.starts_with(&format!("/~preview/{key}/"))
        })
}

fn describe_for_workspace(ws: &Workspace, body: &serde_json::Value) -> Result<String, BoxError> {
    if body.get("running").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(describe(body, None));
    }
    if let Some(origin) = ws.api_base_override.as_deref().and_then(routed_origin) {
        return Ok(describe(body, Some(&origin)));
    }
    let client = crate::gateway::client()?;
    let mut ports = crate::gateway::candidate_ports();
    if let Ok(port) = std::env::var("LUCIDOS_GATEWAY_PORT") {
        if let Ok(port) = port.parse::<u16>() {
            if !ports.contains(&port) {
                ports.insert(0, port);
            }
        }
    }
    if let Some(path) = body.get("path").and_then(|v| v.as_str()) {
        for port in ports {
            let origin = lucidos_local_token::client::origin(port);
            let health = client
                .get(format!("{origin}{}", crate::gateway::HEALTH_PATH))
                .send()
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.json::<serde_json::Value>());
            if health
                .as_ref()
                .is_ok_and(|health| gateway_matches(health, path))
            {
                return Ok(describe(body, Some(&origin)));
            }
        }
    }
    Ok(describe(body, None))
}

fn post(ws: &Workspace, suffix: &str, body: serde_json::Value) -> Result<(), BoxError> {
    let url = endpoint(ws, suffix);
    let resp = http_client()?
        .post(&url)
        .json(&body)
        .send_with_connect_retry()
        .map_err(|e| format!("POST {} failed: {}", url, e))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| format!("POST {} returned {}, body read failed: {}", url, status, e))?;
    if !status.is_success() {
        return Err(format!("POST {} returned {}: {}", url, status, text).into());
    }
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("POST {} returned non-JSON body: {}", url, e))?;
    println!("{}", describe_for_workspace(ws, &parsed)?);
    Ok(())
}

pub(crate) fn cmd_start(ws: &Workspace, thread_id: Option<&str>) -> Result<(), BoxError> {
    let thread_id = resolve_thread_id(thread_id)?;
    post(ws, "/start", serde_json::json!({ "thread_id": thread_id }))
}

pub(crate) fn cmd_stop(ws: &Workspace) -> Result<(), BoxError> {
    post(ws, "/stop", serde_json::json!({}))
}

pub(crate) fn cmd_status(ws: &Workspace) -> Result<(), BoxError> {
    let url = endpoint(ws, "");
    let resp = http_client()?
        .get(&url)
        .send_with_connect_retry()
        .map_err(|e| format!("GET {} failed: {}", url, e))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| format!("GET {} returned {}, body read failed: {}", url, status, e))?;
    if !status.is_success() {
        return Err(format!("GET {} returned {}: {}", url, status, text).into());
    }
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("GET {} returned non-JSON body: {}", url, e))?;
    println!("{}", describe_for_workspace(ws, &parsed)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thread_is_the_flag_then_the_ambient_id_then_an_error_naming_the_flag() {
        std::env::set_var("LUCIDOS_THREAD_ID", "11111111-1111-1111-1111-111111111111");
        assert_eq!(
            resolve_thread_id(Some("22222222-2222-2222-2222-222222222222")).unwrap(),
            "22222222-2222-2222-2222-222222222222"
        );
        assert_eq!(
            resolve_thread_id(Some("  ")).unwrap(),
            "11111111-1111-1111-1111-111111111111"
        );

        std::env::remove_var("LUCIDOS_THREAD_ID");
        let err = resolve_thread_id(None).unwrap_err().to_string();
        assert!(err.contains("--thread-id"), "unhelpful error: {err}");
    }

    #[test]
    fn running_preview_uses_the_reached_gateway_origin_and_path() {
        let body = serde_json::json!({
            "running": true, "thread_id": "example-thread",
            "path": "/~preview/0123456789abcdef/demo/",
        });
        for origin in ["https://example.com:7443", "http://[::1]:8443"] {
            let line = describe(&body, Some(origin));
            assert!(line.contains(&format!("{origin}/~preview/0123456789abcdef/demo/")));
            assert!(line.contains("example-thread"));
        }
        assert!(describe(&body, None).contains("gateway origin unavailable"));
    }

    #[test]
    fn engine_override_is_not_a_gateway_origin() {
        assert_eq!(routed_origin("http://127.0.0.1:17173"), None);
        assert_eq!(
            routed_origin("https://example.com:7443/demo@peer/"),
            Some("https://example.com:7443".into())
        );
    }

    #[test]
    fn discovery_matches_identity_instead_of_picking_the_first_gateway() {
        let key = lucidos_gateway_stamp::frontend_preview::key("expected");
        let path = format!("/~preview/{key}/demo/");
        assert!(!gateway_matches(
            &serde_json::json!({"frontend_preview_key":lucidos_gateway_stamp::frontend_preview::key("other")}),
            &path
        ));
        assert!(gateway_matches(
            &serde_json::json!({"frontend_preview_key":key}),
            &path
        ));
        assert!(!gateway_matches(&serde_json::json!({}), &path));
    }

    #[test]
    fn a_stopped_preview_says_so_plainly() {
        assert_eq!(
            describe(&serde_json::json!({ "running": false }), None),
            "No frontend preview is running."
        );
    }
}
