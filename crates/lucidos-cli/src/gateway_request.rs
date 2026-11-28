use crate::{
    gateway, http,
    workspace::{self, BoxError},
};
use reqwest::{blocking::Client, Method};
use serde_json::{json, Value};

pub(crate) struct ControlClient {
    client: Client,
    base: String,
    token: Option<String>,
}

#[derive(Debug)]
pub(crate) struct AwaitingOwner(pub String);
impl std::fmt::Display for AwaitingOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "request_id: {}\nAsk your question tool with this id as the only question. After Allow once, rerun the exact command.", self.0)
    }
}
impl std::error::Error for AwaitingOwner {}

impl ControlClient {
    pub(crate) fn resolve(port: Option<u16>) -> Result<Self, BoxError> {
        let token = lucidos_local_token::read();
        if uses_engine(token.as_deref(), http::has_origin_token(), port)? {
            return Ok(Self {
                client: http::client()?,
                base: workspace::resolve_from_env()?.base_url(),
                token: None,
            });
        }
        let client = gateway::client()?;
        let ports = port
            .map(|p| vec![p])
            .unwrap_or_else(gateway::candidate_ports);
        let found = gateway::find_gateways(&client, &ports);
        let base = gateway::preferred(&found, port.or_else(gateway::dev_port_override))
            .or(found.first())
            .map(|g| g.base.clone())
            .ok_or_else(|| {
                port.map(gateway::no_gateway_on)
                    .unwrap_or_else(|| gateway::NO_GATEWAY.into())
            })?;
        Ok(Self {
            client,
            base,
            token,
        })
    }

    #[cfg(test)]
    pub(crate) fn direct(client: Client, base: &str, token: &str) -> Self {
        Self {
            client,
            base: base.into(),
            token: Some(token.into()),
        }
    }

    pub(crate) fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, BoxError> {
        let response = if let Some(token) = &self.token {
            let request = self
                .client
                .request(method, format!("{}{path}", self.base))
                .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token);
            let request = if let Some(body) = body {
                request.json(&body)
            } else {
                request
            };
            request.send()?
        } else {
            let path = path
                .strip_prefix("/~/api/v1/control")
                .ok_or("Invalid gateway control path")?;
            self.client.post(format!("{}/api/v1/engine/gateway-control",self.base))
                .json(&json!({"method":method.as_str(),"path":path,"body":body,"reason":"Manage the machine-wide update channel settings."}))
                .send()?
        };
        let status = response.status();
        let text = response.text()?;
        if !status.is_success() {
            let answer = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
            return Err(format!(
                "the gateway refused the request: {}",
                answer["error"].as_str().unwrap_or(status.as_str())
            )
            .into());
        }
        let answer: Value = serde_json::from_str(&text)?;
        if self.token.is_some() {
            return Ok(answer);
        }
        if let Some(id) = answer["request_id"].as_str() {
            return Err(Box::new(AwaitingOwner(id.into())));
        }
        answer
            .get("response")
            .cloned()
            .ok_or_else(|| "Engine returned no gateway response".into())
    }
}

fn uses_engine(token: Option<&str>, agent: bool, port: Option<u16>) -> Result<bool, BoxError> {
    if token.is_some() {
        return Ok(false);
    }
    if !agent {
        return Err("no local token found and no Lucidos agent environment; run on the gateway's own machine or inside Lucidos".into());
    }
    if port.is_some() {
        return Err(
            "--port is unavailable through the engine; agents can only contact their own gateway"
                .into(),
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn route_choice_never_probes_for_an_agent_or_uses_its_port() {
        assert!(!uses_engine(Some("token"), true, Some(5999)).unwrap());
        assert!(!uses_engine(Some("token"), false, None).unwrap());
        assert!(uses_engine(None, true, None).unwrap());
        assert!(uses_engine(None, true, Some(5999)).is_err());
        assert!(uses_engine(None, false, None).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn engine_path_preserves_requests_pending_approval_and_responses() {
        use axum::{routing::post, Json, Router};
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        let app = Router::new().route(
            "/api/v1/engine/gateway-control",
            post(move |Json(body): Json<Value>| {
                captured.lock().unwrap().push(body.clone());
                async move {
                    Json(if body["method"] == "GET" {
                        json!({"response":{"release_check":{"channel":"stable"}}})
                    } else {
                        json!({"request_id":"permission-request:fixture"})
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::task::spawn_blocking(move || {
            let control = ControlClient {
                client: http::client().unwrap(),
                base,
                token: None,
            };
            assert_eq!(
                control
                    .send(
                        Method::GET,
                        lucidos_update_channel::GATEWAY_STATUS_PATH,
                        None
                    )
                    .unwrap()["release_check"]["channel"],
                "stable"
            );
            let pending = control
                .send(
                    Method::PUT,
                    lucidos_update_channel::RELEASE_CHECK_PATH,
                    Some(json!({"channel":"local"})),
                )
                .unwrap_err();
            assert_eq!(
                pending.downcast_ref::<AwaitingOwner>().unwrap().0,
                "permission-request:fixture"
            );
        })
        .await
        .unwrap();
        let requests = seen.lock().unwrap();
        assert_eq!(requests[0]["path"], "/gateway/status");
        assert_eq!(requests[1]["body"], json!({"channel":"local"}));
        assert!(requests
            .iter()
            .all(|r| r.get("port").is_none() && r.get("url").is_none()));
        server.abort();
    }
}
