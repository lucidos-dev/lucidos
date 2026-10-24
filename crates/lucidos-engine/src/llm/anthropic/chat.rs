//! Anthropic `/v1/messages` dispatch: endpoint, auth headers, retry loop.
//! The request body + SSE parse are shared with Vertex via `anthropic_wire`.

use super::{AnthropicAuth, AnthropicProvider, Endpoint};
use crate::llm::anthropic_wire::{build_claude_request, parse_claude_stream, WireTarget};
use crate::llm::claude_prefill;
use crate::llm::prefill::{PrefillBody, TurnRoute};
use crate::llm::provider::{LlmResponse, Message, TokenCallback, ToolDefinition};
use crate::llm::ModelSelection;

/// Body API version sent as an HTTP header on the direct API (Vertex sends its
/// own value in the body instead). Shared with the `web_search` backend in
/// `llm::web_search::anthropic`, which hits the same `/v1/messages` endpoint.
pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The (header name, header value) pair that authenticates the request.
/// API keys go on `x-api-key`; OAuth subscription tokens go on
/// `Authorization: Bearer`.
///
/// `pub(crate)` so the `web_search` backend authenticates identically — two
/// copies of this mapping would silently diverge the moment a third auth kind
/// is added.
pub(crate) fn auth_header(auth: &AnthropicAuth) -> (&'static str, String) {
    match auth {
        AnthropicAuth::ApiKey(key) => ("x-api-key", key.clone()),
        AnthropicAuth::OAuthBearer(token) => ("Authorization", format!("Bearer {}", token)),
    }
}

/// Assemble the `anthropic-beta` header value: the OAuth flag (OAuth auth only)
/// plus the betas the request body needs (`build_claude_request`). Returns
/// `None` when neither applies, so the header is omitted entirely.
fn anthropic_beta_header(auth: &AnthropicAuth, request_betas: &[&str]) -> Option<String> {
    let mut betas: Vec<&str> = Vec::new();
    if matches!(auth, AnthropicAuth::OAuthBearer(_)) {
        betas.push(super::ANTHROPIC_OAUTH_BETA);
    }
    betas.extend_from_slice(request_betas);
    if betas.is_empty() {
        None
    } else {
        Some(betas.join(","))
    }
}

/// The headers that authenticate one request on a Claude gateway: its bearer,
/// the request's own betas, and a `User-Agent` naming Lucidos. Never the
/// subscription OAuth beta, which belongs to a Claude subscription token.
fn gateway_headers(bearer: &str, request_betas: &[&str]) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("Authorization", format!("Bearer {bearer}")),
        ("User-Agent", crate::llm::lucidos_user_agent()),
    ];
    if !request_betas.is_empty() {
        headers.push(("anthropic-beta", request_betas.join(",")));
    }
    headers
}

/// The headers that authenticate one request on the direct API.
fn direct_headers(auth: &AnthropicAuth, request_betas: &[&str]) -> Vec<(&'static str, String)> {
    let mut headers = vec![auth_header(auth)];
    if let Some(beta) = anthropic_beta_header(auth, request_betas) {
        headers.push(("anthropic-beta", beta));
    }
    headers
}

/// What one call presents: the fixed direct credential, or a gateway bearer
/// fetched for this call.
enum CallAuth<'a> {
    Direct(&'a AnthropicAuth),
    Gateway(String),
}

impl CallAuth<'_> {
    fn headers(&self, request_betas: &[&str]) -> Vec<(&'static str, String)> {
        match self {
            Self::Direct(auth) => direct_headers(auth, request_betas),
            Self::Gateway(bearer) => gateway_headers(bearer, request_betas),
        }
    }
}

impl Endpoint {
    fn messages_url(&self) -> String {
        match self {
            Self::Direct(_) => format!("{}/messages", crate::llm::ANTHROPIC_API_BASE_URL),
            Self::ClaudeGateway { origin, .. } => format!("{}/messages", origin.api_base_url()),
        }
    }

    /// The provider name in logs and errors.
    fn label(&self) -> &'static str {
        match self {
            Self::Direct(_) => "Anthropic",
            Self::ClaudeGateway { .. } => "Claude gateway",
        }
    }

    /// The auth one call presents, and the model id it sends. A gateway asks
    /// its gateway access here, so each call presents a fresh bearer. It then
    /// refuses a model the gateway does not serve, before anything is sent.
    async fn begin_call(
        &self,
        requested: &str,
    ) -> Result<(CallAuth<'_>, String), Box<dyn std::error::Error + Send + Sync>> {
        match self {
            Self::Direct(auth) => Ok((CallAuth::Direct(auth), requested.to_string())),
            Self::ClaudeGateway { access, served, .. } => {
                let policy = access.managed_settings().await?;
                let (model, bearer) = served
                    .wire_id(requested, access.as_ref(), policy.as_ref())
                    .await?;
                Ok((CallAuth::Gateway(bearer), model))
            }
        }
    }

    /// A renewed bearer after a `401`, for a gateway call. `None` for a direct
    /// credential, which has no renewal.
    async fn renew_rejected(
        &self,
        auth: &CallAuth<'_>,
    ) -> Option<Result<String, Box<dyn std::error::Error + Send + Sync>>> {
        match (self, auth) {
            (Self::ClaudeGateway { access, .. }, CallAuth::Gateway(rejected)) => {
                Some(access.renew_rejected(rejected).await)
            }
            _ => None,
        }
    }

    /// The error for a `401`, which names the way to recover on this host.
    fn unauthorized(&self, error_body: &str) -> String {
        match self {
            // There is no refresh path for a direct credential: surface a
            // clear, actionable message rather than a generic auth error.
            Self::Direct(_) => format!(
                "Anthropic API 401 — check the Anthropic credential in Settings → Models → Providers \
                 (OAuth subscription tokens are short-lived and may need re-pasting): {}",
                error_body
            ),
            Self::ClaudeGateway { .. } => crate::llm::claude_gateway::unauthorized(error_body),
        }
    }
}

impl AnthropicProvider {
    pub(super) async fn chat_anthropic(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let requested = selection.model.unwrap_or(&self.model);
        let (mut auth, model) = self.endpoint.begin_call(requested).await?;
        let model = model.as_str();
        let reasoning_effort = selection.reasoning_effort;
        let messages_url = self.endpoint.messages_url();
        let label = self.endpoint.label();

        let (mut request, request_betas) = build_claude_request(
            messages,
            tools,
            model,
            system_prompt,
            reasoning_effort,
            WireTarget::Direct { url: &messages_url },
            label,
        );
        if let Some(tool) = selection.forced_tool {
            request.force_tool(tool, model)?;
        }
        let stable_prefix_fingerprint = crate::llm::stable_prefix::fingerprint(&request, model);
        let prefill = claude_prefill::pair(
            &request,
            TurnRoute {
                wire_model: requested,
                reasoning_effort,
                betas: &request_betas,
            },
        );

        let mut headers = auth.headers(&request_betas);
        let mut renewed = false;
        let display = request.thinking_display();

        // Retry loop for connection errors, retryable HTTP status codes, and
        // mid-stream overload errors. Content is accumulated internally by
        // parse_claude_stream, so retrying the full request is safe.
        let mut attempt = 0u32;
        loop {
            attempt += 1;

            let mut builder = self
                .streaming_client
                .post(&messages_url)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("Content-Type", "application/json");
            for (name, value) in &headers {
                builder = builder.header(*name, value);
            }
            let builder = builder.json(&request);

            let resp = match crate::llm::send_streaming_request(
                builder,
                model,
                attempt,
                selection.attempt_timeout,
            )
            .await
            {
                crate::llm::StreamSend::Got(r) => r,
                crate::llm::StreamSend::Retry => continue,
                crate::llm::StreamSend::Failed(e) => return Err(e),
            };

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();

                // 401 means a bad or expired credential for this host. A
                // gateway bearer renews once before the turn fails.
                if status.as_u16() == 401 {
                    if !renewed {
                        if let Some(renewal) = self.endpoint.renew_rejected(&auth).await {
                            auth = CallAuth::Gateway(renewal?);
                            headers = auth.headers(&request_betas);
                            renewed = true;
                            continue;
                        }
                    }
                    return Err(self.endpoint.unauthorized(&error_body).into());
                }

                if crate::llm::should_retry_http(status.as_u16(), &error_body, attempt) {
                    let delay = crate::llm::retry_delay(attempt, 1);
                    crate::llm::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                    tokio::time::sleep(delay).await;
                    continue;
                }

                return Err(crate::llm::not_served::failure(
                    status.as_u16(),
                    &error_body,
                    crate::llm::with_retry_context(
                        format!("{label} API error ({}): {}", status, error_body),
                        attempt,
                    ),
                ));
            }

            match parse_claude_stream(resp, &on_token, display, label).await {
                Ok(response) => {
                    return Ok(LlmResponse {
                        stable_prefix_fingerprint,
                        prefill,
                        ..response
                    })
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if crate::llm::retry_after_stream_error(model, &err_str, attempt).await {
                        continue;
                    }
                    return Err(crate::llm::with_retry_context(e, attempt).into());
                }
            }
        }
    }

    /// Send one prefill beat, with a fresh credential for a gateway.
    pub(super) async fn prefill_anthropic(
        &self,
        body: &PrefillBody,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let (auth, _) = self.endpoint.begin_call(&body.wire_model).await?;
        let mut builder = self
            .streaming_client
            .post(self.endpoint.messages_url())
            .header("anthropic-version", ANTHROPIC_VERSION);
        for (name, value) in auth.headers(&body.betas) {
            builder = builder.header(name, value);
        }
        crate::llm::prefill::send(builder, body, self.endpoint.label()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::anthropic_wire::ANTHROPIC_BETA_1M_CONTEXT;

    #[test]
    fn api_key_uses_x_api_key_header() {
        let (name, value) = auth_header(&AnthropicAuth::ApiKey("sk-ant-123".into()));
        assert_eq!(name, "x-api-key");
        assert_eq!(value, "sk-ant-123");
    }

    #[test]
    fn oauth_uses_authorization_bearer_header() {
        let (name, value) = auth_header(&AnthropicAuth::OAuthBearer("oat-token".into()));
        assert_eq!(name, "Authorization");
        assert_eq!(value, "Bearer oat-token");
    }

    #[test]
    fn api_key_no_1m_omits_beta_header() {
        assert_eq!(
            anthropic_beta_header(&AnthropicAuth::ApiKey("k".into()), &[]),
            None
        );
    }

    #[test]
    fn api_key_with_1m_sends_only_context_beta() {
        assert_eq!(
            anthropic_beta_header(
                &AnthropicAuth::ApiKey("k".into()),
                &[ANTHROPIC_BETA_1M_CONTEXT]
            ),
            Some(ANTHROPIC_BETA_1M_CONTEXT.to_string())
        );
    }

    #[test]
    fn oauth_no_1m_sends_only_oauth_beta() {
        assert_eq!(
            anthropic_beta_header(&AnthropicAuth::OAuthBearer("t".into()), &[]),
            Some(crate::llm::anthropic::ANTHROPIC_OAUTH_BETA.to_string())
        );
    }

    #[test]
    fn oauth_with_1m_sends_both_betas_comma_joined() {
        assert_eq!(
            anthropic_beta_header(
                &AnthropicAuth::OAuthBearer("t".into()),
                &[ANTHROPIC_BETA_1M_CONTEXT]
            ),
            Some(format!(
                "{},{}",
                crate::llm::anthropic::ANTHROPIC_OAUTH_BETA,
                ANTHROPIC_BETA_1M_CONTEXT
            ))
        );
    }

    /// A gateway request carries its bearer and a `User-Agent` naming Lucidos,
    /// and never the subscription beta or an API key.
    #[test]
    fn gateway_headers_send_bearer_and_user_agent_never_the_oauth_beta() {
        let headers = gateway_headers("gw-token", &[]);
        let get = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("Authorization"), Some("Bearer gw-token"));
        assert!(get("User-Agent").is_some_and(|ua| ua.starts_with("Lucidos/")));
        assert_eq!(get("anthropic-beta"), None);
        assert_eq!(get("x-api-key"), None);
    }

    #[test]
    fn gateway_headers_keep_the_request_betas() {
        let headers = gateway_headers("t", &[ANTHROPIC_BETA_1M_CONTEXT]);
        assert!(headers
            .iter()
            .any(|(n, v)| *n == "anthropic-beta" && v == ANTHROPIC_BETA_1M_CONTEXT));
        assert!(!headers
            .iter()
            .any(|(_, v)| v.contains(crate::llm::anthropic::ANTHROPIC_OAUTH_BETA)));
    }

    mod on_a_mock_gateway {
        use super::super::super::AnthropicProvider;
        use crate::llm::claude_gateway::gateway_managed_settings::GatewayManagedSettings;
        use crate::llm::claude_gateway::{GatewayAccess, GatewayOrigin};
        use crate::llm::{Message, MessageContent, ModelSelection};
        use axum::http::{HeaderMap, StatusCode, Uri};
        use std::sync::{Arc, Mutex};

        const COMPLETED_TURN: &str = concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
            "\n\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"#,
            r#""usage":{"output_tokens":3}}"#,
            "\n\n",
            r#"data: {"type":"message_stop"}"#,
            "\n\n"
        );

        /// A turn that streams one text delta.
        const TEXT_TURN: &str = concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":0,"#,
            r#""content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"#,
            r#""delta":{"type":"text_delta","text":"hello"}}"#,
            "\n\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"#,
            r#""usage":{"output_tokens":3}}"#,
            "\n\n",
            r#"data: {"type":"message_stop"}"#,
            "\n\n"
        );

        /// The `request-id` header every mock response carries.
        const MOCK_REQUEST_ID: &str = "req_mock";

        /// A bearer, and what renewing it after a `401` gives.
        struct FixedTokens(Result<String, String>, Result<String, String>);

        impl FixedTokens {
            /// A source whose renewal hands back the same bearer.
            fn fixed(bearer: Result<String, String>) -> Self {
                Self(bearer.clone(), bearer)
            }
        }

        #[async_trait::async_trait]
        impl GatewayAccess for FixedTokens {
            async fn bearer(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
                self.0.clone().map_err(Into::into)
            }

            async fn renew_rejected(
                &self,
                _rejected: &str,
            ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
                self.1.clone().map_err(Into::into)
            }

            async fn managed_settings(
                &self,
            ) -> Result<Option<GatewayManagedSettings>, Box<dyn std::error::Error + Send + Sync>>
            {
                Ok(None)
            }
        }

        /// [`FixedTokens`] under a gateway policy.
        struct UnderPolicy(FixedTokens, serde_json::Value);

        #[async_trait::async_trait]
        impl GatewayAccess for UnderPolicy {
            async fn bearer(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
                self.0.bearer().await
            }

            async fn renew_rejected(
                &self,
                rejected: &str,
            ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
                self.0.renew_rejected(rejected).await
            }

            async fn managed_settings(
                &self,
            ) -> Result<Option<GatewayManagedSettings>, Box<dyn std::error::Error + Send + Sync>>
            {
                Ok(Some(GatewayManagedSettings::from_document(&self.1)))
            }
        }

        /// What the mock gateway saw: each request's path and headers.
        type Seen = Arc<Mutex<Vec<(String, HeaderMap)>>>;

        /// The one model the mock gateway lists.
        const LISTED: &str = "claude-test";

        /// A gateway listing [`LISTED`] that answers every Messages request
        /// with `status` and `body`.
        async fn serve(status: StatusCode, body: &'static str) -> (GatewayOrigin, Seen) {
            serve_with(move |_| (status, body)).await
        }

        /// A gateway listing [`LISTED`] that answers each Messages request
        /// with what `answer` gives for its headers.
        async fn serve_with(
            answer: impl Fn(&HeaderMap) -> (StatusCode, &'static str) + Clone + Send + Sync + 'static,
        ) -> (GatewayOrigin, Seen) {
            let seen: Seen = Arc::default();
            let record = seen.clone();
            let listing = seen.clone();
            let app = axum::Router::new()
                .route(
                    "/v1/models",
                    axum::routing::get(move |headers: HeaderMap| {
                        listing
                            .lock()
                            .unwrap()
                            .push(("/v1/models".to_string(), headers));
                        async move {
                            axum::Json(serde_json::json!({
                                "data": [{"type": "model", "id": LISTED}],
                                "has_more": false,
                            }))
                        }
                    }),
                )
                .fallback(move |uri: Uri, headers: HeaderMap| {
                    let (status, body) = answer(&headers);
                    record
                        .lock()
                        .unwrap()
                        .push((uri.path().to_string(), headers));
                    async move {
                        (
                            status,
                            [
                                ("content-type", "text/event-stream"),
                                ("request-id", MOCK_REQUEST_ID),
                            ],
                            body,
                        )
                    }
                });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let origin = GatewayOrigin::parse(&format!("http://{addr}")).unwrap();
            (origin, seen)
        }

        /// The Messages requests the mock saw, with their headers.
        fn messages(seen: &Seen) -> Vec<HeaderMap> {
            seen.lock()
                .unwrap()
                .iter()
                .filter(|(path, _)| path == "/v1/messages")
                .map(|(_, headers)| headers.clone())
                .collect()
        }

        async fn turn_on(
            origin: GatewayOrigin,
            tokens: impl GatewayAccess + 'static,
            model: &str,
        ) -> Result<crate::llm::provider::LlmResponse, Box<dyn std::error::Error + Send + Sync>>
        {
            let provider =
                AnthropicProvider::claude_gateway(origin, Arc::new(tokens), "m".to_string())
                    .unwrap();
            provider
                .chat_anthropic(
                    vec![Message {
                        role: "user".to_string(),
                        content: MessageContent::Text("hi".to_string()),
                    }],
                    vec![],
                    ModelSelection::model(model),
                    None,
                    None,
                )
                .await
        }

        async fn one_turn(
            origin: GatewayOrigin,
            tokens: FixedTokens,
        ) -> Result<crate::llm::provider::LlmResponse, String> {
            turn_on(origin, tokens, LISTED)
                .await
                .map_err(|e| e.to_string())
        }

        /// The gateway provider sends to its own origin's Messages path, with
        /// the gateway access bearer and no direct-API credential.
        #[tokio::test]
        async fn sends_its_bearer_to_its_own_origin() {
            let (origin, seen) = serve(StatusCode::OK, COMPLETED_TURN).await;
            let response = one_turn(origin, FixedTokens::fixed(Ok("gw-token".into())))
                .await
                .expect("a completed stream is a completed turn");
            assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));

            let sent = messages(&seen);
            let [headers] = sent.as_slice() else {
                panic!("expected one Messages request, saw {}", sent.len());
            };
            assert_eq!(headers["authorization"], "Bearer gw-token");
            assert!(headers["user-agent"]
                .to_str()
                .unwrap()
                .starts_with("Lucidos/"));
            assert!(headers.get("x-api-key").is_none());
            assert!(headers.get("anthropic-beta").is_none());
        }

        /// A model the gateway did not list is refused as not served, and the
        /// Messages API never sees it.
        #[tokio::test]
        async fn an_unlisted_model_is_refused_before_any_request() {
            let (origin, seen) = serve(StatusCode::OK, COMPLETED_TURN).await;
            let err = turn_on(
                origin,
                FixedTokens::fixed(Ok("gw-token".into())),
                "claude-unlisted",
            )
            .await
            .expect_err("the gateway did not list it");
            assert!(err.is::<crate::llm::ModelNotServed>(), "{err}");
            assert!(messages(&seen).is_empty());
        }

        /// The policy's `availableModels` decides what the gateway serves, in
        /// place of its listing. A model outside it is refused before any
        /// request, and one inside it is sent as the entry names it.
        #[tokio::test]
        async fn the_policy_allowlist_decides_what_is_served() {
            let (origin, seen) = serve(StatusCode::OK, COMPLETED_TURN).await;
            let policy = serde_json::json!({"availableModels": ["claude-allowed[1m]"]});
            let err = turn_on(
                origin.clone(),
                UnderPolicy(FixedTokens::fixed(Ok("gw-token".into())), policy.clone()),
                LISTED,
            )
            .await
            .expect_err("the policy does not allow it");
            assert!(err.is::<crate::llm::ModelNotServed>(), "{err}");
            assert!(err.to_string().contains("claude-allowed"), "{err}");
            assert!(seen.lock().unwrap().is_empty(), "nothing was sent");

            turn_on(
                origin,
                UnderPolicy(FixedTokens::fixed(Ok("gw-token".into())), policy),
                "claude-allowed",
            )
            .await
            .expect("the policy allows it");
            let paths: Vec<String> = seen
                .lock()
                .unwrap()
                .iter()
                .map(|(p, _)| p.clone())
                .collect();
            assert_eq!(paths, vec!["/v1/messages"], "the listing is not read");
        }

        /// A gateway refuses a model outside the account's access group with a
        /// `400`. Its body reaches the user as the gateway wrote it.
        #[tokio::test]
        async fn a_400_reaches_the_user_verbatim() {
            const REFUSAL: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"model not allowed for your group"}}"#;
            let (origin, _) = serve(StatusCode::BAD_REQUEST, REFUSAL).await;
            let err = one_turn(origin, FixedTokens::fixed(Ok("gw-token".into())))
                .await
                .expect_err("a 400 fails the turn");
            assert!(err.contains(REFUSAL), "{err}");
        }

        /// A `401` that renewing does not cure tells the user to sign in to
        /// the gateway again, after one renewed attempt.
        #[tokio::test]
        async fn a_401_says_sign_in_again() {
            let (origin, seen) = serve(StatusCode::UNAUTHORIZED, "{}").await;
            let err = one_turn(origin, FixedTokens::fixed(Ok("stale".into())))
                .await
                .expect_err("a 401 fails the turn");
            assert!(err.contains("sign in to the Claude gateway again"), "{err}");
            assert_eq!(messages(&seen).len(), 2);
        }

        /// A gateway that ends a token early answers `401`. The renewed
        /// bearer then completes the turn.
        #[tokio::test]
        async fn a_401_renews_the_bearer_once() {
            let (origin, seen) = serve_with(|headers| match headers["authorization"].to_str() {
                Ok("Bearer renewed") => (StatusCode::OK, COMPLETED_TURN),
                _ => (StatusCode::UNAUTHORIZED, "{}"),
            })
            .await;
            let tokens = FixedTokens(Ok("ended".into()), Ok("renewed".into()));
            one_turn(origin, tokens)
                .await
                .expect("the renewed bearer completes the turn");
            let sent: Vec<_> = messages(&seen)
                .iter()
                .map(|h| h["authorization"].to_str().unwrap().to_string())
                .collect();
            assert_eq!(sent, ["Bearer ended", "Bearer renewed"]);
        }

        /// A renewal that fails fails the turn with its own message.
        #[tokio::test]
        async fn a_failed_renewal_fails_the_turn_with_its_message() {
            let (origin, seen) = serve(StatusCode::UNAUTHORIZED, "{}").await;
            let tokens = FixedTokens(Ok("ended".into()), Err("sign in again".into()));
            let err = one_turn(origin, tokens)
                .await
                .expect_err("no renewed bearer, no turn");
            assert_eq!(err, "sign in again");
            assert_eq!(messages(&seen).len(), 1);
        }

        /// A gateway access that cannot produce a bearer fails the call before
        /// anything is sent, with its own message.
        #[tokio::test]
        async fn a_token_failure_sends_nothing() {
            let (origin, seen) = serve(StatusCode::OK, COMPLETED_TURN).await;
            let err = one_turn(origin, FixedTokens::fixed(Err("sign in again".into())))
                .await
                .expect_err("no bearer, no turn");
            assert_eq!(err, "sign in again");
            assert!(seen.lock().unwrap().is_empty());
        }

        /// The call trace names the answering response's `request-id` and the
        /// stream's first delta, and no delta where the stream carried none.
        #[tokio::test]
        async fn a_turn_traces_its_request_id_and_first_delta() {
            let (origin, _) = serve(StatusCode::OK, TEXT_TURN).await;
            let (outcome, trace) = crate::llm::call_trace::scoped(turn_on(
                origin,
                FixedTokens::fixed(Ok("gw-token".into())),
                LISTED,
            ))
            .await;
            outcome.expect("a completed stream is a completed turn");
            assert_eq!(trace.attempts, 1);
            assert_eq!(trace.status, Some(200));
            assert_eq!(trace.request_id.as_deref(), Some(MOCK_REQUEST_ID));
            assert!(trace.first_delta.is_some());

            let (origin, _) = serve(StatusCode::OK, COMPLETED_TURN).await;
            let (_, silent) = crate::llm::call_trace::scoped(turn_on(
                origin,
                FixedTokens::fixed(Ok("gw-token".into())),
                LISTED,
            ))
            .await;
            assert!(silent.first_delta.is_none(), "no delta, no first delta");
        }

        /// A retry's trace describes the attempt that answered: the overloaded
        /// first attempt counts, and its status does not survive.
        #[tokio::test]
        async fn a_retried_turn_traces_the_answering_attempt() {
            let calls = Arc::new(Mutex::new(0u32));
            let (origin, _) = serve_with(move |_| {
                let mut calls = calls.lock().unwrap();
                *calls += 1;
                if *calls == 1 {
                    (StatusCode::from_u16(529).unwrap(), "{}")
                } else {
                    (StatusCode::OK, TEXT_TURN)
                }
            })
            .await;
            let (outcome, trace) = crate::llm::call_trace::scoped(turn_on(
                origin,
                FixedTokens::fixed(Ok("gw-token".into())),
                LISTED,
            ))
            .await;
            outcome.expect("the retry answers");
            assert_eq!(trace.attempts, 2);
            assert_eq!(trace.status, Some(200));
            assert!(trace.first_delta.is_some());
        }

        /// A failed call's trace names the last status, which classifies it.
        #[tokio::test]
        async fn a_refused_turn_traces_its_http_status() {
            let (origin, _) = serve(StatusCode::BAD_REQUEST, "{}").await;
            let (outcome, trace) = crate::llm::call_trace::scoped(turn_on(
                origin,
                FixedTokens::fixed(Ok("gw-token".into())),
                LISTED,
            ))
            .await;
            outcome.expect_err("a 400 fails the turn");
            assert_eq!(trace.status, Some(400));
            assert_eq!(
                trace.failure(false),
                crate::llm::call_trace::CallFailure::Http
            );
        }

        /// A gateway that streams a turn and a beat alike, and keeps each
        /// Messages body it was sent. [`COMPLETED_TURN`] stands for the
        /// stream a beat hangs up on.
        async fn serve_recording_bodies() -> (GatewayOrigin, Arc<Mutex<Vec<serde_json::Value>>>) {
            let bodies: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
            let record = bodies.clone();
            let app = axum::Router::new()
                .route(
                    "/v1/models",
                    axum::routing::get(|| async {
                        axum::Json(serde_json::json!({
                            "data": [{"type": "model", "id": LISTED}],
                            "has_more": false,
                        }))
                    }),
                )
                .route(
                    "/v1/messages",
                    axum::routing::post(move |body: axum::body::Bytes| {
                        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                        record.lock().unwrap().push(body);
                        async move { ([("content-type", "text/event-stream")], COMPLETED_TURN) }
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            (
                GatewayOrigin::parse(&format!("http://{addr}")).unwrap(),
                bodies,
            )
        }

        /// A turn hands back its prefill pair. Its whole body goes out as the
        /// turn's bytes and parameters plus the beat prompt, and the usage of
        /// the stream's `message_start` comes back.
        #[tokio::test]
        async fn a_turn_returns_a_pair_whose_beat_reads_the_turn() {
            let (origin, bodies) = serve_recording_bodies().await;
            let provider = AnthropicProvider::claude_gateway(
                origin,
                Arc::new(FixedTokens::fixed(Ok("gw-token".into()))),
                "m".to_string(),
            )
            .unwrap();
            let turn = provider
                .chat_anthropic(
                    vec![Message {
                        role: "user".to_string(),
                        content: MessageContent::Text("hi".to_string()),
                    }],
                    vec![],
                    ModelSelection::model(LISTED),
                    Some("system prompt"),
                    None,
                )
                .await
                .expect("the turn completes");
            let pair = turn.prefill.expect("a gateway turn can be prefilled");
            assert_eq!(pair.whole.wire_model, LISTED);

            let beat = provider
                .prefill_anthropic(&pair.whole)
                .await
                .expect("the beat answers");
            assert_eq!(beat.input_tokens, Some(9));

            let sent = bodies.lock().unwrap().clone();
            let [turn_body, beat_body] = sent.as_slice() else {
                panic!("expected a turn and a beat, saw {}", sent.len());
            };
            let mut expected = turn_body.clone();
            expected["messages"][0]["content"]
                .as_array_mut()
                .expect("the turn's message is blocks")
                .push(serde_json::json!({
                    "type": "text",
                    "text": crate::llm::claude_prefill::BEAT_PROMPT
                }));
            assert_eq!(beat_body, &expected);
        }
    }
}
