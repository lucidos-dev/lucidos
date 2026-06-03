//! Google Vertex AI provider.
//!
//! This module owns the shared `VertexProvider` struct, auth/token plumbing,
//! endpoint construction, and the `LlmProvider` dispatch. The two request
//! paths live in child modules:
//!
//! - [`claude`] — Claude/Anthropic request-build + SSE stream-parse.
//! - [`gemini`] — Gemini request-build + response-mapping + search grounding.
//!
//! Splitting is purely structural — the public surface (`VertexProvider`,
//! `LocationHandle`, `TokenCache`, and the free `location_handle` /
//! `read_location` / `vertex_host` / `get_cached_access_token` helpers) is
//! unchanged and still reachable at `crate::llm::vertex::*`.


use crate::llm::provider::{LlmProvider, LlmResponse, Message, TokenCallback, ToolDefinition};
use async_trait::async_trait;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

mod claude;
mod gemini;

/// Map a unified `reasoning_effort` string to the thinking-budget token count
/// shared by the Claude `budget_tokens` field and Gemini-3
/// `thinkingConfig.thinkingBudget`. Unknown values fall back to the "high"
/// budget — the same default both call sites picked independently before
/// this was DRYed up.
fn thinking_budget_for_effort(effort: &str) -> u32 {
    match effort {
        "low" => 4096,
        "medium" => 8192,
        "high" => 16384,
        "xhigh" => 24576,
        "max" => 32768,
        _ => 16384,
    }
}

/// Shared access token cache for all VertexProvider instances in the same project.
/// gcloud tokens are project-scoped, so one cache serves all models.
pub type TokenCache = Arc<std::sync::Mutex<Option<(String, std::time::Instant)>>>;

/// Shared handle to the current Vertex AI region. Updated in place by the
/// engine when `vertex_region` changes; provider clones read the new value
/// on their next API call.
pub type LocationHandle = Arc<std::sync::RwLock<String>>;

/// Construct a `LocationHandle` from an initial region string.
pub fn location_handle(initial: String) -> LocationHandle {
    Arc::new(std::sync::RwLock::new(initial))
}

/// Snapshot the current region. Returns `"global"` if the lock is poisoned —
/// poisoning means a writer panicked mid-update, which should never happen
/// (writers only call `*guard = new`), but readers still shouldn't take down
/// every LLM and image call if it does.
pub fn read_location(handle: &LocationHandle) -> String {
    handle.read().map(|g| g.clone()).unwrap_or_else(|e| {
        crate::log!(
            "[Vertex] location lock poisoned, falling back to global: {}",
            e
        );
        "global".to_string()
    })
}

pub fn vertex_host(location: &str) -> String {
    if location == "global" {
        "aiplatform.googleapis.com".to_string()
    } else {
        format!("{}-aiplatform.googleapis.com", location)
    }
}

/// Get a cached gcloud access token, refreshing only when expired (50 min TTL).
/// Shared by VertexProvider and VertexImagenProvider. The `gcloud` subprocess
/// runs through `tokio::process::Command` so the runtime worker stays free
/// during the (cached, rare) refresh.
pub async fn get_cached_access_token(
    cache: &TokenCache,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    const TOKEN_TTL: Duration = Duration::from_secs(3000);

    {
        let guard = cache
            .lock()
            .map_err(|e| format!("Token cache mutex poisoned: {}", e))?;
        if let Some((ref token, ref fetched_at)) = *guard {
            if fetched_at.elapsed() < TOKEN_TTL {
                return Ok(token.clone());
            }
        }
    }

    let output = Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
        .await?;

    if output.status.success() {
        let token = String::from_utf8(output.stdout)?.trim().to_string();
        let mut guard = cache
            .lock()
            .map_err(|e| format!("Token cache mutex poisoned: {}", e))?;
        *guard = Some((token.clone(), std::time::Instant::now()));
        Ok(token)
    } else {
        Err(format!(
            "Failed to get access token: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

#[derive(Clone)]
pub struct VertexProvider {
    project_id: String,
    location: LocationHandle,
    model: String,
    client: reqwest::Client,
    /// Client without per-request timeout, used for Claude streaming where
    /// we apply per-chunk timeouts instead.
    streaming_client: reqwest::Client,
    /// Shared cached access token with expiry (tokens last 3600s, refresh at 3000s)
    token_cache: TokenCache,
}

impl VertexProvider {
    /// Build the provider; returns `Err` if either reqwest builder rejects
    /// the configuration so the engine can fail at startup with a logged reason.
    pub fn new(
        project_id: String,
        location: String,
        model: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::with_location_handle(
            project_id,
            location_handle(location),
            model,
            Arc::new(std::sync::Mutex::new(None)),
        )
    }

    pub fn with_location_handle(
        project_id: String,
        location: LocationHandle,
        model: String,
        token_cache: TokenCache,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(900))
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        let streaming_client = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            project_id,
            location,
            model,
            client,
            streaming_client,
            token_cache,
        })
    }

    /// Snapshot the current region. URL builders call this per-request.
    fn current_location(&self) -> String {
        read_location(&self.location)
    }

    fn is_claude_model(model: &str) -> bool {
        model.starts_with("claude")
    }

    /// Strip `[1m]` suffix from model ID, returning (base_model, is_1m_context).
    fn parse_context_suffix(model: &str) -> (&str, bool) {
        if let Some(base) = model.strip_suffix("[1m]") {
            (base, true)
        } else {
            (model, false)
        }
    }

    /// Models that support extended thinking (reasoning goes to dedicated blocks
    /// instead of polluting the text response).
    fn supports_extended_thinking(model: &str) -> bool {
        model.contains("claude-3-7-sonnet")
            || model.contains("claude-sonnet-4")
            || model.contains("claude-opus-4")
    }

    /// Opus 4.7+ only supports adaptive thinking (no budget_tokens, no
    /// temperature/top_p/top_k). Effort is controlled via output_config.effort.
    fn requires_adaptive_thinking(model: &str) -> bool {
        model.contains("claude-opus-4-7") || model.contains("claude-opus-4-8")
    }

    fn endpoint_for_model(&self, model: &str) -> String {
        let location = self.current_location();
        let host = vertex_host(&location);
        if Self::is_claude_model(model) {
            format!(
                "https://{}/v1/projects/{}/locations/{}/publishers/anthropic/models/{}:streamRawPredict",
                host, self.project_id, location, model
            )
        } else if model.starts_with("gemini-3") {
            format!(
                "https://aiplatform.googleapis.com/v1/projects/{}/locations/global/publishers/google/models/{}:generateContent",
                self.project_id, model
            )
        } else {
            format!(
                "https://{}/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
                host, self.project_id, location, model
            )
        }
    }

    async fn get_access_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        get_cached_access_token(&self.token_cache).await
    }

    /// Invalidate the cached token and refresh it. Returns the new token on success.
    /// `retried_auth` is flipped to true to prevent infinite retry loops.
    async fn handle_auth_refresh(&self, model: &str, retried_auth: &mut bool) -> Option<String> {
        *retried_auth = true;
        if let Ok(mut cache) = self.token_cache.lock() {
            *cache = None;
        }
        match get_cached_access_token(&self.token_cache).await {
            Ok(new_token) => {
                log!("[{}] HTTP 401, refreshed access token and retrying", model);
                Some(new_token)
            }
            Err(e) => {
                log!("[{}] HTTP 401 and failed to refresh token: {}", model, e);
                None
            }
        }
    }

    /// Send an HTTP request with retry on retryable status codes and network errors.
    /// On 401, invalidates the cached token, refreshes it, and retries once.
    async fn request_with_retry(
        &self,
        model: &str,
        url: &str,
        access_token: &str,
        body: &impl Serialize,
    ) -> Result<(reqwest::StatusCode, String), Box<dyn std::error::Error + Send + Sync>> {
        let mut attempt = 0u32;
        let mut token = access_token.to_string();
        let mut retried_auth = false;
        loop {
            attempt += 1;

            let response = match self
                .client
                .post(url)
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .json(body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    if attempt <= super::MAX_RETRIES {
                        let delay = super::retry_delay(attempt, 1);
                        super::log_retry(model, &format!("Network error: {:?}", e), attempt, delay);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(super::with_retry_context(e, attempt).into());
                }
            };

            let status = response.status();
            let response_body = response.text().await?;

            if status.as_u16() == 401 && !retried_auth {
                if let Some(new_token) = self.handle_auth_refresh(model, &mut retried_auth).await {
                    token = new_token;
                    continue;
                }
                return Ok((status, response_body));
            }

            if super::should_retry_http(status.as_u16(), &response_body, attempt) {
                let delay = super::retry_delay(attempt, 1);
                super::log_retry(model, &format!("HTTP {}", status), attempt, delay);
                tokio::time::sleep(delay).await;
                continue;
            }

            return Ok((status, response_body));
        }
    }
}

#[async_trait]
impl LlmProvider for VertexProvider {
    fn default_model(&self) -> &str {
        &self.model
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        model_override: Option<&str>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        reasoning_effort: Option<&str>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let model = model_override.unwrap_or(&self.model);
        if Self::is_claude_model(model) {
            self.chat_claude(
                messages,
                tools,
                model,
                system_prompt,
                on_token,
                reasoning_effort,
            )
            .await
        } else {
            self.chat_gemini(
                messages,
                tools,
                model,
                system_prompt,
                on_token,
                reasoning_effort,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_global_location_no_region_prefix() {
        let provider = VertexProvider::new(
            "my-project".into(),
            "global".into(),
            "claude-opus-4-6".into(),
        )
        .unwrap();
        let url = provider.endpoint_for_model("claude-opus-4-6");
        assert!(
            url.starts_with("https://aiplatform.googleapis.com/"),
            "global should not have region prefix: {}",
            url
        );
        assert!(
            url.contains("/locations/global/"),
            "path should still use locations/global: {}",
            url
        );
    }

    #[test]
    fn endpoint_regional_location_has_prefix() {
        let provider = VertexProvider::new(
            "my-project".into(),
            "europe-west1".into(),
            "claude-opus-4-6".into(),
        )
        .unwrap();
        let url = provider.endpoint_for_model("claude-opus-4-6");
        assert!(
            url.starts_with("https://europe-west1-aiplatform.googleapis.com/"),
            "regional should have prefix: {}",
            url
        );
        assert!(
            url.contains("/locations/europe-west1/"),
            "path should use the region: {}",
            url
        );
    }

    #[test]
    fn endpoint_gemini_global_location() {
        let provider = VertexProvider::new(
            "my-project".into(),
            "global".into(),
            "gemini-2.5-flash".into(),
        )
        .unwrap();
        let url = provider.endpoint_for_model("gemini-2.5-flash");
        assert!(
            url.starts_with("https://aiplatform.googleapis.com/"),
            "global gemini should not have region prefix: {}",
            url
        );
        assert!(
            url.contains("/locations/global/"),
            "path should use locations/global: {}",
            url
        );
    }

    #[test]
    fn endpoint_gemini3_always_global() {
        let provider = VertexProvider::new(
            "my-project".into(),
            "us-central1".into(),
            "gemini-3-flash-preview".into(),
        )
        .unwrap();
        let url = provider.endpoint_for_model("gemini-3-flash-preview");
        assert!(
            url.starts_with("https://aiplatform.googleapis.com/"),
            "gemini-3 always uses global host: {}",
            url
        );
        assert!(
            url.contains("/locations/global/"),
            "gemini-3 always uses locations/global: {}",
            url
        );
    }

    #[test]
    fn parse_context_suffix_strips_1m() {
        assert_eq!(
            VertexProvider::parse_context_suffix("claude-opus-4-6[1m]"),
            ("claude-opus-4-6", true)
        );
        assert_eq!(
            VertexProvider::parse_context_suffix("claude-sonnet-4-6[1m]"),
            ("claude-sonnet-4-6", true)
        );
    }

    #[test]
    fn parse_context_suffix_preserves_base_model() {
        assert_eq!(
            VertexProvider::parse_context_suffix("claude-opus-4-6"),
            ("claude-opus-4-6", false)
        );
        assert_eq!(
            VertexProvider::parse_context_suffix("gemini-2.5-pro"),
            ("gemini-2.5-pro", false)
        );
    }

    #[test]
    fn endpoint_reflects_live_location_handle_updates() {
        let handle = location_handle("europe-west1".into());
        let provider = VertexProvider::with_location_handle(
            "my-project".into(),
            handle.clone(),
            "claude-opus-4-6".into(),
            Arc::new(std::sync::Mutex::new(None)),
        )
        .unwrap();

        let before = provider.endpoint_for_model("claude-opus-4-6");
        assert!(
            before.contains("europe-west1") && before.contains("/locations/europe-west1/"),
            "initial URL must reflect handle's starting region, got: {}",
            before
        );

        *handle.write().unwrap() = "us-central1".into();

        let after = provider.endpoint_for_model("claude-opus-4-6");
        assert!(
            after.contains("us-central1") && after.contains("/locations/us-central1/"),
            "URL must pick up updated handle without rebuilding the provider, got: {}",
            after
        );
        assert!(
            !after.contains("europe-west1"),
            "URL must not still contain the old region, got: {}",
            after
        );
    }
}
