use lucidos_update_channel::{
    Channel, CustomName, UpdateAddress, GATEWAY_STATUS_PATH, RELEASE_CHECK_PATH,
};
use serde_json::{json, Value};

#[cfg(test)]
use crate::gateway;
use crate::gateway_request::{AwaitingOwner, ControlClient};
use crate::workspace::BoxError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Show,
    List,
    Set(String),
    Add {
        name: String,
        url: String,
        select: bool,
    },
    Remove(String),
}

pub fn cmd_update_channel(action: &Action, port: Option<u16>) -> Result<u8, BoxError> {
    let body = request_body(action)?;
    let control = ControlClient::resolve(port)?;
    let snapshot = match exchange_control(&control, body) {
        Ok(snapshot) => snapshot,
        Err(error) if error.downcast_ref::<AwaitingOwner>().is_some() => {
            println!("{error}");
            return Ok(3);
        }
        Err(error) => return Err(error),
    };
    println!(
        "{}",
        match action {
            Action::List => describe_list(&snapshot)?,
            _ => describe(&snapshot)?,
        }
    );
    Ok(0)
}

fn exchange_control(control: &ControlClient, body: Option<Value>) -> Result<Value, BoxError> {
    let status = control.send(reqwest::Method::GET, GATEWAY_STATUS_PATH, None)?;
    let snapshot = status.get("release_check").cloned().unwrap_or(Value::Null);
    require_channels(&snapshot)?;
    let Some(body) = body else {
        return Ok(snapshot);
    };
    let snapshot = control.send(reqwest::Method::PUT, RELEASE_CHECK_PATH, Some(body))?;
    require_channels(&snapshot)?;
    Ok(snapshot)
}

#[cfg(test)]
fn exchange(
    client: &reqwest::blocking::Client,
    base: &str,
    token: &str,
    body: Option<Value>,
) -> Result<Value, BoxError> {
    exchange_control(&ControlClient::direct(client.clone(), base, token), body)
}

fn request_body(action: &Action) -> Result<Option<Value>, BoxError> {
    Ok(match action {
        Action::Show | Action::List => None,
        Action::Set(raw) => Some(json!({ "channel": Channel::parse(raw)?.as_str() })),
        Action::Add { name, url, select } => {
            let name = CustomName::parse(name)?;
            let address =
                UpdateAddress::parse(url)?.ok_or("a custom slot needs a latest.json URL")?;
            let mut body =
                json!({ "custom_upsert": { "name": name.as_str(), "url": address.as_str() } });
            if *select {
                body["channel"] = json!(Channel::Custom(name).as_str());
            }
            Some(body)
        }
        Action::Remove(name) => Some(json!({ "custom_remove": CustomName::parse(name)?.as_str() })),
    })
}

fn require_channels(snapshot: &Value) -> Result<&str, BoxError> {
    snapshot
        .get("channel")
        .and_then(Value::as_str)
        .filter(|_| snapshot.get("channels").and_then(Value::as_array).is_some())
        .ok_or_else(|| "this gateway does not support update channels. Update Lucidos first".into())
}

fn describe(snapshot: &Value) -> Result<String, BoxError> {
    let channel = require_channels(snapshot)?;
    let mut lines = vec![format!("Update channel: {channel}")];
    if let Some(address) = snapshot["update_address"].as_str() {
        lines.push(format!("Update address: {address}"));
    }
    if let Some(error) = snapshot["channel_error"]
        .as_str()
        .or_else(|| snapshot["last_error"].as_str())
    {
        lines.push(format!("Update checks failed: {error}"));
    }
    if snapshot["ahead_of_channel"].as_bool() == Some(true) {
        let current = snapshot["current_version"]
            .as_str()
            .ok_or("gateway omitted the running version")?;
        let offered = snapshot["channel_version"]
            .as_str()
            .ok_or("gateway omitted the channel version")?;
        lines.push(format!(
            "Running {current} is ahead of {channel} ({offered}). No downgrade is offered."
        ));
    } else if let Some(version) = snapshot["channel_version"].as_str() {
        lines.push(format!("Channel version: {version}"));
    }
    if let Some(dir) = snapshot["local_channel_dir"].as_str() {
        lines.push(format!("Local staging directory: {dir}"));
    }
    Ok(lines.join("\n"))
}

fn describe_list(snapshot: &Value) -> Result<String, BoxError> {
    let selected = require_channels(snapshot)?;
    let mut lines = vec![];
    for entry in snapshot["channels"].as_array().unwrap() {
        let channel = entry["channel"]
            .as_str()
            .ok_or("gateway omitted a channel name")?;
        let marker = if channel == selected { "*" } else { " " };
        let availability = if entry["available"].as_bool() == Some(true) {
            "available".to_string()
        } else {
            format!(
                "unavailable: {}",
                entry["reason"].as_str().unwrap_or("no reason reported")
            )
        };
        lines.push(format!("{marker} {channel}: {availability}"));
    }
    if let Some(slots) = snapshot["custom"].as_array() {
        for slot in slots {
            let name = slot["name"]
                .as_str()
                .ok_or("gateway omitted a custom slot name")?;
            let url = slot["url"]
                .as_str()
                .ok_or("gateway omitted a custom slot URL")?;
            let channel = Channel::Custom(CustomName::parse(name)?).to_string();
            let marker = if channel == selected { "*" } else { " " };
            lines.push(format!("{marker} {channel}: {url}"));
        }
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_status_shows_a_cached_poll_failure() {
        let text = describe(&json!({
            "channel": "stable", "channels": [], "channel_error": null,
            "last_error": "Check origin unavailable"
        }))
        .unwrap();
        assert!(text.contains("Check origin unavailable"), "{text}");
    }

    #[test]
    fn channel_status_names_refusal_and_ahead_versions() {
        let text = describe(&json!({
            "channel": "beta", "channels": [], "current_version": "1.2.0",
            "channel_version": "1.1.0", "ahead_of_channel": true,
            "channel_error": "Beta requires the packaged macOS app"
        }))
        .unwrap();
        assert!(text.contains("beta"), "{text}");
        assert!(text.contains("1.2.0") && text.contains("1.1.0"), "{text}");
        assert!(text.contains("ahead"), "{text}");
        assert!(
            text.contains("Beta requires the packaged macOS app"),
            "{text}"
        );
    }

    #[test]
    fn reads_and_partial_writes() {
        assert_eq!(request_body(&Action::Show).unwrap(), None);
        assert_eq!(request_body(&Action::List).unwrap(), None);
        assert_eq!(
            request_body(&Action::Set("stable".into())).unwrap(),
            Some(json!({"channel":"stable"}))
        );
        assert_eq!(
            request_body(&Action::Remove("Preview".into())).unwrap(),
            Some(json!({"custom_remove":"Preview"}))
        );
        for select in [false, true] {
            let body = request_body(&Action::Add {
                name: "Preview".into(),
                url: " https://example.com/latest.json ".into(),
                select,
            })
            .unwrap()
            .unwrap();
            assert_eq!(
                body["custom_upsert"],
                json!({"name":"Preview","url":"https://example.com/latest.json"})
            );
            assert_eq!(
                body.get("channel").and_then(Value::as_str),
                select.then_some("custom:Preview")
            );
            assert!(body.get("enabled").is_none());
        }
    }

    #[test]
    fn invalid_inputs_fail_before_discovery() {
        for action in [
            Action::Set("unknown".into()),
            Action::Set("custom:bad/name".into()),
            Action::Remove("".into()),
            Action::Add {
                name: "bad/name".into(),
                url: "https://example.com/latest.json".into(),
                select: false,
            },
            Action::Add {
                name: "Preview".into(),
                url: "http://example.com/latest.json".into(),
                select: false,
            },
            Action::Add {
                name: "Preview".into(),
                url: " ".into(),
                select: true,
            },
        ] {
            assert!(request_body(&action).is_err(), "{action:?}");
        }
    }

    #[test]
    fn list_preserves_availability_reasons_and_named_slots() {
        let text = describe_list(&json!({
            "channel":"custom:Preview",
            "channels":[
                {"channel":"stable","available":true},
                {"channel":"beta","available":false,"reason":"requires the packaged macOS app"},
                {"channel":"nightly","available":false,"reason":"not published"}
            ],
            "custom":[{"name":"Preview","url":"https://example.com/latest.json"}]
        }))
        .unwrap();
        assert!(
            text.contains("beta: unavailable: requires the packaged macOS app"),
            "{text}"
        );
        assert!(
            text.contains("nightly: unavailable: not published"),
            "{text}"
        );
        assert!(
            text.contains("* custom:Preview: https://example.com/latest.json"),
            "{text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gateway_exchange_authenticates_reads_and_writes_and_keeps_refusals() {
        use axum::{
            extract::State,
            http::HeaderMap,
            routing::{get, put},
            Json, Router,
        };
        use std::sync::{Arc, Mutex};
        async fn status(headers: HeaderMap, State(snapshot): State<Value>) -> Json<Value> {
            assert_eq!(
                headers[lucidos_local_token::HEADER_LOCAL_TOKEN],
                "fixture-token"
            );
            Json(json!({"release_check": snapshot}))
        }
        let writes = Arc::new(Mutex::new(Vec::new()));
        let written = writes.clone();
        let app = Router::new()
            .route(GATEWAY_STATUS_PATH, get(status))
            .route(
                RELEASE_CHECK_PATH,
                put(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let written = written.clone();
                    async move {
                        assert_eq!(
                            headers[lucidos_local_token::HEADER_LOCAL_TOKEN],
                            "fixture-token"
                        );
                        written.lock().unwrap().push(body);
                        (
                            axum::http::StatusCode::BAD_REQUEST,
                            Json(json!({"error":"this channel needs the packaged macOS app"})),
                        )
                    }
                }),
            )
            .with_state(json!({"channel":"stable","channels":[]}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = tokio::task::spawn_blocking(move || {
            let client = gateway::client().unwrap();
            let read = exchange(
                &client,
                &base,
                "fixture-token",
                request_body(&Action::List).unwrap(),
            )
            .unwrap();
            assert_eq!(read["channel"], "stable");
            exchange(
                &client,
                &base,
                "fixture-token",
                request_body(&Action::Set("beta".into())).unwrap(),
            )
            .unwrap_err()
            .to_string()
        })
        .await
        .unwrap();
        server.abort();
        assert!(
            result.contains("this channel needs the packaged macOS app"),
            "{result}"
        );
        assert_eq!(*writes.lock().unwrap(), vec![json!({"channel":"beta"})]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn old_gateway_never_receives_a_write() {
        use axum::{routing::get, Json, Router};
        let app = Router::new().route(
            GATEWAY_STATUS_PATH,
            get(|| async { Json(json!({"release_check":{"update_address":null}})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = tokio::task::spawn_blocking(move || {
            exchange(
                &gateway::client().unwrap(),
                &base,
                "fixture-token",
                request_body(&Action::Set("stable".into())).unwrap(),
            )
            .unwrap_err()
            .to_string()
        })
        .await
        .unwrap();
        server.abort();
        assert!(result.contains("Update Lucidos first"), "{result}");
    }

    #[test]
    fn old_gateways_are_refused() {
        for snapshot in [
            Value::Null,
            json!({"update_address":null}),
            json!({"channel":"stable"}),
        ] {
            assert!(describe(&snapshot)
                .unwrap_err()
                .to_string()
                .contains("Update Lucidos"));
            assert!(describe_list(&snapshot).is_err());
        }
    }
}
