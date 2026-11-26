use super::{
    error::ApiError,
    permission_requests::{self, Spent},
    AppState,
};
use crate::engine::event_bus::EventBus;
use crate::engine::gateway_control;
use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    method: String,
    path: String,
    #[serde(default)]
    body: Value,
    #[serde(default)]
    reason: String,
}

struct Route {
    method: &'static str,
    path: &'static str,
    change: bool,
    summary: &'static str,
}
const ROUTES: &[Route] = &[
    Route {
        method: "GET",
        path: "/gateway/status",
        change: false,
        summary: "Read gateway status.",
    },
    Route {
        method: "PUT",
        path: "/release-check",
        change: true,
        summary: "Change the machine-wide update channel settings.",
    },
];

impl Request {
    fn checked_route(&self) -> Result<&'static Route, ApiError> {
        let route = ROUTES
            .iter()
            .find(|r| r.method == self.method && r.path == self.path)
            .ok_or_else(|| {
                ApiError::bad_request("Gateway control request method or path is not allowed")
            })?;
        if !route.change && !self.body.is_null() {
            return Err(ApiError::bad_request("Gateway status reads take no body"));
        }
        if route.change && !self.body.is_object() {
            return Err(ApiError::bad_request(
                "Gateway setting changes require a JSON object",
            ));
        }
        Ok(route)
    }

    fn ask(&self, route: &Route) -> Result<permission_requests::Ask, ApiError> {
        fn canonical(value: &Value) -> Value {
            match value {
                Value::Object(map) => {
                    let sorted: std::collections::BTreeMap<_, _> = map.iter().collect();
                    Value::Object(
                        sorted
                            .into_iter()
                            .map(|(k, v)| (k.clone(), canonical(v)))
                            .collect(),
                    )
                }
                Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
                other => other.clone(),
            }
        }
        let exact = serde_json::to_string(&canonical(
            &json!({"method":self.method, "path":self.path,"body":self.body}),
        ))
        .map_err(|_| ApiError::bad_request("Invalid gateway control request"))?;
        permission_requests::gateway_control_ask(&exact, route.summary, &self.reason)
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/engine/gateway-control", post(send))
}

async fn send(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    let caller = permission_requests::calling_agent_thread(&headers)?;
    let port = crate::core::base_path::gateway_port()
        .ok_or_else(|| ApiError::bad_request("Gateway control requests require a gateway"))?;
    execute(&state.engine.event_bus, caller, request, &port).await
}

async fn execute(
    bus: &EventBus,
    caller: uuid::Uuid,
    request: Request,
    port: &str,
) -> Result<Json<Value>, ApiError> {
    let route = request.checked_route()?;
    if route.change {
        match permission_requests::spend_or_prepare(bus, caller, request.ask(route)?).await? {
            Spent::Requested(id) => {
                return Ok(Json(
                    json!({"request_id":id,"next":"Ask your question tool with request_id as the only question. After Allow once, rerun the exact command."}),
                ))
            }
            Spent::Allowed(_) => {}
            Spent::DeviceSessionScope => {
                return Err(ApiError::internal("Unexpected permission scope"))
            }
        }
    }
    let method = reqwest::Method::from_bytes(route.method.as_bytes()).expect("static method");
    let text = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        gateway_control::send_control(port, method, route.path, &request.body),
    )
    .await
    .map_err(|_| {
        ApiError::internal(
            "Gateway outcome uncertain; inspect status before requesting another change",
        )
    })?
    .map_err(|error| match error {
        gateway_control::ControlRefusal::Refused {
            status, message, ..
        } => ApiError::with_code(status, message),
        gateway_control::ControlRefusal::Unreachable(message) => ApiError::internal(message),
    })?;
    let answer: Value = serde_json::from_str(&text)
        .map_err(|_| ApiError::internal("Gateway returned invalid JSON"))?;
    Ok(Json(json!({"response":answer})))
}

#[cfg(test)]
#[path = "gateway_requests_tests.rs"]
mod tests;
