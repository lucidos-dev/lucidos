//! Pipeline runner for `Vec<Box<dyn AuthLayer>>`. Threads `prior_layer_outputs`
//! through layers in declared order, drives one-shot 401 retry across
//! opted-in cache hits.
//!
//! `RAW_BODY_THRESHOLD_BYTES` switches the body view: requests ≤1MB get the
//! full bytes (`BodyView::Raw`), larger ones get a SHA-256 + length
//! (`BodyView::HashOnly`) so signers don't have to buffer megabytes for a
//! signature they only compute over the digest.

use crate::api::proxy_auth_layer::{AuthLayer, BodyView, RetryHint};
use axum::http::{HeaderName, Method, StatusCode};
use bytes::Bytes;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Result of running the pipeline once. The pipeline merges per-layer
/// `AuthMutation`s and exposes the merged result here.
#[derive(Debug, Clone, Default)]
pub struct PipelineOutcome {
    pub headers: Vec<(HeaderName, String)>,
    pub query: Vec<(String, String)>,
    pub replace_body: Option<Bytes>,
    /// URL safe to write to logs and error responses — credentials in
    /// the query string (e.g. `QueryParamLayer`'s value) are replaced
    /// with `REDACTED`. Layers that fold a credential into the URL MUST
    /// publish a redacted form via `outputs.log_url_replacement`; layers
    /// that only mutate headers leave this equal to the request URL.
    pub log_url: String,
    /// Per-layer outputs keyed by namespace, in declared order.
    pub outputs: Value,
    /// For each layer (in order): true iff the layer's `apply` returned
    /// `AuthMutation { cache_was_hit: true, .. }`.
    pub cache_hits: Vec<bool>,
}

/// 1MB threshold matches the pipeline body-handling decision.
pub const RAW_BODY_THRESHOLD_BYTES: usize = 1024 * 1024;

/// Compute SHA-256 of a body. Used by `run_pipeline`'s >1MB hash-only path.
pub fn body_hash(body: &Bytes) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(body);
    h.finalize().into()
}

/// Run all layers in order; merge results.
///
/// Body-mode handling: requests at or under `RAW_BODY_THRESHOLD_BYTES` (1MB)
/// get `BodyView::Raw(&body)`; larger ones get `BodyView::HashOnly` with a
/// precomputed SHA-256 + length, so layers can sign over the digest without
/// buffering the full body into their own view. The hash buffer is owned
/// here so its borrow lives for the whole pipeline run.
pub async fn run_pipeline(
    layers: &[Arc<dyn AuthLayer>],
    method: &Method,
    url: &str,
    headers: &[(HeaderName, String)],
    body: &Bytes,
) -> Result<PipelineOutcome, (StatusCode, String)> {
    // Pre-hash large bodies so the borrow lives for the entire loop.
    let hash_buf;
    let body_view = if body.len() <= RAW_BODY_THRESHOLD_BYTES {
        BodyView::Raw(body)
    } else {
        hash_buf = body_hash(body);
        BodyView::HashOnly {
            sha256: &hash_buf,
            length: body.len() as u64,
        }
    };
    let mut prior: HashMap<String, Value> = HashMap::new();
    let mut outcome = PipelineOutcome {
        log_url: url.to_string(),
        outputs: Value::Object(Map::new()),
        ..Default::default()
    };

    for layer in layers {
        let input = crate::api::proxy_auth_layer::LayerInput {
            method,
            url,
            headers,
            body: body_view,
            prior_layer_outputs: &prior,
        };
        let mutation = layer.apply(&input).await?;

        outcome.headers.extend(mutation.add_headers.into_iter());
        outcome.query.extend(mutation.add_query.into_iter());
        if mutation.replace_body.is_some() {
            // Last layer to set wins; unusual but explicit.
            outcome.replace_body = mutation.replace_body;
        }
        // Layers that fold credentials into the URL publish a redacted form
        // under `outputs.log_url_replacement`. Last writer wins.
        if let Some(redacted) = mutation
            .outputs
            .get("log_url_replacement")
            .and_then(|v| v.as_str())
        {
            outcome.log_url = redacted.to_string();
        }

        let ns = layer.output_namespace().to_string();
        prior.insert(ns.clone(), mutation.outputs.clone());
        outcome.cache_hits.push(mutation.cache_was_hit);

        if let Value::Object(ref mut m) = outcome.outputs {
            m.insert(ns, mutation.outputs);
        }
    }

    Ok(outcome)
}

// ---- 401 retry decision ----------------------------------------------
//
// Replaces the legacy `proxy.rs::should_retry_after_401` (single-variant,
// `script_handshake`-only) with a pipeline-aware version that triggers iff
// some layer in the run consumed a cached value AND opted into retry.

/// True iff the upstream returned 401 AND at least one layer in the
/// pipeline (a) said its `apply` was a cache hit AND (b) opted in to
/// `RetryHint::InvalidateAndRetry`. The pipeline runner then invalidates
/// those layers' caches via `pipeline_invalidate_for_retry` and re-runs
/// the pipeline once.
pub fn pipeline_should_retry(
    layers: &[Arc<dyn AuthLayer>],
    outcome: &PipelineOutcome,
    status: StatusCode,
) -> bool {
    if status != StatusCode::UNAUTHORIZED {
        return false;
    }
    layers
        .iter()
        .zip(&outcome.cache_hits)
        .any(|(layer, &hit)| hit && layer.retry_on_401() == RetryHint::InvalidateAndRetry)
}

/// Invalidate every layer that opted in AND was a cache hit. Called before
/// the second pipeline run.
pub async fn pipeline_invalidate_for_retry(
    layers: &[Arc<dyn AuthLayer>],
    outcome: &PipelineOutcome,
) {
    for (layer, &hit) in layers.iter().zip(&outcome.cache_hits) {
        if hit && layer.retry_on_401() == RetryHint::InvalidateAndRetry {
            layer.invalidate_cache().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::proxy_auth_layer::*;
    use crate::api::proxy_hex::hex_lower;
    use axum::http::HeaderName;
    use bytes::Bytes;
    use std::sync::Arc;

    /// Test layer that adds a header and publishes one output value.
    struct StaticHeaderLayer {
        name: &'static str,
        header: (HeaderName, String),
        output: serde_json::Value,
    }

    #[async_trait::async_trait]
    impl AuthLayer for StaticHeaderLayer {
        fn output_namespace(&self) -> &str {
            self.name
        }
        async fn apply(
            &self,
            _input: &LayerInput<'_>,
        ) -> Result<AuthMutation, (axum::http::StatusCode, String)> {
            Ok(AuthMutation {
                add_headers: vec![self.header.clone()],
                outputs: self.output.clone(),
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn pipeline_threads_outputs_in_order() {
        let layers: Vec<Arc<dyn AuthLayer>> = vec![
            Arc::new(StaticHeaderLayer {
                name: "first",
                header: (HeaderName::from_static("x-first"), "1".into()),
                output: serde_json::json!({"token": "abc"}),
            }),
            Arc::new(StaticHeaderLayer {
                name: "second",
                header: (HeaderName::from_static("x-second"), "2".into()),
                output: serde_json::json!({"sig": "xyz"}),
            }),
        ];
        let body = Bytes::new();
        let result = run_pipeline(
            &layers,
            &axum::http::Method::GET,
            "https://example.com/foo",
            &[],
            &body,
        )
        .await
        .unwrap();
        assert_eq!(result.headers.len(), 2);
        assert_eq!(result.outputs["first"]["token"], "abc");
        assert_eq!(result.outputs["second"]["sig"], "xyz");
        assert_eq!(result.log_url, "https://example.com/foo");
    }

    #[tokio::test]
    async fn pipeline_returns_layer_error() {
        struct FailingLayer;
        #[async_trait::async_trait]
        impl AuthLayer for FailingLayer {
            fn output_namespace(&self) -> &str {
                "fail"
            }
            async fn apply(
                &self,
                _input: &LayerInput<'_>,
            ) -> Result<AuthMutation, (axum::http::StatusCode, String)> {
                Err((axum::http::StatusCode::BAD_GATEWAY, "boom".into()))
            }
        }
        let layers: Vec<Arc<dyn AuthLayer>> = vec![Arc::new(FailingLayer)];
        let body = Bytes::new();
        let err = run_pipeline(
            &layers,
            &axum::http::Method::GET,
            "https://example.com",
            &[],
            &body,
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_GATEWAY);
    }

    // ---- Retry truth table -------------------------------------------

    /// Layer with configurable retry hint + cache-hit behavior. Used to
    /// drive the pipeline_should_retry / pipeline_invalidate_for_retry
    /// truth tables.
    struct ConfigurableRetryLayer {
        name: &'static str,
        retry_hint: RetryHint,
        cache_was_hit: bool,
        invalidated: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl AuthLayer for ConfigurableRetryLayer {
        fn output_namespace(&self) -> &str {
            self.name
        }
        fn retry_on_401(&self) -> RetryHint {
            self.retry_hint
        }
        async fn invalidate_cache(&self) {
            self.invalidated
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        async fn apply(
            &self,
            _input: &LayerInput<'_>,
        ) -> Result<AuthMutation, (axum::http::StatusCode, String)> {
            Ok(AuthMutation {
                cache_was_hit: self.cache_was_hit,
                ..Default::default()
            })
        }
    }

    fn outcome_with_hits(hits: Vec<bool>) -> PipelineOutcome {
        PipelineOutcome {
            cache_hits: hits,
            ..Default::default()
        }
    }

    #[test]
    fn retry_skipped_when_status_not_401() {
        let layer: Arc<dyn AuthLayer> = Arc::new(ConfigurableRetryLayer {
            name: "x",
            retry_hint: RetryHint::InvalidateAndRetry,
            cache_was_hit: true,
            invalidated: std::sync::atomic::AtomicBool::new(false),
        });
        let outcome = outcome_with_hits(vec![true]);
        assert!(!pipeline_should_retry(
            &[layer],
            &outcome,
            StatusCode::INTERNAL_SERVER_ERROR
        ));
    }

    #[test]
    fn retry_skipped_when_layer_did_not_opt_in() {
        let layer: Arc<dyn AuthLayer> = Arc::new(ConfigurableRetryLayer {
            name: "x",
            retry_hint: RetryHint::Never,
            cache_was_hit: true,
            invalidated: std::sync::atomic::AtomicBool::new(false),
        });
        let outcome = outcome_with_hits(vec![true]);
        assert!(!pipeline_should_retry(
            &[layer],
            &outcome,
            StatusCode::UNAUTHORIZED
        ));
    }

    #[test]
    fn retry_skipped_when_no_cache_hit() {
        // Cache miss → 401 is a real auth failure, not staleness; no point
        // retrying with the same fresh token.
        let layer: Arc<dyn AuthLayer> = Arc::new(ConfigurableRetryLayer {
            name: "x",
            retry_hint: RetryHint::InvalidateAndRetry,
            cache_was_hit: false,
            invalidated: std::sync::atomic::AtomicBool::new(false),
        });
        let outcome = outcome_with_hits(vec![false]);
        assert!(!pipeline_should_retry(
            &[layer],
            &outcome,
            StatusCode::UNAUTHORIZED
        ));
    }

    #[test]
    fn retry_triggered_when_cached_layer_opted_in() {
        let layer: Arc<dyn AuthLayer> = Arc::new(ConfigurableRetryLayer {
            name: "x",
            retry_hint: RetryHint::InvalidateAndRetry,
            cache_was_hit: true,
            invalidated: std::sync::atomic::AtomicBool::new(false),
        });
        let outcome = outcome_with_hits(vec![true]);
        assert!(pipeline_should_retry(
            &[layer],
            &outcome,
            StatusCode::UNAUTHORIZED
        ));
    }

    #[test]
    fn retry_triggered_when_any_layer_satisfies_the_predicate() {
        // First layer: opt-in but cache miss. Second: opt-in and cache hit.
        // Either one being a hit-and-opt-in is enough.
        let l1: Arc<dyn AuthLayer> = Arc::new(ConfigurableRetryLayer {
            name: "a",
            retry_hint: RetryHint::InvalidateAndRetry,
            cache_was_hit: false,
            invalidated: std::sync::atomic::AtomicBool::new(false),
        });
        let l2: Arc<dyn AuthLayer> = Arc::new(ConfigurableRetryLayer {
            name: "b",
            retry_hint: RetryHint::InvalidateAndRetry,
            cache_was_hit: true,
            invalidated: std::sync::atomic::AtomicBool::new(false),
        });
        let outcome = outcome_with_hits(vec![false, true]);
        assert!(pipeline_should_retry(
            &[l1, l2],
            &outcome,
            StatusCode::UNAUTHORIZED
        ));
    }

    #[tokio::test]
    async fn invalidate_for_retry_invalidates_only_opted_in_cache_hits() {
        let l1 = Arc::new(ConfigurableRetryLayer {
            name: "stale-cache",
            retry_hint: RetryHint::InvalidateAndRetry,
            cache_was_hit: true,
            invalidated: std::sync::atomic::AtomicBool::new(false),
        });
        let l2 = Arc::new(ConfigurableRetryLayer {
            name: "stateless",
            retry_hint: RetryHint::Never,
            cache_was_hit: true,
            invalidated: std::sync::atomic::AtomicBool::new(false),
        });
        let l3 = Arc::new(ConfigurableRetryLayer {
            name: "fresh",
            retry_hint: RetryHint::InvalidateAndRetry,
            cache_was_hit: false,
            invalidated: std::sync::atomic::AtomicBool::new(false),
        });
        let layers: Vec<Arc<dyn AuthLayer>> = vec![l1.clone(), l2.clone(), l3.clone()];
        let outcome = outcome_with_hits(vec![true, true, false]);
        pipeline_invalidate_for_retry(&layers, &outcome).await;
        assert!(l1.invalidated.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!l2.invalidated.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!l3.invalidated.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// Layer that captures the body view it received so the test can
    /// inspect whether the pipeline handed it raw bytes or a hash.
    struct BodyCapturingLayer {
        captured: std::sync::Mutex<Option<BodyViewSnapshot>>,
    }

    #[derive(Debug, Clone)]
    enum BodyViewSnapshot {
        Raw(usize),
        HashOnly { sha256_hex: String, length: u64 },
    }

    #[async_trait::async_trait]
    impl AuthLayer for BodyCapturingLayer {
        fn output_namespace(&self) -> &str {
            "capture"
        }
        async fn apply(
            &self,
            input: &LayerInput<'_>,
        ) -> Result<AuthMutation, (axum::http::StatusCode, String)> {
            let snapshot = match input.body {
                BodyView::Raw(b) => BodyViewSnapshot::Raw(b.len()),
                BodyView::HashOnly { sha256, length } => BodyViewSnapshot::HashOnly {
                    sha256_hex: hex_lower(sha256),
                    length,
                },
            };
            *self.captured.lock().unwrap() = Some(snapshot);
            Ok(AuthMutation::default())
        }
    }

    #[tokio::test]
    async fn pipeline_uses_raw_body_for_small_requests() {
        let layer = Arc::new(BodyCapturingLayer {
            captured: std::sync::Mutex::new(None),
        });
        let layers: Vec<Arc<dyn AuthLayer>> = vec![layer.clone()];
        let body = bytes::Bytes::from(b"small body".to_vec());
        run_pipeline(&layers, &axum::http::Method::POST, "https://x", &[], &body)
            .await
            .unwrap();
        let captured = layer.captured.lock().unwrap();
        match captured.as_ref().unwrap() {
            BodyViewSnapshot::Raw(len) => assert_eq!(*len, 10),
            other => panic!("expected raw view, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pipeline_switches_to_hash_only_view_above_threshold() {
        let layer = Arc::new(BodyCapturingLayer {
            captured: std::sync::Mutex::new(None),
        });
        let layers: Vec<Arc<dyn AuthLayer>> = vec![layer.clone()];
        // 1MB + 1 byte → exceeds RAW_BODY_THRESHOLD_BYTES.
        let body_size = RAW_BODY_THRESHOLD_BYTES + 1;
        let body = bytes::Bytes::from(vec![0xABu8; body_size]);
        let expected_hash = body_hash(&body);
        let expected_hex = hex_lower(&expected_hash);
        run_pipeline(&layers, &axum::http::Method::POST, "https://x", &[], &body)
            .await
            .unwrap();
        let captured = layer.captured.lock().unwrap();
        match captured.as_ref().unwrap() {
            BodyViewSnapshot::HashOnly { sha256_hex, length } => {
                assert_eq!(*length, body_size as u64);
                assert_eq!(sha256_hex, &expected_hex);
            }
            other => panic!("expected hash view, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pipeline_picks_up_log_url_replacement() {
        struct RedactingLayer;
        #[async_trait::async_trait]
        impl AuthLayer for RedactingLayer {
            fn output_namespace(&self) -> &str {
                "redact"
            }
            async fn apply(
                &self,
                _input: &LayerInput<'_>,
            ) -> Result<AuthMutation, (axum::http::StatusCode, String)> {
                Ok(AuthMutation {
                    outputs: serde_json::json!({
                        "log_url_replacement": "https://example.com/foo?key=REDACTED",
                    }),
                    ..Default::default()
                })
            }
        }
        let layers: Vec<Arc<dyn AuthLayer>> = vec![Arc::new(RedactingLayer)];
        let body = Bytes::new();
        let result = run_pipeline(
            &layers,
            &axum::http::Method::GET,
            "https://example.com/foo?key=secret",
            &[],
            &body,
        )
        .await
        .unwrap();
        assert_eq!(result.log_url, "https://example.com/foo?key=REDACTED");
    }
}
