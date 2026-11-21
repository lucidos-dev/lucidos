use super::*;

#[tokio::test]
async fn discovery_requires_a_matching_issuer_without_minting_tokens() {
    use crate::test_support::{
        seed_claude_gateway_sign_in, seed_preference, setup_test_db, teardown_test_db,
    };
    let (pool, db) = setup_test_db().await;
    let factory: crate::llm::claude_gateway::GatewayAccessFactory = Arc::new(|_| {
        Arc::new(Access {
            renewals: AtomicUsize::new(0),
        })
    });
    assert_eq!(
        resolve_for(&pool, &factory).await.err().unwrap().0,
        StatusCode::NOT_FOUND
    );
    seed_preference(
        &pool,
        prefs::CLAUDE_GATEWAY_URL.key(),
        "https://gateway.example",
    )
    .await
    .unwrap();
    assert_eq!(
        resolve_for(&pool, &factory).await.err().unwrap().0,
        StatusCode::NOT_FOUND
    );
    seed_claude_gateway_sign_in(&pool, "https://other.example").await;
    assert_eq!(
        resolve_for(&pool, &factory).await.err().unwrap().0,
        StatusCode::NOT_FOUND
    );
    seed_preference(
        &pool,
        prefs::CLAUDE_GATEWAY_URL.key(),
        "https://other.example",
    )
    .await
    .unwrap();
    let (base, layers) = resolve_for(&pool, &factory).await.unwrap();
    assert_eq!(base, "https://other.example/v1");
    assert_eq!(
        layers[0].scope_bindings(),
        vec![pinned("the Claude gateway sign-in", &base)]
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn policy_inspects_large_messages_too() {
    let layers: Vec<Arc<dyn AuthLayer>> = vec![Arc::new(GatewayLayer::new(
        GatewayOrigin::parse("https://gateway.example").unwrap(),
        Arc::new(Access {
            renewals: AtomicUsize::new(0),
        }),
    ))];
    let body: bytes::Bytes = serde_json::to_vec(&serde_json::json!({
        "model": "claude-haiku-5-5", "messages": [{"role":"user", "content":"x".repeat(1024 * 1024)}]
    })).unwrap().into();
    let result = run_pipeline(
        &layers,
        &Method::POST,
        "https://gateway.example/v1/messages",
        &[],
        &body,
    )
    .await
    .unwrap();
    assert!(result.replace_body.unwrap().len() > 1024 * 1024);
}
use crate::api::proxy_pipeline::{
    pipeline_invalidate_for_retry, pipeline_should_retry, run_pipeline,
};
use crate::llm::anthropic_wire::ANTHROPIC_BETA_1M_CONTEXT;
use crate::llm::claude_gateway::gateway_managed_settings::GatewayManagedSettings;
use axum::http::Method;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn dispatch_retries_401_once_and_preserves_quota_refusals() {
    use crate::api::proxy::{dispatch_scoped, ScopeContext, ScopedPipeline};
    use axum::{body::Bytes, http::HeaderMap, response::IntoResponse, routing::post, Router};
    for quota in [false, true] {
        let seen = Arc::new(AtomicUsize::new(0));
        let calls = seen.clone();
        let router = Router::new().route(
            "/v1/messages",
            post(move |headers: HeaderMap, body: Bytes| {
                let calls = calls.clone();
                async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    assert!(headers["user-agent"]
                        .to_str()
                        .unwrap()
                        .starts_with("Lucidos/"));
                    assert_eq!(
                        headers["authorization"],
                        if n == 0 {
                            "Bearer first-token"
                        } else {
                            "Bearer renewed-token"
                        }
                    );
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(body["model"], "claude-haiku-5-5-20261001");
                    if quota {
                        (
                            StatusCode::TOO_MANY_REQUESTS,
                            [("retry-after", "60")],
                            "spend limit",
                        )
                            .into_response()
                    } else if n == 0 {
                        StatusCode::UNAUTHORIZED.into_response()
                    } else {
                        (StatusCode::OK, crate::llm::lucidos_user_agent()).into_response()
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin =
            GatewayOrigin::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let access = Arc::new(Access {
            renewals: AtomicUsize::new(0),
        });
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let bus = crate::test_support::offline_event_bus();
        let ctx = ScopeContext {
            pool: &pool,
            workspace_path: workspace.path(),
            event_bus: &bus,
        };
        let scoped = ScopedPipeline::bind(
            &ctx,
            "claude-gateway",
            origin.api_base_url(),
            vec![Arc::new(GatewayLayer::new(origin, access.clone()))],
            false,
        )
        .await
        .unwrap();
        let mut headers = HeaderMap::new();
        crate::api::proxy_cost::prepare_request(
            &crate::api::proxy_cost::Upstream::Builtin {
                name: "claude-gateway",
            },
            &mut headers,
        );
        let response = dispatch_scoped(
            "claude-gateway",
            &scoped,
            std::time::Duration::from_secs(5),
            Method::POST,
            "/messages".into(),
            None,
            headers,
            Bytes::from_static(br#"{"model":"claude-haiku-5-5","messages":[],"max_tokens":8}"#),
        )
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            if quota {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::OK
            }
        );
        assert_eq!(seen.load(Ordering::SeqCst), if quota { 1 } else { 2 });
        assert_eq!(
            access.renewals.load(Ordering::SeqCst),
            if quota { 0 } else { 1 }
        );
        if quota {
            assert_eq!(response.headers()["retry-after"], "60");
            assert_eq!(
                axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap(),
                "spend limit"
            );
        } else {
            assert_eq!(
                axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap(),
                crate::llm::lucidos_user_agent()
            );
        }
        server.abort();
    }
}

struct Access {
    renewals: AtomicUsize,
}
#[async_trait]
impl GatewayAccess for Access {
    async fn bearer(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok(if self.renewals.load(Ordering::SeqCst) == 0 {
            "first-token"
        } else {
            "renewed-token"
        }
        .into())
    }
    async fn renew_rejected(
        &self,
        rejected: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        assert_eq!(rejected, "first-token");
        self.renewals.fetch_add(1, Ordering::SeqCst);
        Ok("renewed-token".into())
    }
    async fn managed_settings(
        &self,
    ) -> Result<Option<GatewayManagedSettings>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Some(GatewayManagedSettings::from_document(
            &serde_json::json!({
                "availableModels": ["claude-haiku-5-5-20261001"]
            }),
        )))
    }
}

#[tokio::test]
async fn policy_snapshot_identity_and_rejected_token_retry() {
    let access = Arc::new(Access {
        renewals: AtomicUsize::new(0),
    });
    let origin = GatewayOrigin::parse("https://gateway.example").unwrap();
    let layers: Vec<Arc<dyn AuthLayer>> = vec![Arc::new(GatewayLayer::new(origin, access.clone()))];
    let body =
        bytes::Bytes::from_static(br#"{"model":"claude-haiku-5-5","max_tokens":8,"messages":[]}"#);
    let url = "https://gateway.example/v1/messages";
    let result = run_pipeline(&layers, &Method::POST, url, &[], &body)
        .await
        .unwrap();
    let replaced: serde_json::Value =
        serde_json::from_slice(result.replace_body.as_ref().unwrap()).unwrap();
    assert_eq!(replaced["model"], "claude-haiku-5-5-20261001");
    assert!(pipeline_should_retry(
        &layers,
        &result,
        StatusCode::UNAUTHORIZED
    ));
    assert!(!pipeline_should_retry(
        &layers,
        &result,
        StatusCode::TOO_MANY_REQUESTS
    ));
    pipeline_invalidate_for_retry(&layers, &result).await;
    let retried = run_pipeline(&layers, &Method::POST, url, &[], &body)
        .await
        .unwrap();
    assert!(retried
        .headers
        .iter()
        .any(|(name, value)| name == "authorization" && value == "Bearer renewed-token"));
    assert_eq!(access.renewals.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_1m_model_sends_the_listed_id_and_the_context_beta() {
    let layers: Vec<Arc<dyn AuthLayer>> = vec![Arc::new(GatewayLayer::new(
        GatewayOrigin::parse("https://gateway.example").unwrap(),
        Arc::new(Access {
            renewals: AtomicUsize::new(0),
        }),
    ))];
    let body = bytes::Bytes::from_static(br#"{"model":"claude-haiku-5-5[1m]","messages":[]}"#);
    let caller = [(
        HeaderName::from_static("anthropic-beta"),
        "caller-beta".to_string(),
    )];
    let result = run_pipeline(
        &layers,
        &Method::POST,
        "https://gateway.example/v1/messages",
        &caller,
        &body,
    )
    .await
    .unwrap();
    let replaced: serde_json::Value =
        serde_json::from_slice(result.replace_body.as_ref().unwrap()).unwrap();
    assert_eq!(replaced["model"], "claude-haiku-5-5-20261001");
    let beta = format!("caller-beta,{ANTHROPIC_BETA_1M_CONTEXT}");
    assert!(result
        .headers
        .iter()
        .any(|(name, value)| name == "anthropic-beta" && *value == beta));
}

#[tokio::test]
async fn refuses_other_origins_paths_methods_and_models() {
    let layers: Vec<Arc<dyn AuthLayer>> = vec![Arc::new(GatewayLayer::new(
        GatewayOrigin::parse("https://gateway.example").unwrap(),
        Arc::new(Access {
            renewals: AtomicUsize::new(0),
        }),
    ))];
    for (method, url, model) in [
        (
            Method::POST,
            "https://other.example/v1/messages",
            "claude-haiku-5-5",
        ),
        (
            Method::POST,
            "https://gateway.example/v1/logs",
            "claude-haiku-5-5",
        ),
        (
            Method::GET,
            "https://gateway.example/v1/messages",
            "claude-haiku-5-5",
        ),
        (
            Method::POST,
            "https://gateway.example/v1/messages",
            "claude-opus-5-5",
        ),
    ] {
        let body =
            bytes::Bytes::from(serde_json::to_vec(&serde_json::json!({"model":model})).unwrap());
        let error = run_pipeline(&layers, &method, url, &[], &body)
            .await
            .unwrap_err();
        if model == "claude-opus-5-5" {
            assert_eq!(error.0, StatusCode::FORBIDDEN);
        }
    }
}
