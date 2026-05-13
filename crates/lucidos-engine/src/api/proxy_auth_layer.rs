//! `AuthLayer` trait — composable units of the proxy auth pipeline.
//! See docs/plans/2026-05-08-proxy-auth-pipeline.md

use async_trait::async_trait;
use axum::http::{HeaderName, Method};
use bytes::Bytes;
use serde_json::Value;
use std::collections::HashMap;

/// What a layer can read about the in-flight request.
///
/// Layers are pure functions of `LayerInput` → `AuthMutation`. The pipeline
/// applies mutations in order and threads `outputs` from earlier layers into
/// `prior_layer_outputs` of later ones.
#[derive(Debug, Clone)]
pub struct LayerInput<'a> {
    pub method: &'a Method,
    pub url: &'a str,
    pub headers: &'a [(HeaderName, String)],
    pub body: BodyView<'a>,
    /// JSON values produced by earlier layers in this pipeline run, keyed by
    /// the producing layer's `output_namespace()`.
    pub prior_layer_outputs: &'a HashMap<String, Value>,
}

/// Body shape exposed to a layer. ≤1MB requests get the full bytes; larger
/// requests get a precomputed SHA-256 plus length so layers can sign over a
/// digest without buffering megabytes into the layer's view. Modules that
/// REQUIRE raw bytes refuse to load if their manifest declares
/// `body_mode = "raw"` and the request exceeds the threshold.
#[derive(Debug, Clone, Copy)]
pub enum BodyView<'a> {
    Raw(&'a Bytes),
    HashOnly { sha256: &'a [u8; 32], length: u64 },
}

/// Mutations a layer wants applied to the outgoing request.
#[derive(Debug, Clone, Default)]
pub struct AuthMutation {
    pub add_headers: Vec<(HeaderName, String)>,
    /// (param_name, value) — appended to URL query string by the pipeline.
    pub add_query: Vec<(String, String)>,
    /// Replace the entire request body. Only set by layers whose module
    /// declared `replace_body` capability and were granted explicit consent.
    pub replace_body: Option<Bytes>,
    /// True iff this `apply` returned a cached value (rather than
    /// producing a fresh one). The pipeline aggregates these into
    /// `PipelineOutcome.cache_hits` so the 401-retry decision can fire
    /// when a cached layer with `retry_on_401 = InvalidateAndRetry` is
    /// the likely cause of the upstream's auth failure.
    pub cache_was_hit: bool,
    /// Outputs published into `prior_layer_outputs` for downstream layers.
    /// Keyed by the layer's `output_namespace()`.
    pub outputs: Value,
}

/// Whether a layer wants the pipeline to retry once after a 401.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetryHint {
    /// Stateless or non-retryable — re-running won't help.
    #[default]
    Never,
    /// Layer maintains a cache; on 401, invalidate it and re-run the pipeline once.
    InvalidateAndRetry,
}

#[async_trait]
pub trait AuthLayer: Send + Sync {
    /// Stable namespace under which this layer publishes outputs (becomes
    /// the key in downstream layers' `prior_layer_outputs`). Must be unique
    /// within a pipeline.
    fn output_namespace(&self) -> &str;

    /// On 401 from upstream, should the pipeline blow away this layer's
    /// cache (if any) and retry once?
    fn retry_on_401(&self) -> RetryHint {
        RetryHint::Never
    }

    /// Invalidate this layer's cache (called by the pipeline before a retry).
    /// Default: no-op for stateless layers.
    async fn invalidate_cache(&self) {}

    /// The actual work: read from `LayerInput`, return mutations. Set
    /// `AuthMutation.cache_was_hit = true` if the layer returned a cached
    /// value — the pipeline uses that with `retry_on_401()` to decide
    /// whether a 401 from upstream is worth a retry-after-invalidation.
    async fn apply(
        &self,
        input: &LayerInput<'_>,
    ) -> Result<AuthMutation, (axum::http::StatusCode, String)>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_mutation_default_is_empty() {
        let m = AuthMutation::default();
        assert!(m.add_headers.is_empty());
        assert!(m.add_query.is_empty());
        assert!(m.replace_body.is_none());
        // Default `serde_json::Value` is `Null`.
        assert!(m.outputs.is_null());
    }

    #[test]
    fn retry_hint_default_is_never() {
        assert_eq!(RetryHint::default(), RetryHint::Never);
    }
}
