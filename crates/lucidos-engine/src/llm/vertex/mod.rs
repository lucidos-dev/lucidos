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
    if location == "global" {
        "aiplatform.googleapis.com".to_string()
    } else if is_multi_region(location) {
        format!("aiplatform.{}.rep.googleapis.com", location)
    } else {
        format!("{}-aiplatform.googleapis.com", location)
    }
}

/// Is `location` one of Vertex's two multi-regions?
///
/// They carry Anthropic publisher models but no Google ones. So `vertex_region
/// = eu` is both the right setting to reach Claude there and a guaranteed 404
/// for any Gemini model. [`vertex_host`] branches on this, so the host
/// derivation and the advice cannot name different sets.
fn is_multi_region(location: &str) -> bool {
    matches!(location, "us" | "eu")
}

/// The segment after `/<name>/` in an endpoint this module built.
///
/// Read back from the URL rather than from the inputs that shaped it. A failure
/// message then cannot name a project or location the request did not use. A
/// pinned endpoint reports the location it pinned.
fn segment_after(url: &str, name: &str) -> Option<String> {
    url.split(&format!("/{name}/"))
        .nth(1)?
        .split('/')
        .next()
        .map(str::to_string)
}

/// The `locations/<name>` segment of an endpoint this module built.
///
/// `location` here, `region` in anything the user reads: this parses a URL
/// segment Google spells `locations`, while the Settings label and the
/// `VERTEX_REGION` env var both say region.
fn location_from_endpoint(url: &str) -> Option<String> {
    segment_after(url, "locations")
}

/// The `projects/<id>` segment of an endpoint this module built.
///
/// The one Vertex input with no Settings field. It resolves from
/// `VERTEX_PROJECT_ID`, then the quota project in the gcloud ADC file, then
/// `gcloud config`. So a user reading a 404 cannot otherwise see which project
/// the request was billed to.
fn project_from_endpoint(url: &str) -> Option<String> {
    segment_after(url, "projects")
}

/// Rewrite Vertex's publisher-model 404 into advice the user can act on.
///
/// That 404 has several causes and Vertex names no fix for any of them. The
/// project may be one that never enabled Claude, the model may need enabling in
/// Model Garden, or the region may be wrong. The message names the project and
/// the region it actually used, then the fixes that apply.
///
/// Naming the project is the load-bearing half. It is the one Vertex input with
/// no Settings field, so a user who is on the wrong one has nothing to compare
/// against.
///
/// Returns `None` for every other failure, so an unrelated error keeps its own
/// wording. The message appends the raw Vertex sentence, so nothing is lost.
pub(crate) fn explain_publisher_model_404(
    status: u16,
    body: &str,
    model: &str,
    url: &str,
) -> Option<String> {
    // Every live sample spells it `Publisher model`, on the Anthropic and the
    // Google publisher alike. Matched case-insensitively anyway. No other 404
    // body carries the phrase, so a wider match cannot misfire. A casing change
    // at Google would otherwise drop the advice in silence.
    if status != 404 || !body.to_ascii_lowercase().contains("publisher model") {
        return None;
    }
    let location = location_from_endpoint(url)?;
    let project = project_from_endpoint(url)?;
    let mut advice =
        format!("Vertex has no `{model}` in region `{location}` for project `{project}`.");

    let is_claude = VertexProvider::is_claude_model(model);
    if is_claude {
        advice.push_str(
            " Enable the model for that project in Vertex AI Model Garden, \
             or point Lucidos at a project that has it. \
             The project comes from VERTEX_PROJECT_ID, then the quota project in your gcloud \
             ADC file.",
        );
    }

    // Sending the user to the region setting is only honest when changing it
    // could help. Two cases where it cannot, and naming the setting would point
    // at a control that must not move.
    if VertexProvider::region_is_pinned(model) {
        advice.push_str(&format!(
            " This model always runs in `{location}`, whatever the region setting says."
        ));
    } else if !is_claude && is_multi_region(&location) {
        advice.push_str(
            " The `eu` and `us` multi-regions serve Anthropic models but no Google ones. \
             A region that reaches this model may move Claude off the one it needs.",
        );
    } else {
        advice.push_str(
            " Check the region in Settings, Models, Providers, Vertex AI. \
             That setting overrides the VERTEX_REGION environment variable.",
        );
    }

    Some(format!("{advice} Vertex said: {body}"))
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

/// Run a subprocess with a wall-clock ceiling, killing it on timeout so it
/// cannot outlive this call. Extracted so the timeout itself is testable
/// against a real wedged child, without waiting out the 60s production value.
async fn run_with_timeout(
    mut cmd: Command,
    timeout: Duration,
    label: &str,
) -> Result<std::process::Output, Box<dyn std::error::Error + Send + Sync>> {
    cmd.kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(result) => Ok(result?),
        Err(_) => Err(format!("{label} timed out after {}s", timeout.as_secs()).into()),
    }
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

    // `gcloud` does its own network token exchange. A wedged one hangs the
    // chat turn with no terminal event, the same hang class as the ADC
    // client above. So it is bounded to the same `adc::TOKEN_REQUEST_TIMEOUT`
    // (60s).
    let mut cmd = Command::new("gcloud");
    cmd.args(["auth", "application-default", "print-access-token"]);
    let output = run_with_timeout(cmd, adc::TOKEN_REQUEST_TIMEOUT, "gcloud").await?;

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

    /// Cap every HTTP attempt this provider makes at `timeout`, streaming
    /// included. Builder-style, so the ordinary constructors keep their
    /// unbounded-stream behaviour and only the caller that wants a bound pays
    /// for it.
    ///
    /// The one caller is [`crate::memory::MemoryExtractor`], which serves
    /// *auxiliary model calls*. Each of those runs under a deadline from
    /// `engine::aux_purpose`, and that deadline can only contain the provider's
    /// retries if one attempt is itself bounded. Without this, the 900s default
    /// meant the first attempt outlived any deadline worth setting, so the
    /// three retries behind it never happened.
    pub fn with_request_timeout(
        mut self,
        timeout: std::time::Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        self.client = reqwest::Client::builder()
            .timeout(timeout)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        self.streaming_client = reqwest::Client::builder()
            .timeout(timeout)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        Ok(self)
    }

    /// Snapshot the current region. URL builders call this per-request.
    fn current_location(&self) -> String {
        read_location(&self.location)
    }

    /// Which of the two Vertex request paths a model id takes: Claude, or
    /// (everything else) Gemini. `pub(crate)` because `llm::reasoning` decides
    /// a Vertex model's reasoning tiers by the same split, and a second copy of
    /// the rule would silently start offering a model the other path's tiers.
    pub(crate) fn is_claude_model(model: &str) -> bool {
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

    /// Does this model's endpoint ignore the configured region?
    ///
    /// True only for `gemini-3*`, which is published globally. Named rather
    /// than spelled inline because two things must agree: which endpoint the
    /// request takes, and whether a failure may tell the user to change the
    /// region. [`endpoint_for_model`](Self::endpoint_for_model) branches on
    /// this, so the two cannot drift.
    fn region_is_pinned(model: &str) -> bool {
        !Self::is_claude_model(model) && model.starts_with("gemini-3")
    }

    fn endpoint_for_model(&self, model: &str) -> String {
        let location = self.current_location();
        let host = vertex_host(&location);
        if Self::is_claude_model(model) {
            format!(
                "https://{}/v1/projects/{}/locations/{}/publishers/anthropic/models/{}:streamRawPredict",
                host, self.project_id, location, model
            )
        } else if Self::region_is_pinned(model) {
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

    /// A wedged subprocess (a `gcloud` that hangs on its network exchange)
    /// must be killed at the timeout rather than left running forever. Uses a
    /// real `sleep` child and a millisecond-scale timeout so the test proves
    /// the mechanism without waiting out the 60s production value.
    #[tokio::test]
    async fn a_wedged_command_is_killed_at_the_timeout() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        let err = run_with_timeout(cmd, Duration::from_millis(50), "test-command")
            .await
            .expect_err("a command that outlives its timeout must error");
        assert!(err.to_string().contains("timed out"), "got: {err}");
    }

    /// A command that finishes well inside the timeout still returns its
    /// output normally: the bound must not fire early on ordinary success.
    #[tokio::test]
    async fn a_fast_command_completes_normally() {
        let cmd = Command::new("true");
        let output = run_with_timeout(cmd, Duration::from_secs(5), "test-command")
            .await
            .expect("a fast command must not be treated as wedged");
        assert!(output.status.success());
    }

    /// The project and region in the message come from the URL the request
    /// used, for every endpoint shape this module builds. `gemini-3*` is the
    /// sharp case: it pins `global` and ignores the configured region.
    #[test]
    fn the_project_and_region_are_read_back_out_of_every_endpoint_shape() {
        let provider =
            VertexProvider::new("my-project".into(), "eu".into(), "claude-opus-5".into()).unwrap();
        for (model, expected) in [
            ("claude-opus-5@default", "eu"),
            ("gemini-2.5-flash", "eu"),
            ("gemini-3-flash-preview", "global"),
        ] {
            let url = provider.endpoint_for_model(model);
            assert_eq!(
                location_from_endpoint(&url),
                Some(expected.to_string()),
                "{model}"
            );
            assert_eq!(
                project_from_endpoint(&url),
                Some("my-project".to_string()),
                "{model}"
            );
        }
        let junk = "https://example.com/v1/nope";
        assert_eq!(location_from_endpoint(junk), None);
        assert_eq!(project_from_endpoint(junk), None);
    }

    /// The reported incident: a user pointed at a Google Cloud project that
    /// never enabled Claude, on a region that serves it fine elsewhere. Every
    /// Anthropic model 404s there, so the region is a red herring.
    ///
    /// The project is the one Vertex input with no Settings field, so a message
    /// that does not name it leaves nothing to compare against.
    #[test]
    fn the_message_names_the_project_the_request_was_billed_to() {
        let url = "https://europe-west1-aiplatform.googleapis.com/v1/projects/example-project\
                   /locations/europe-west1/publishers/anthropic/models/claude-opus-5@default\
                   :streamRawPredict";
        let body = "Publisher model `projects/example-project/locations/europe-west1/publishers\
                    /anthropic/models/claude-opus-5@default` was not found or your project does \
                    not have access to it.";

        let message = explain_publisher_model_404(404, body, "claude-opus-5@default", url)
            .expect("a publisher-model 404 must be rewritten");

        assert!(message.contains("`example-project`"), "{message}");
        assert!(message.contains("VERTEX_PROJECT_ID"), "{message}");
        assert!(
            message.contains("quota project in your gcloud ADC file"),
            "{message}"
        );
    }

    /// The 404 a fresh Vertex setup meets. Google's own sentence names neither
    /// fix, so the rewrite has to name the region, where to change it, and the
    /// per-project Model Garden step. The raw sentence is kept.
    #[test]
    fn a_claude_publisher_model_404_names_the_region_and_both_fixes() {
        let body = concat!(
            r#"[{ "error": { "code": 404, "message": "Publisher model "#,
            "`projects/example-project/locations/europe-west1/publishers/anthropic",
            r#"/models/claude-opus-5@default` was not found or your project does "#,
            r#"not have access to it.", "status": "NOT_FOUND" } } ]"#
        );
        let url = "https://europe-west1-aiplatform.googleapis.com/v1/projects/example-project\
                   /locations/europe-west1/publishers/anthropic/models/claude-opus-5@default\
                   :streamRawPredict";

        let message = explain_publisher_model_404(404, body, "claude-opus-5@default[1m]", url)
            .expect("a publisher-model 404 must be rewritten");

        assert!(message.contains("`europe-west1`"), "{message}");
        assert!(message.contains("claude-opus-5@default[1m]"), "{message}");
        assert!(
            message.contains("Settings, Models, Providers, Vertex AI"),
            "{message}"
        );
        assert!(message.contains("VERTEX_REGION"), "{message}");
        assert!(message.contains("Model Garden"), "{message}");
        assert!(message.contains(body), "the raw Vertex text must survive");
    }

    /// A `gemini-3*` endpoint pins `global` and ignores the region setting.
    /// Pointing the user at that setting would name a control that cannot move
    /// the request. It reports the pin instead.
    #[test]
    fn a_pinned_model_never_sends_the_user_to_the_region_setting() {
        let provider =
            VertexProvider::new("my-project".into(), "eu".into(), "claude-opus-5".into()).unwrap();
        let model = "gemini-3-flash-preview";
        let url = provider.endpoint_for_model(model);
        let body = "Publisher model `x` was not found";

        let message = explain_publisher_model_404(404, body, model, &url)
            .expect("a publisher-model 404 must be rewritten");

        assert!(message.contains("`global`"), "{message}");
        assert!(
            !message.contains("Settings, Models, Providers, Vertex AI"),
            "a pinned model must not be blamed on the region setting: {message}"
        );
        assert!(
            message.contains("whatever the region setting says"),
            "{message}"
        );
    }

    /// The pin rule has one spelling. `endpoint_for_model` and the advice both
    /// branch on it, so a model routed to `global` is exactly a model the
    /// advice calls pinned.
    #[test]
    fn the_pin_predicate_agrees_with_where_the_request_goes() {
        let provider =
            VertexProvider::new("my-project".into(), "eu".into(), "claude-opus-5".into()).unwrap();
        for model in [
            "claude-opus-5@default",
            "gemini-2.5-flash",
            "gemini-3-flash-preview",
            "gemini-3.5-flash",
        ] {
            let went_global = location_from_endpoint(&provider.endpoint_for_model(model))
                .as_deref()
                == Some("global");
            assert_eq!(
                VertexProvider::region_is_pinned(model),
                went_global,
                "{model}"
            );
        }
    }

    /// A Gemini model on `eu` or `us` 404s by construction: those multi-regions
    /// carry no Google publisher models. Telling the user to change the region
    /// is wrong, because `eu` is often exactly what Claude needs.
    #[test]
    fn a_gemini_404_in_a_multi_region_does_not_ask_for_the_region_setting() {
        for region in ["eu", "us"] {
            let provider =
                VertexProvider::new("my-project".into(), region.into(), "claude-opus-5".into())
                    .unwrap();
            let model = "gemini-2.5-flash";
            let url = provider.endpoint_for_model(model);
            let message =
                explain_publisher_model_404(404, "Publisher model `x` was not found", model, &url)
                    .expect("a publisher-model 404 must be rewritten");

            assert!(
                !message.contains("Check the region in Settings"),
                "{region}: changing the region would move Claude: {message}"
            );
            assert!(message.contains("no Google ones"), "{region}: {message}");
        }
    }

    /// The same model in an ordinary region DOES take the region advice: there
    /// the setting is the thing to look at, and nothing else depends on it.
    #[test]
    fn a_gemini_404_in_an_ordinary_region_still_asks_for_the_region_setting() {
        let provider = VertexProvider::new(
            "my-project".into(),
            "europe-west1".into(),
            "claude-opus-5".into(),
        )
        .unwrap();
        let model = "gemini-2.5-flash";
        let url = provider.endpoint_for_model(model);
        let message =
            explain_publisher_model_404(404, "Publisher model `x` was not found", model, &url)
                .expect("a publisher-model 404 must be rewritten");

        assert!(
            message.contains("Check the region in Settings"),
            "{message}"
        );
    }

    /// `vertex_host` and the advice read the same multi-region set, so neither
    /// can start naming a location the other does not.
    #[test]
    fn the_multi_region_set_is_the_one_the_host_derivation_uses() {
        for location in ["eu", "us"] {
            assert!(is_multi_region(location), "{location}");
            assert_eq!(
                vertex_host(location),
                format!("aiplatform.{location}.rep.googleapis.com")
            );
        }
        for location in ["global", "europe-west1", "us-central1"] {
            assert!(!is_multi_region(location), "{location}");
            assert!(
                !vertex_host(location).contains(".rep.googleapis.com"),
                "{location}"
            );
        }
    }

    /// A Google publisher model needs no Model Garden opt-in, so that sentence
    /// would be wrong advice. The region half still applies.
    #[test]
    fn a_gemini_publisher_model_404_omits_the_model_garden_step() {
        let body = "Publisher model `projects/example-project/locations/eu/publishers\
                    /google/models/gemini-2.5-flash` was not found";
        let url = "https://aiplatform.eu.rep.googleapis.com/v1/projects/example-project\
                   /locations/eu/publishers/google/models/gemini-2.5-flash:generateContent";

        let message = explain_publisher_model_404(404, body, "gemini-2.5-flash", url)
            .expect("a publisher-model 404 must be rewritten");

        assert!(message.contains("`eu`"), "{message}");
        assert!(!message.contains("Model Garden"), "{message}");
    }

    /// The match is on the phrase, not on Google's casing of it. Every live
    /// sample says `Publisher model`, so a capitalised variant would drop the
    /// advice silently, with the 404 looking untouched.
    #[test]
    fn the_phrase_is_matched_whatever_google_capitalises() {
        let url = "https://aiplatform.eu.rep.googleapis.com/v1/projects/example-project\
                   /locations/eu/publishers/anthropic/models/claude-opus-5@default\
                   :streamRawPredict";
        for body in [
            "Publisher model `x` was not found",
            "Publisher Model `x` was not found",
            "PUBLISHER MODEL `x` was not found",
        ] {
            assert!(
                explain_publisher_model_404(404, body, "claude-opus-5@default", url).is_some(),
                "{body}"
            );
        }
    }

    /// The distinction the rewrite exists to keep. A project that HAS access
    /// but no quota answers 429 in the same region. Reading that as a region
    /// problem would send the user to change a setting that is already right.
    /// A 404 from anything else keeps its own wording too.
    #[test]
    fn only_a_publisher_model_404_is_rewritten() {
        let url = "https://europe-west1-aiplatform.googleapis.com/v1/projects/example-project\
                   /locations/europe-west1/publishers/anthropic/models/claude-opus-5@default\
                   :streamRawPredict";
        let quota = "Quota exceeded for aiplatform.googleapis.com\
                     /online_prediction_input_tokens_per_minute_per_base_model";

        assert_eq!(
            explain_publisher_model_404(429, quota, "claude-opus-5@default", url),
            None
        );
        assert_eq!(
            explain_publisher_model_404(404, "Not found: some other thing", "x", url),
            None
        );
        assert_eq!(
            explain_publisher_model_404(500, "Publisher model blew up", "x", url),
            None
        );
    }

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
            let provider =
                VertexProvider::new("my-project".into(), region.into(), "claude-opus-5".into())
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
