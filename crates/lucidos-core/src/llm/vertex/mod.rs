use crate::llm::call_trace;
use crate::llm::provider::{
    LlmProvider, LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition,
};
use async_trait::async_trait;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

pub use super::vertex_auth::{adc, get_cached_access_token, TokenCache};
mod claude;
mod gemini;
pub use super::vertex_defaults::sdk_request_defaults;

pub type LocationHandle = Arc<std::sync::RwLock<String>>;

pub fn location_handle(initial: String) -> LocationHandle {
    Arc::new(std::sync::RwLock::new(initial))
}

pub fn read_location(handle: &LocationHandle) -> String {
    handle.read().map(|g| g.clone()).unwrap_or_else(|e| {
        crate::log_legacy!(
            "[Vertex] location lock poisoned, falling back to global: {}",
            e
        );
        "global".to_string()
    })
}

/// INVARIANT: Derive the Vertex AI API host for a given `location`.
///
/// Three cases, because Vertex serves them on different hosts:
/// - `global` → the default `aiplatform.googleapis.com`.
/// - A **multi-region** (`us`, `eu`) → the dedicated
///   `aiplatform.{location}.rep.googleapis.com` host. Multi-regions are NOT
///   served by the default host nor by a `{location}-aiplatform.googleapis.com`
///   regional host — hitting those 404s with a Google robot HTML page.
/// - Any specific region (e.g. `europe-west1`) →
///   `{location}-aiplatform.googleapis.com`.
///
/// A location the caller made up falls back to `global`, loudly. `vertex_region`
/// is an ordinary settable preference, so this string is caller data, and it is
/// interpolated straight into a host. Left alone, `evil.test#` builds
/// `https://evil.test#-aiplatform.googleapis.com`, whose host is `evil.test`,
/// and a live ADC access token follows it there.
pub fn vertex_host(location: &str) -> String {
    let lowered = location.to_ascii_lowercase();
    let location = well_formed_location(&lowered);
    if location == "global" {
        VERTEX_GLOBAL_HOST.to_string()
    } else if is_multi_region(location) {
        format!("aiplatform.{}{}", location, VERTEX_MULTI_REGION_SUFFIX)
    } else {
        format!("{}-{}", location, VERTEX_GLOBAL_HOST)
    }
}

pub const GROUNDING_MODEL: &str = "gemini-3.1-flash-lite";

const VERTEX_GLOBAL_HOST: &str = "aiplatform.googleapis.com";
const VERTEX_MULTI_REGION_SUFFIX: &str = ".rep.googleapis.com";

pub fn is_vertex_host(host: &str) -> bool {
    host == VERTEX_GLOBAL_HOST
        || host.ends_with(&format!("-{VERTEX_GLOBAL_HOST}"))
        || (host.starts_with("aiplatform.") && host.ends_with(VERTEX_MULTI_REGION_SUFFIX))
}

fn well_formed_location(location: &str) -> &str {
    let ok = !location.is_empty()
        && location
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        return location;
    }
    crate::log_legacy!(
        "[Vertex] refusing malformed region {:?}; using global instead",
        location
    );
    "global"
}

fn is_multi_region(location: &str) -> bool {
    matches!(location, "us" | "eu")
}

fn segment_after(url: &str, name: &str) -> Option<String> {
    url.split(&format!("/{name}/"))
        .nth(1)?
        .split('/')
        .next()
        .map(str::to_string)
}

fn location_from_endpoint(url: &str) -> Option<String> {
    segment_after(url, "locations")
}

fn project_from_endpoint(url: &str) -> Option<String> {
    segment_after(url, "projects")
}

pub(crate) fn explain_publisher_model_404(
    status: u16,
    body: &str,
    model: &str,
    url: &str,
) -> Option<String> {
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
        advice.push_str(concat!(
            " Check the region in ",
            providers_settings!(),
            ", Vertex AI. That setting overrides the VERTEX_REGION environment variable.",
        ));
    }

    Some(format!("{advice} Vertex said: {body}"))
}

#[derive(Clone)]
pub struct VertexProvider {
    project_id: String,
    location: LocationHandle,
    model: String,
    client: reqwest::Client,
    /// INVARIANT: Client without per-request timeout, used for Claude streaming where
    /// we apply per-chunk timeouts instead.
    streaming_client: reqwest::Client,
    token_cache: TokenCache,
}

impl VertexProvider {
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

    fn current_location(&self) -> String {
        read_location(&self.location)
    }

    pub fn is_claude_model(model: &str) -> bool {
        super::claude_model::is_vertex_claude_model(model)
    }

    /// INVARIANT: Gemini `:generateContent` endpoint pinned to `locations/global`,
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

    fn region_is_pinned(model: &str) -> bool {
        !Self::is_claude_model(model)
            && (model.starts_with("gemini-3")
                || crate::llm::model_registry::unified_output_limit(model).is_some())
    }

    fn endpoint_for_model(&self, model: &str) -> String {
        let location = self.current_location();
        let host = vertex_host(&location);
        if Self::is_claude_model(model) {
            self.claude_endpoint(model)
        } else if Self::region_is_pinned(model) {
            self.global_gemini_endpoint(model)
        } else {
            format!(
                "https://{}/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
                host, self.project_id, location, model
            )
        }
    }

    fn claude_endpoint(&self, model: &str) -> String {
        let location = self.current_location();
        format!(
            "https://{}/v1/projects/{}/locations/{}/publishers/anthropic/models/{}:streamRawPredict",
            vertex_host(&location),
            self.project_id,
            location,
            model
        )
    }

    async fn get_access_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        get_cached_access_token(&self.token_cache).await
    }

    async fn handle_auth_refresh(&self, model: &str, retried_auth: &mut bool) -> Option<String> {
        *retried_auth = true;
        if let Ok(mut cache) = self.token_cache.lock() {
            *cache = None;
        }
        match get_cached_access_token(&self.token_cache).await {
            Ok(new_token) => {
                log_legacy!("[{}] HTTP 401, refreshed access token and retrying", model);
                Some(new_token)
            }
            Err(e) => {
                log_legacy!("[{}] HTTP 401 and failed to refresh token: {}", model, e);
                None
            }
        }
    }

    async fn request_with_retry(
        &self,
        model: &str,
        url: &str,
        access_token: &str,
        body: &impl Serialize,
        attempt_timeout: Option<std::time::Duration>,
    ) -> Result<(reqwest::StatusCode, String), Box<dyn std::error::Error + Send + Sync>> {
        crate::llm::metered::check_provider(crate::llm::ProviderKind::Vertex).await?;
        let mut attempt = 0u32;
        let mut token = access_token.to_string();
        let mut retried_auth = false;
        loop {
            attempt += 1;

            let request = self
                .client
                .post(url)
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .json(body);
            call_trace::attempt_began();
            let response = match super::cap_attempt(request, attempt_timeout).send().await {
                Ok(r) => {
                    call_trace::response_received(
                        r.status().as_u16(),
                        call_trace::request_id_of(r.headers()).as_deref(),
                    );
                    r
                }
                Err(e) => {
                    call_trace::transport_failed(if e.is_timeout() {
                        call_trace::CallFailure::Timeout
                    } else {
                        call_trace::CallFailure::Connection
                    });
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
            let error_headers = response.headers().clone();
            let response_body = response.text().await?;
            if !status.is_success() {
                if let Some(cap) = super::metered::http_spend_cap(
                    super::ProviderKind::Vertex,
                    status.as_u16(),
                    &error_headers,
                    &response_body,
                )
                .await
                {
                    return Err(cap.into());
                }
            }

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
    fn resolved_provider_kind(
        &self,
        _selection: &ModelSelection<'_>,
    ) -> Result<Option<crate::llm::ProviderKind>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Some(crate::llm::ProviderKind::Vertex))
    }

    fn default_model(&self) -> &str {
        &self.model
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let model = selection.model.unwrap_or(&self.model);
        if Self::is_claude_model(model) {
            self.chat_claude(messages, tools, selection, system_prompt, on_token)
                .await
        } else {
            self.chat_gemini(messages, tools, selection, system_prompt, on_token)
                .await
        }
    }

    async fn prefill(
        &self,
        body: &crate::llm::prefill::PrefillBody,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.prefill_claude(body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_project_and_region_are_read_back_out_of_every_endpoint_shape() {
        let provider =
            VertexProvider::new("my-project".into(), "eu".into(), "claude-opus-5".into()).unwrap();
        for (model, expected) in [
            ("claude-opus-5@default", "eu"),
            ("claude-sonnet-5", "eu"),
            ("claude-sonnet-5-5", "eu"),
            ("claude-haiku-5-5", "eu"),
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
            message.contains(concat!(providers_settings!(), ", Vertex AI")),
            "{message}"
        );
        assert!(message.contains("VERTEX_REGION"), "{message}");
        assert!(message.contains("Model Garden"), "{message}");
        assert!(message.contains(body), "the raw Vertex text must survive");
    }

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
            !message.contains(concat!(providers_settings!(), ", Vertex AI")),
            "a pinned model must not be blamed on the region setting: {message}"
        );
        assert!(
            message.contains("whatever the region setting says"),
            "{message}"
        );
    }

    #[test]
    fn the_pin_predicate_agrees_with_where_the_request_goes() {
        let provider =
            VertexProvider::new("my-project".into(), "eu".into(), "claude-opus-5".into()).unwrap();
        for model in [
            "claude-opus-5@default",
            "claude-sonnet-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "gemini-2.5-flash",
            "gemini-3-flash-preview",
            "gemini-3.8-flash",
            "gemini-4-argon",
            "gemini-4-carbon",
            "gemini-4-neon",
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
    fn every_built_vertex_host_is_recognised() {
        for location in ["global", "eu", "us", "europe-west1"] {
            assert!(is_vertex_host(&vertex_host(location)), "{location}");
        }
        assert!(!is_vertex_host("storage.googleapis.com"));
    }

    #[test]
    fn vertex_host_covers_global_multiregion_and_specific_region() {
        assert_eq!(vertex_host("global"), "aiplatform.googleapis.com");
        assert_eq!(vertex_host("us"), "aiplatform.us.rep.googleapis.com");
        assert_eq!(vertex_host("eu"), "aiplatform.eu.rep.googleapis.com");
        assert_eq!(
            vertex_host("europe-west1"),
            "europe-west1-aiplatform.googleapis.com"
        );
    }

    #[test]
    fn a_malformed_region_cannot_choose_the_host() {
        assert_eq!(
            vertex_host("EUROPE-WEST1"),
            "europe-west1-aiplatform.googleapis.com"
        );
        for region in [
            "evil.test#",
            "evil.test/",
            "evil.test?",
            "user@evil.test",
            "evil.test:443",
            "eu rope",
            "",
        ] {
            let url = format!("https://{}/v1/projects/p", vertex_host(region));
            let parsed = reqwest::Url::parse(&url)
                .unwrap_or_else(|e| panic!("region {region:?} built an unparseable URL: {e}"));
            let host = parsed.host_str().unwrap_or_default();
            assert!(
                host.ends_with("googleapis.com"),
                "region {region:?} chose host {host}"
            );
        }
    }

    #[test]
    fn grounding_endpoint_is_global_for_every_region() {
        for region in ["eu", "us", "europe-west1", "global"] {
            let provider =
                VertexProvider::new("my-project".into(), region.into(), "claude-opus-5".into())
                    .unwrap();
            assert_eq!(
                provider.global_gemini_endpoint(GROUNDING_MODEL),
                format!(
                    "https://aiplatform.googleapis.com/v1/projects/my-project/locations/global\
                     /publishers/google/models/{GROUNDING_MODEL}:generateContent"
                ),
                "grounding must ignore the configured region, got region={region}"
            );
        }
    }

    #[test]
    fn gemini_3_models_use_the_global_endpoint_builder() {
        let provider =
            VertexProvider::new("my-project".into(), "eu".into(), "gemini-3.8-flash".into())
                .unwrap();
        assert_eq!(
            provider.endpoint_for_model("gemini-3.8-flash"),
            provider.global_gemini_endpoint("gemini-3.8-flash"),
        );
    }

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
        assert!(
            url.starts_with("https://aiplatform.eu.rep.googleapis.com/"),
            "eu multi-region must use the rep host: {}",
            url
        );
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

    #[tokio::test]
    async fn a_vertex_call_traces_its_attempts_and_status() {
        let app = axum::Router::new().fallback(|| async { (axum::http::StatusCode::OK, "{}") });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/call", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider =
            VertexProvider::new("my-project".into(), "eu".into(), "gemini-3.5-flash".into())
                .unwrap();

        let (outcome, trace) = crate::llm::call_trace::scoped(provider.request_with_retry(
            "gemini-3.5-flash",
            &url,
            "token",
            &serde_json::json!({}),
            None,
        ))
        .await;

        outcome.expect("the server answers");
        assert_eq!(trace.attempts, 1);
        assert_eq!(trace.status, Some(200));
    }
}
