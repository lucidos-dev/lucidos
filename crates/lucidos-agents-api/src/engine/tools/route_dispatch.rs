use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use serde_json::{Map, Value};
use tower::ServiceExt as _;

use super::ToolOutcome;
use crate::capability_manifest::{domains, Arg, ArgIn, ArgType, Domain, LlmExecution, Method};
use crate::core::subprocess_origin::{
    subprocess_origin_env_vars, ENV_AGENT_ORIGIN_TOKEN, HEADER_AGENT_ORIGIN_TOKEN,
};
use crate::engine::LucidosEngine;

const ANSWER_LIMIT: usize = 4 * 1024 * 1024;

pub(crate) fn route_backed_domain(tool_name: &str) -> Option<&'static Domain> {
    domains()
        .iter()
        .find(|d| d.llm && d.tool_name == tool_name && d.llm_execution == LlmExecution::Route)
}

#[derive(Debug, PartialEq)]
pub(crate) struct RouteRequest {
    pub(crate) method: Method,
    pub(crate) uri: String,
    pub(crate) body: Option<Value>,
}

pub(crate) fn route_request(domain: &Domain, args: &Value) -> Result<RouteRequest, String> {
    let action = args["action"].as_str().unwrap_or("").trim();
    let op = domain
        .operations
        .iter()
        .find(|op| op.action == action && op.on_llm(domain))
        .ok_or_else(|| unknown_action(domain, action))?;
    let mut path = op.path.to_string();
    let mut query: Vec<String> = Vec::new();
    let mut body = Map::new();
    for arg in op.args {
        match arg.loc {
            ArgIn::Path => {
                let value = scalar(arg, &args[arg.name])?
                    .ok_or_else(|| format!("Error: {} is required.", arg.name))?;
                path = path.replace(&format!(":{}", arg.name), &urlencoding::encode(&value));
            }
            ArgIn::Query => {
                if let Some(value) = scalar(arg, &args[arg.name])? {
                    query.push(format!("{}={}", arg.name, urlencoding::encode(&value)));
                }
            }
            ArgIn::Body => {
                if let Some(value) = args.get(arg.name) {
                    body.insert(arg.name.to_string(), body_value(arg, value)?);
                }
            }
            ArgIn::FixedBody(literal) => {
                let value = serde_json::from_str(literal)
                    .map_err(|e| format!("Error: {}'s fixed value is not JSON: {e}", arg.name))?;
                body.insert(arg.name.to_string(), value);
            }
        }
    }
    let uri = match query.is_empty() {
        true => path,
        false => format!("{path}?{}", query.join("&")),
    };
    Ok(RouteRequest {
        method: op.method,
        uri,
        body: (op.method != Method::Get).then_some(Value::Object(body)),
    })
}

fn unknown_action(domain: &Domain, action: &str) -> String {
    let actions = domain.actions().join(", ");
    match action {
        "" => format!("Error: action is required (one of: {actions})."),
        other => format!("Error: unknown action '{other}'. Use one of: {actions}."),
    }
}

fn scalar(arg: &Arg, value: &Value) -> Result<Option<String>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text.clone())),
        Value::Bool(_) | Value::Number(_) => Ok(Some(value.to_string())),
        other => Err(format!("Error: {} takes one value, not {other}.", arg.name)),
    }
}

fn body_value(arg: &Arg, value: &Value) -> Result<Value, String> {
    match (arg.ty, value) {
        (ArgType::Json, Value::String(raw)) => serde_json::from_str(raw)
            .map_err(|e| format!("Error: {} must be valid JSON: {e}", arg.name)),
        _ => Ok(value.clone()),
    }
}

pub(crate) fn origin_headers(thread_id: uuid::Uuid) -> HeaderMap {
    let trigger = crate::scheduler::user_tasks::current_trigger_id();
    let depth = crate::scheduler::user_tasks::current_event_trigger_depth();
    let mut headers = HeaderMap::new();
    let token = subprocess_origin_env_vars(Some(thread_id), depth, trigger.as_deref())
        .into_iter()
        .find(|(name, _)| *name == ENV_AGENT_ORIGIN_TOKEN)
        .and_then(|(_, token)| HeaderValue::from_str(&token).ok());
    if let Some(token) = token {
        headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token);
    }
    headers
}

pub(crate) fn tool_result(request: &RouteRequest, status: StatusCode, answer: &str) -> ToolOutcome {
    let answer = answer.trim();
    let json: Value = serde_json::from_str(answer).unwrap_or_default();
    let refused = json["success"] == Value::Bool(false);
    if status.is_success() && !refused {
        return Ok(match answer.is_empty() {
            true => "[ACTION COMPLETED]".to_string(),
            false => answer.to_string(),
        });
    }
    if status == StatusCode::FORBIDDEN
        && json["reason"] == crate::api::permission_requests::PERMISSION_REQUIRED
    {
        if let Some(request_id) = json["request_id"].as_str() {
            return Err(permission_text(request_id));
        }
    }
    let error = json["error"].as_str().unwrap_or(answer);
    Err(format!(
        "Error: {} /api/v1{} returned {status}: {error}",
        request.method.as_str(),
        request.uri
    ))
}

fn permission_text(request_id: &str) -> String {
    format!(
        "Error: Nothing changed: this needs the owner's permission. Call {} with one \
         question whose text is exactly `{request_id}`, and the owner sees the engine's \
         card. Once they allow it, run this same action again.",
        crate::llm::tool_names::ASK_USER_QUESTION
    )
}

fn http_request(request: &RouteRequest, headers: HeaderMap) -> Result<Request<Body>, String> {
    let body = match &request.body {
        Some(body) => Body::from(body.to_string()),
        None => Body::empty(),
    };
    let mut built = Request::builder()
        .method(request.method.as_str())
        .uri(&request.uri)
        .body(body)
        .map_err(|e| format!("Error: could not build the request: {e}"))?;
    *built.headers_mut() = headers;
    if request.body.is_some() {
        built.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    Ok(built)
}

pub(crate) async fn send(
    router: axum::Router,
    request: &RouteRequest,
    headers: HeaderMap,
) -> ToolOutcome {
    let response = router
        .oneshot(http_request(request, headers)?)
        .await
        .map_err(|e| format!("Error: the route failed: {e}"))?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), ANSWER_LIMIT)
        .await
        .map_err(|e| format!("Error: could not read the route's answer: {e}"))?;
    tool_result(request, status, &String::from_utf8_lossy(&bytes))
}

#[easy_ext::ext(RouteDispatchExt)]
pub(crate) impl LucidosEngine {
    async fn execute_route_backed(
        &self,
        domain: &Domain,
        args: &Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let request = route_request(domain, args)?;
        let router = self
            .api_router()
            .ok_or("Error: the engine's router is not built yet.")?;
        send(router, &request, origin_headers(thread_id)).await
    }
}

#[cfg(test)]
#[path = "route_dispatch_tests.rs"]
mod tests;
