//! Claude/Anthropic request dispatch for `VertexProvider`.
//!
//! The wire format (request building, cache-control, SSE parsing) is shared
//! with the direct Anthropic provider and lives in [`crate::llm::anthropic_wire`].
//! This file is the Vertex-specific transport: gcloud bearer auth, the
//! `streamRawPredict` URL, and the retry/auth-refresh loop.

use super::VertexProvider;
use crate::llm::anthropic_wire::{
    build_claude_request, parse_claude_stream, parse_context_suffix, WireTarget,
};
use crate::llm::provider::{LlmResponse, Message, TokenCallback, ToolDefinition};

impl VertexProvider {
    pub(super) async fn chat_claude(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model: &str,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Vertex carries the model in the URL, so strip the [1m] suffix for the
        // endpoint; the request builder folds the 1M beta into the body.
        let (base_model, _) = parse_context_suffix(model);
        let url = self.endpoint_for_model(base_model);

        let (request, _is_1m) = build_claude_request(
            messages,
            tools,
            model,
            system_prompt,
            reasoning_effort,
            WireTarget::Vertex,
            "Vertex",
        );

        let mut access_token = self.get_access_token().await?;

        // Retry loop for connection errors, retryable HTTP status codes, and
        // mid-stream overload errors. Content is accumulated internally by
        // parse_claude_stream, so retrying the full request is safe.
        let mut attempt = 0u32;
        let mut retried_auth = false;
        loop {
            attempt += 1;

            let builder = self
                .streaming_client
                .post(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("Content-Type", "application/json")
                .json(&request);
            let resp = match crate::llm::send_streaming_request(builder, model, attempt).await {
                crate::llm::StreamSend::Got(r) => r,
                crate::llm::StreamSend::Retry => continue,
                crate::llm::StreamSend::Failed(e) => return Err(e),
            };

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();

                if status.as_u16() == 401 && !retried_auth {
                    if let Some(new_token) =
                        self.handle_auth_refresh(model, &mut retried_auth).await
                    {
                        access_token = new_token;
                        continue;
                    }
                    return Err(format!("Claude API error ({}): {}", status, error_body).into());
                }

                if crate::llm::should_retry_http(status.as_u16(), &error_body, attempt) {
                    let delay = crate::llm::retry_delay(attempt, 1);
                    crate::llm::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                    tokio::time::sleep(delay).await;
                    continue;
                }

                return Err(crate::llm::with_retry_context(
                    format!("Claude API error ({}): {}", status, error_body),
                    attempt,
                )
                .into());
            }

            // Parse SSE stream — retry on overload errors
            match parse_claude_stream(resp, &on_token, "Vertex").await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    let err_str = e.to_string();

                    if crate::llm::is_retryable_error(&err_str)
                        && attempt <= crate::llm::MAX_RETRIES
                    {
                        let delay = crate::llm::retry_delay(attempt, 2); // longer for stream errors
                        crate::llm::log_retry(
                            model,
                            &format!("Stream error: {}", err_str),
                            attempt,
                            delay,
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(crate::llm::with_retry_context(e, attempt).into());
                }
            }
        }
    }
}
