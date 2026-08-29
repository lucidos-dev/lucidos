//! Static credential layers — Bearer / ApiKey / Basic share a single
//! `StaticHeaderLayer` (they differ only in header name + value
//! formatting, computed at construction time). QueryParam is its own
//! layer because it injects into the URL and publishes a redacted
//! `log_url` so credentials never reach logs.
//!
//! Credential values are read at pipeline-build time so rotations are
//! picked up on the next request without engine state.

use crate::api::proxy_auth_layer::{AuthLayer, AuthMutation, LayerInput, ScopeBinding};
use async_trait::async_trait;
use axum::http::{HeaderName, StatusCode};

/// Adds one fixed header to the outgoing request. Bearer / ApiKey / Basic
/// auth all reduce to "compute the header value once at construction,
/// emit it on every apply" — see the `bearer`, `api_key`, `basic`
/// constructors.
///
/// Every constructor takes the [`ScopeBinding`] for the value it carries. The
/// caller has to say where the secret came from, because that is the only
/// thing the gate can check it against.
pub struct StaticHeaderLayer {
    namespace: String,
    header_name: HeaderName,
    value: String,
    binding: ScopeBinding,
}

impl StaticHeaderLayer {
    /// `Authorization: Bearer <token>`.
    pub fn bearer(namespace: String, token: String, binding: ScopeBinding) -> Self {
        Self {
            namespace,
            header_name: HeaderName::from_static("authorization"),
            value: format!("Bearer {token}"),
            binding,
        }
    }

    /// Custom header (e.g. `X-API-Key`) with the credential as the bare
    /// value. `header_name_str` is validated and lowercased by `HeaderName`.
    pub fn api_key(
        namespace: String,
        header_name_str: &str,
        value: String,
        binding: ScopeBinding,
    ) -> Result<Self, String> {
        let header_name = HeaderName::try_from(header_name_str)
            .map_err(|e| format!("invalid header name '{header_name_str}': {e}"))?;
        Ok(Self {
            namespace,
            header_name,
            value,
            binding,
        })
    }

    /// `Authorization: Basic base64(user:password)`.
    pub fn basic(namespace: String, username: &str, password: &str, binding: ScopeBinding) -> Self {
        use base64::Engine;
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
        Self {
            namespace,
            header_name: HeaderName::from_static("authorization"),
            value: format!("Basic {encoded}"),
            binding,
        }
    }
}

#[async_trait]
impl AuthLayer for StaticHeaderLayer {
    fn output_namespace(&self) -> &str {
        &self.namespace
    }
    fn scope_bindings(&self) -> Vec<ScopeBinding> {
        vec![self.binding.clone()]
    }
    async fn apply(&self, _input: &LayerInput<'_>) -> Result<AuthMutation, (StatusCode, String)> {
        Ok(AuthMutation {
            add_headers: vec![(self.header_name.clone(), self.value.clone())],
            outputs: serde_json::json!({}),
            ..Default::default()
        })
    }
}

pub struct QueryParamLayer {
    namespace: String,
    param: String,
    value: String,
    binding: ScopeBinding,
}

impl QueryParamLayer {
    pub fn new(namespace: String, param: String, value: String, binding: ScopeBinding) -> Self {
        Self {
            namespace,
            param,
            value,
            binding,
        }
    }
}

#[async_trait]
impl AuthLayer for QueryParamLayer {
    fn output_namespace(&self) -> &str {
        &self.namespace
    }
    fn scope_bindings(&self) -> Vec<ScopeBinding> {
        vec![self.binding.clone()]
    }
    async fn apply(&self, input: &LayerInput<'_>) -> Result<AuthMutation, (StatusCode, String)> {
        // Credentials in query strings must be redacted before logs/error
        // bodies — publish a redacted `log_url` for the pipeline runner to
        // swap into `PipelineOutcome.log_url`.
        let redacted = crate::api::proxy::append_query_param(input.url, &self.param, "REDACTED");
        Ok(AuthMutation {
            add_query: vec![(self.param.clone(), self.value.clone())],
            outputs: serde_json::json!({"log_url_replacement": redacted}),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::proxy_auth_layer::*;
    use axum::http::Method;
    use bytes::Bytes;
    use std::collections::HashMap;

    fn input_for<'a>(
        body: &'a Bytes,
        url: &'a str,
        prior: &'a HashMap<String, serde_json::Value>,
    ) -> LayerInput<'a> {
        LayerInput {
            method: &Method::GET,
            url,
            headers: &[],
            body: BodyView::Raw(body),
            prior_layer_outputs: prior,
        }
    }

    #[tokio::test]
    async fn bearer_emits_authorization_header() {
        let layer = StaticHeaderLayer::bearer(
            "test-cred".into(),
            "secret123".into(),
            ScopeBinding::StoredCredential("test-cred".into()),
        );
        let body = Bytes::new();
        let prior = HashMap::new();
        let input = input_for(&body, "https://example.com/x", &prior);
        let m = layer.apply(&input).await.unwrap();
        assert_eq!(m.add_headers.len(), 1);
        assert_eq!(m.add_headers[0].0.as_str(), "authorization");
        assert_eq!(m.add_headers[0].1, "Bearer secret123");
    }

    #[tokio::test]
    async fn api_key_emits_named_header() {
        let layer = StaticHeaderLayer::api_key(
            "svc".into(),
            "x-api-key",
            "abc-def".into(),
            ScopeBinding::StoredCredential("svc".into()),
        )
        .unwrap();
        let body = Bytes::new();
        let prior = HashMap::new();
        let input = input_for(&body, "https://example.com/x", &prior);
        let m = layer.apply(&input).await.unwrap();
        assert_eq!(m.add_headers.len(), 1);
        assert_eq!(m.add_headers[0].0.as_str(), "x-api-key");
        assert_eq!(m.add_headers[0].1, "abc-def");
    }

    #[test]
    fn api_key_rejects_invalid_header_name() {
        match StaticHeaderLayer::api_key(
            "svc".into(),
            "bad header",
            "v".into(),
            ScopeBinding::StoredCredential("svc".into()),
        ) {
            Err(err) => assert!(err.contains("invalid header name"), "msg was: {err}"),
            Ok(_) => panic!("expected api_key constructor to reject 'bad header'"),
        }
    }

    #[tokio::test]
    async fn basic_emits_base64_basic_authorization() {
        let layer = StaticHeaderLayer::basic(
            "svc".into(),
            "user",
            "pass",
            ScopeBinding::StoredCredential("svc".into()),
        );
        let body = Bytes::new();
        let prior = HashMap::new();
        let input = input_for(&body, "https://example.com/x", &prior);
        let m = layer.apply(&input).await.unwrap();
        assert_eq!(m.add_headers[0].0.as_str(), "authorization");
        // base64("user:pass") = "dXNlcjpwYXNz"
        assert_eq!(m.add_headers[0].1, "Basic dXNlcjpwYXNz");
    }

    #[tokio::test]
    async fn query_param_layer_publishes_redacted_log_url() {
        let layer = QueryParamLayer::new(
            "svc".into(),
            "api-key".into(),
            "actual-secret".into(),
            ScopeBinding::StoredCredential("svc".into()),
        );
        let body = Bytes::new();
        let prior = HashMap::new();
        let input = input_for(&body, "https://example.com/data", &prior);
        let m = layer.apply(&input).await.unwrap();
        assert_eq!(m.add_query.len(), 1);
        assert_eq!(m.add_query[0], ("api-key".into(), "actual-secret".into()));
        let log_url = m.outputs["log_url_replacement"].as_str().unwrap();
        assert!(!log_url.contains("actual-secret"));
        assert!(log_url.contains("REDACTED"));
    }
}
