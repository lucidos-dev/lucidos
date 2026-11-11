//! The ChatGPT plan route through the router (ADR 0636): a plan failure never
//! moves the call to the platform route, and a served call names the plan.

use super::*;
use crate::llm::chatgpt_plan::{ChatGptPlanAccess, ChatGptPlanTokens, LimitPause};
use crate::llm::model_registry::ModelRouting;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::RwLock;

const SOL: &str = "gpt-6.1-sol";
const UNLISTED: &str = "gpt-5.4";

const COMPLETED_TURN: &str = concat!(
    "event: response.output_text.delta\n",
    r#"data: {"delta":"done"}"#,
    "\n\n",
    "event: response.completed\n",
    r#"data: {"response":{"status":"completed","usage":{"input_tokens":9,"output_tokens":1}}}"#,
    "\n\n",
);

const LIMIT_BODY: &str = r#"{"error":{"message":"limit","type":"usage_limit","param":null,"code":"subscription_sharing_usage_limit_exceeded"}}"#;

struct FixedTokens;

#[async_trait]
impl ChatGptPlanTokens for FixedTokens {
    async fn access_token(&self) -> Result<String, BoxError> {
        Ok("plan-token".into())
    }

    async fn after_rejection(&self, _rejected: &str) -> Result<String, BoxError> {
        Ok("plan-token".into())
    }
}

/// A plan backend listing only `SOL`, answering every Responses request with
/// `status` and `body`.
async fn plan_backend(status: u16, body: &'static str) -> OpenAiProvider {
    let app = axum::Router::new()
        .route(
            "/v1/models",
            axum::routing::get(|| async {
                r#"{"models":[{"slug":"gpt-6.1-sol","visibility":"list"}]}"#
            }),
        )
        .route(
            "/v1/responses",
            axum::routing::post(move || async move {
                let kind = match status {
                    200 => "text/event-stream",
                    _ => "application/json",
                };
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    [("content-type", kind)],
                    body,
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let access = ChatGptPlanAccess {
        tokens: Arc::new(FixedTokens),
        pause: Arc::new(LimitPause::unannounced()),
    };
    OpenAiProvider::new_chatgpt_plan(access, SOL.into(), &format!("http://{addr}/v1")).unwrap()
}

/// A platform backend that counts every request and answers none usefully.
async fn counting_openai() -> (OpenAiProvider, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let app = axum::Router::new().fallback(move || {
        seen.fetch_add(1, Ordering::SeqCst);
        async { (axum::http::StatusCode::BAD_REQUEST, "{}") }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let provider = OpenAiProvider::new_with_base_url(
        "sk-platform".into(),
        SOL.into(),
        &format!("http://{addr}/v1"),
        Vec::new(),
        false,
    )
    .unwrap();
    (provider, hits)
}

/// Every model routed plan first, then the platform key.
fn plan_then_openai(router_backends: RoutingBackends) -> RoutingProvider {
    let row = || ModelRouting {
        routes: vec![
            RouteEntry::new(ProviderKind::ChatGptPlan, "unused"),
            RouteEntry::new(ProviderKind::OpenAi, "unused"),
        ],
        ..ModelRouting::single(ProviderKind::ChatGptPlan, "unused")
    };
    let mut rows = HashMap::new();
    for model in [SOL, UNLISTED] {
        let mut routing = row();
        for route in &mut routing.routes {
            route.wire_id = model.to_string();
        }
        rows.insert(model.to_string(), routing);
    }
    RoutingProvider::new(router_backends, Arc::new(RwLock::new(rows)), SOL.into())
}

async fn ask(router: &RoutingProvider, model: &str) -> Result<LlmResponse, BoxError> {
    router
        .chat(
            vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            vec![],
            ModelSelection::model(model),
            None,
            None,
            crate::llm::metered::CallToken::for_test(),
        )
        .await
}

/// A plan limit, the pause after it, and a model the plan does not list all
/// fail on the plan route. None of them sends a request on the API key.
#[tokio::test]
async fn a_plan_failure_never_moves_the_call_to_the_openai_route() {
    let (openai, hits) = counting_openai().await;
    let router = plan_then_openai(RoutingBackends {
        chatgpt_plan: Some(plan_backend(429, LIMIT_BODY).await),
        openai: Some(openai),
        ..RoutingBackends::default()
    });
    for model in [SOL, SOL, UNLISTED, UNLISTED] {
        assert!(ask(&router, model).await.is_err(), "{model}");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

/// A served plan call names its route, which the cost capture records as
/// the provider `chatgpt-plan`.
#[tokio::test]
async fn a_served_plan_call_names_the_plan_route() {
    let router = plan_then_openai(RoutingBackends {
        chatgpt_plan: Some(plan_backend(200, COMPLETED_TURN).await),
        ..RoutingBackends::default()
    });
    let response = ask(&router, SOL).await.expect("the plan answers");
    assert_eq!(response.provider, Some(ProviderKind::ChatGptPlan));
    assert_eq!(ProviderKind::ChatGptPlan.as_str(), "chatgpt-plan");
    assert!(response.prefill.is_none(), "a plan turn offers no beat");
}

/// Apps never reach the plan: its route has no proxy.
#[test]
fn the_plan_route_has_no_app_proxy() {
    assert!(crate::api::proxy_builtin::model_proxy_info("chatgpt-plan", SOL).is_none());
}
