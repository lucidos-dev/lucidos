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

pub mod adc;
mod claude;
mod gemini;

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

/// Derive the Vertex AI API host for a given `location`.
///
/// Three cases, because Vertex serves them on different hosts:
/// - `global` → the default `aiplatform.googleapis.com`.
/// - A **multi-region** (`us`, `eu`) → the dedicated
///   `aiplatform.{location}.rep.googleapis.com` host. Multi-regions are NOT
///   served by the default host nor by a `{location}-aiplatform.googleapis.com`
///   regional host — hitting those 404s with a Google robot HTML page.
/// - Any specific region (e.g. `europe-west1`) →
///   `{location}-aiplatform.googleapis.com`.
pub fn vertex_host(location: &str) -> String {
    match location {
        "global" => "aiplatform.googleapis.com".to_string(),
        "us" | "eu" => format!("aiplatform.{}.rep.googleapis.com", location),
        _ => format!("{}-aiplatform.googleapis.com", location),
    }
}

/// Get a cached Vertex access token, refreshing only when expired (50 min TTL).
/// Shared by VertexProvider, VertexImagenProvider, and the MemoryExtractor's
/// Vertex provider, so every Vertex caller benefits from the same auth.
///
/// On a cache miss the token is acquired **ADC-file-first, gcloud-fallback**:
/// 1. If the user's Application Default Credentials are an `authorized_user`
///    file (what `gcloud auth application-default login` writes), refresh the
///    token directly via the OAuth endpoint — **no `gcloud` binary**, so this
///    works in a packaged build.
/// 2. Otherwise (no ADC file, an ADC type we don't parse, or a refresh error)
///    fall back to the `gcloud auth application-default print-access-token`
///    subprocess — the dev path, unchanged.
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

    let token = fetch_access_token().await?;
    let mut guard = cache
        .lock()
        .map_err(|e| format!("Token cache mutex poisoned: {}", e))?;
    *guard = Some((token.clone(), std::time::Instant::now()));
    Ok(token)
}

/// Acquire a fresh Vertex access token (uncached). ADC-direct first
/// (packaged-friendly, no binary), then the `gcloud` subprocess fallback.
async fn fetch_access_token() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(creds) = adc::load() {
        match adc::refresh_access_token(&creds).await {
            Ok(token) => return Ok(token),
            // Don't fail outright — a stale/revoked ADC refresh should still try
            // the gcloud path (which may re-prompt / use a different config).
            Err(e) => crate::log!(
                "[Vertex] ADC token refresh failed, falling back to gcloud: {}",
                e
            ),
        }
    }

    let output = Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
        .await?;

    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    } else {
        Err(format!(
            "Failed to get Vertex access token (no usable ADC file, and gcloud failed): {}",
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

    /// Gemini `:generateContent` endpoint pinned to `locations/global`,
    /// ignoring the configured region entirely.
    ///
    /// Two callers need this, for different reasons:
    /// - `gemini-3*` models are only published globally.
    /// - Grounded web search (`search_with_grounding`), because Google Search
    ///   grounding is a global-endpoint feature.
    ///
    /// Pinning matters because the configured region is the *chat* region, and a
    /// workspace legitimately pins one that serves no Gemini models at all: the
    /// `eu` / `us` multi-regions carry Anthropic publisher models (so
    /// `vertex_region = eu` is the right setting to reach Claude there) but no
    /// Google ones, so any Gemini call routed to them 404s with
    /// `Publisher model … was not found`.
    fn global_gemini_endpoint(&self, model: &str) -> String {
        format!(
            "https://{}/v1/projects/{}/locations/global/publishers/google/models/{}:generateContent",
            vertex_host("global"),
            self.project_id,
            model
        )
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
            self.global_gemini_endpoint(model)
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
    fn vertex_host_covers_global_multiregion_and_specific_region() {
        // global → default host
        assert_eq!(vertex_host("global"), "aiplatform.googleapis.com");
        // multi-regions → dedicated .rep.googleapis.com host
        assert_eq!(vertex_host("us"), "aiplatform.us.rep.googleapis.com");
        assert_eq!(vertex_host("eu"), "aiplatform.eu.rep.googleapis.com");
        // a specific region → {region}-aiplatform.googleapis.com
        assert_eq!(
            vertex_host("europe-west1"),
            "europe-west1-aiplatform.googleapis.com"
        );
    }

    /// Grounded web search must ALWAYS hit `locations/global`, whatever region
    /// the workspace is pinned to.
    ///
    /// Regression for the 2026-07-03 break: `search_with_grounding` routed its
    /// `gemini-2.5-flash-lite` call through `endpoint_for_model`, which sends
    /// any non-`gemini-3*` model to the configured *chat* region. A workspace on
    /// `vertex_region = eu` (the correct setting to reach Claude Opus 5 there)
    /// therefore asked a multi-region that publishes no Google models for a
    /// Gemini one, and every search 404'd with `Publisher model
    /// projects/…/locations/eu/… was not found`. Verified against the live API:
    /// grounding returns 200 on `global` and on `europe-west1`, and 404 on both
    /// the `eu` and `us` multi-regions.
    #[test]
    fn grounding_endpoint_is_global_for_every_region() {
        // Every shape `vertex_host` distinguishes: the two multi-regions (which
        // serve no Gemini models at all), a specific region, and global itself.
        for region in ["eu", "us", "europe-west1", "global"] {
            let provider = VertexProvider::new(
                "my-project".into(),
                region.into(),
                "claude-opus-5".into(),
            )
            .unwrap();
            assert_eq!(
                provider.global_gemini_endpoint("gemini-2.5-flash-lite"),
                "https://aiplatform.googleapis.com/v1/projects/my-project/locations/global\
                 /publishers/google/models/gemini-2.5-flash-lite:generateContent",
                "grounding must ignore the configured region, got region={region}"
            );
        }
    }

    /// The `gemini-3*` carve-out routes through the same global builder, so the
    /// two callers can't drift apart.
    #[test]
    fn gemini_3_models_use_the_global_endpoint_builder() {
        let provider =
            VertexProvider::new("my-project".into(), "eu".into(), "gemini-3.5-flash".into())
                .unwrap();
        assert_eq!(
            provider.endpoint_for_model("gemini-3.5-flash"),
            provider.global_gemini_endpoint("gemini-3.5-flash"),
        );
    }

    /// The region-following path is deliberately unchanged: a Gemini model that
    /// is NOT grounding and NOT `gemini-3*` still resolves against the
    /// configured region. Guards the fix from over-reaching into a blanket
    /// "all Gemini is global" rule, which would silently move chat traffic.
    #[test]
    fn non_grounding_gemini_still_follows_the_configured_region() {
        let provider = VertexProvider::new(
            "my-project".into(),
            "europe-west1".into(),
            "gemini-2.5-flash".into(),
        )
        .unwrap();
        let url = provider.endpoint_for_model("gemini-2.5-flash");
        assert!(
            url.starts_with("https://europe-west1-aiplatform.googleapis.com/")
                && url.contains("/locations/europe-west1/"),
            "non-grounding Gemini must still follow the region: {url}"
        );
    }

    #[test]
    fn endpoint_multiregion_eu_uses_rep_host_and_keeps_path() {
        let provider =
            VertexProvider::new("my-project".into(), "eu".into(), "claude-opus-4-8".into())
                .unwrap();
        let url = provider.endpoint_for_model("claude-opus-4-8");
        // Host must be the multi-region REP endpoint, NOT the default host or a
        // bogus `eu-aiplatform.googleapis.com` regional host (the 404 bug).
        assert!(
            url.starts_with("https://aiplatform.eu.rep.googleapis.com/"),
            "eu multi-region must use the rep host: {}",
            url
        );
        // Path is unchanged — only the host derivation differs.
        assert!(
            url.ends_with(
                "/v1/projects/my-project/locations/eu/publishers/anthropic/models/claude-opus-4-8:streamRawPredict"
            ),
            "path must be unchanged for multi-region: {}",
            url
        );
    }

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
