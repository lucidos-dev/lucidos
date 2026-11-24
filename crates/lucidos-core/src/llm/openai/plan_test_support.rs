#![cfg(any(test, feature = "test-support"))]

use super::OpenAiProvider;

const SOL: &str = "gpt-6.1-sol";

/// A token source that hands out `plan-token-1`, and `plan-token-2` after
/// a rejection.
#[derive(Default)]
pub(super) struct FakeTokens {
    pub(super) rejections: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl crate::llm::chatgpt_plan::ChatGptPlanTokens for FakeTokens {
    async fn access_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok("plan-token-1".to_string())
    }

    async fn after_rejection(
        &self,
        rejected: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        self.rejections.lock().unwrap().push(rejected.to_string());
        Ok("plan-token-2".to_string())
    }
}

/// Fields the plan route refuses or its docs say to omit, plus the two
/// Lucidos sends on the platform that the probes saw refused. A pinned
/// wire contract: the literals are the docs' list, less `max_tool_calls`.
/// That one also names a preference, which ADR 0368 keeps to the catalog,
/// and no route sends it.
const PLAN_FORBIDDEN_FIELDS: &[&str] = &[
    "background",
    "conversation",
    "max_output_tokens",
    "metadata",
    "moderation",
    "multi_agent",
    "prompt",
    "prompt_cache_retention",
    "safety_identifier",
    "temperature",
    "top_logprobs",
    "top_p",
    "truncation",
    "user",
    "previous_response_id",
    "service_tier",
    "prompt_cache_options",
];

/// The `unsupported_capability` answer a strict mock gives a body the
/// plan route would refuse, or `None` for a body it accepts.
pub(super) fn plan_refusal(body: &serde_json::Value) -> Option<String> {
    if !body["input"].is_array() {
        return Some(r#"{"detail":"Input must be a list"}"#.into());
    }
    let refused = PLAN_FORBIDDEN_FIELDS
        .iter()
        .find(|field| body.get(**field).is_some())
        .map(|field| field.to_string())
        .or_else(|| {
            body.to_string()
                .contains("prompt_cache_breakpoint")
                .then(|| "prompt_cache_breakpoint".to_string())
        })
        .or_else(|| {
            (body["store"] != false || body["stream"] != true).then(|| "store".to_string())
        })?;
    Some(format!(
        r#"{{"error":{{"message":"unsupported","type":"invalid_request_error","param":"{refused}","code":"subscription_sharing_unsupported_capability"}}}}"#
    ))
}

type PlanRequests = std::sync::Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>;

/// The plan's model listing, in its own shape: `SOL` listed, one hidden.
pub(super) const PLAN_MODELS: &str = r#"{"models":[{"slug":"gpt-6.1-sol","visibility":"list"},{"slug":"gpt-reserve","visibility":"hide"}]}"#;

/// A plan Responses mock. `answer` gets the request's index and returns a
/// status and body; a strict check answers first. Records each request's
/// `Authorization` header and body.
pub(super) async fn serve_plan(answer: fn(usize) -> (u16, &'static str)) -> (String, PlanRequests) {
    let requests = PlanRequests::default();
    let record = requests.clone();
    let app = axum::Router::new()
        .route("/v1/models", axum::routing::get(|| async { PLAN_MODELS }))
        .route(
            "/v1/responses",
            axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
                let body: serde_json::Value = serde_json::from_str(&body).unwrap();
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let index = {
                    let mut record = record.lock().unwrap();
                    record.push((auth, body.clone()));
                    record.len() - 1
                };
                async move {
                    let (status, reply) = match plan_refusal(&body) {
                        Some(refusal) => (400, refusal),
                        None => {
                            let (status, reply) = answer(index);
                            (status, reply.to_string())
                        }
                    };
                    let kind = if status == 200 {
                        "text/event-stream"
                    } else {
                        "application/json"
                    };
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        [("content-type", kind), ("x-request-id", "req_mock")],
                        reply,
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/v1"), requests)
}

pub(super) fn plan_provider(base: &str, tokens: std::sync::Arc<FakeTokens>) -> OpenAiProvider {
    let access = crate::llm::chatgpt_plan::ChatGptPlanAccess {
        tokens,
        pause: std::sync::Arc::new(crate::llm::chatgpt_plan::LimitPause::unannounced()),
    };
    OpenAiProvider::new_chatgpt_plan(access, SOL.into(), base).unwrap()
}

/// Serve the plan model listing and fail if availability spends the plan.
pub async fn plan_availability_fixture() -> (OpenAiProvider, PlanRequests) {
    let (base, requests) = serve_plan(|_| panic!("availability must not call a model")).await;
    (plan_provider(&base, Default::default()), requests)
}
