//! Direct Anthropic `/v1/messages` dispatch: endpoint, auth headers, retry loop.
//! The request body + SSE parse are shared with Vertex via `anthropic_wire`.

use super::{AnthropicAuth, AnthropicProvider};
use crate::llm::anthropic_wire::{
    build_claude_request, parse_claude_stream, WireTarget, ANTHROPIC_BETA_1M_CONTEXT,
};
use crate::llm::provider::{LlmResponse, Message, TokenCallback, ToolDefinition};

const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
/// Body API version sent as an HTTP header on the direct API (Vertex sends its
/// own value in the body instead).
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Beta flag that authorizes a Claude subscription OAuth bearer token to call
/// the Messages API.
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// The (header name, header value) pair that authenticates the request.
/// API keys go on `x-api-key`; OAuth subscription tokens go on
/// `Authorization: Bearer`.
fn auth_header(auth: &AnthropicAuth) -> (&'static str, String) {
    match auth {
        AnthropicAuth::ApiKey(key) => ("x-api-key", key.clone()),
        AnthropicAuth::OAuthBearer(token) => ("Authorization", format!("Bearer {}", token)),
    }
}

/// Assemble the `anthropic-beta` header value: the OAuth flag (OAuth auth only)
/// plus the 1M-context flag (when the model carries `[1m]`). Returns `None` when
/// neither applies, so the header is omitted entirely.
fn anthropic_beta_header(auth: &AnthropicAuth, is_1m: bool) -> Option<String> {
    let mut betas: Vec<&str> = Vec::new();
    if matches!(auth, AnthropicAuth::OAuthBearer(_)) {
        betas.push(OAUTH_BETA);
    }
    if is_1m {
        betas.push(ANTHROPIC_BETA_1M_CONTEXT);
    }
    if betas.is_empty() {
        None
    } else {
        Some(betas.join(","))
    }
}

impl AnthropicProvider {
    pub(super) async fn chat_anthropic(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: &str,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let (request, is_1m) = build_claude_request(
            messages,
            tools,
            model,
            system_prompt,
            reasoning_effort,
            WireTarget::Direct,
            "Anthropic",
        );

        let (auth_name, auth_value) = auth_header(&self.auth);
        let beta = anthropic_beta_header(&self.auth, is_1m);

        // Retry loop for connection errors, retryable HTTP status codes, and
        // mid-stream overload errors. Content is accumulated internally by
        // parse_claude_stream, so retrying the full request is safe.
        let mut attempt = 0u32;
        loop {
            attempt += 1;

            let mut builder = self
                .streaming_client
                .post(ANTHROPIC_MESSAGES_URL)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("Content-Type", "application/json")
                .header(auth_name, &auth_value);
            if let Some(beta) = &beta {
                builder = builder.header("anthropic-beta", beta);
            }

            let resp = match builder.json(&request).send().await {
                Ok(r) => r,
                Err(e) => {
                    if attempt <= crate::llm::MAX_RETRIES {
                        let delay = crate::llm::retry_delay(attempt, 1);
                        crate::llm::log_retry(model, &format!("Network error: {:?}", e), attempt, delay);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(crate::llm::with_retry_context(e, attempt).into());
                }
            };

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();

                // 401 on the direct API means a bad/expired key or OAuth token.
                // There is no refresh path in v1 — surface a clear, actionable
                // message rather than a generic auth error.
                if status.as_u16() == 401 {
                    return Err(format!(
                        "Anthropic API 401 — check the Anthropic credential in Settings → Providers \
                         (OAuth subscription tokens are short-lived and may need re-pasting): {}",
                        error_body
                    )
                    .into());
                }

                if crate::llm::should_retry_http(status.as_u16(), &error_body, attempt) {
                    let delay = crate::llm::retry_delay(attempt, 1);
                    crate::llm::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                    tokio::time::sleep(delay).await;
                    continue;
                }

                return Err(crate::llm::with_retry_context(
                    format!("Anthropic API error ({}): {}", status, error_body),
                    attempt,
                )
                .into());
            }

            match parse_claude_stream(resp, &on_token, "Anthropic").await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    let err_str = e.to_string();
                    if crate::llm::is_retryable_error(&err_str) && attempt <= crate::llm::MAX_RETRIES {
                        let delay = crate::llm::retry_delay(attempt, 2); // longer for stream errors
                        crate::llm::log_retry(model, &format!("Stream error: {}", err_str), attempt, delay);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(crate::llm::with_retry_context(e, attempt).into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            anthropic_beta_header(&AnthropicAuth::ApiKey("k".into()), false),
            None
        );
    }

    #[test]
    fn api_key_with_1m_sends_only_context_beta() {
        assert_eq!(
            anthropic_beta_header(&AnthropicAuth::ApiKey("k".into()), true),
            Some(ANTHROPIC_BETA_1M_CONTEXT.to_string())
        );
    }

    #[test]
    fn oauth_no_1m_sends_only_oauth_beta() {
        assert_eq!(
            anthropic_beta_header(&AnthropicAuth::OAuthBearer("t".into()), false),
            Some(OAUTH_BETA.to_string())
        );
    }

    #[test]
    fn oauth_with_1m_sends_both_betas_comma_joined() {
        assert_eq!(
            anthropic_beta_header(&AnthropicAuth::OAuthBearer("t".into()), true),
            Some(format!("{},{}", OAUTH_BETA, ANTHROPIC_BETA_1M_CONTEXT))
        );
    }
}
