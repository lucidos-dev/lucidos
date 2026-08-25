//! `ScriptHandshakeLayer` — runs a per-provider login script (or reuses
//! a cached result) and attaches the resulting headers to subsequent
//! requests. Owns the singleflight gate that keeps concurrent first-time
//! requests for the same proxy from all spawning the script.

use crate::api::proxy::fetch_required_credential;
use crate::api::proxy_auth_layer::{AuthLayer, AuthMutation, LayerInput, RetryHint};
use crate::api::proxy_token_cache::ProxyTokenCache;
use crate::core::oauth;
use async_trait::async_trait;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use serde_json::json;
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Floor on the script-returned TTL. Guards against a buggy script that
/// returns `expires_in: 1` and DOSes itself with per-request handshakes.
const MIN_HANDSHAKE_TTL_SECS: u64 = 60;

/// Looks up + auto-refreshes the OAuth accounts that a script_handshake
/// layer wants injected as `OAUTH_<P>_ACCESS_TOKEN` env vars. Trait so
/// the layer's tests can stub it without a Postgres pool.
#[async_trait]
pub trait OAuthLookup: Send + Sync {
    /// Return the account for each requested provider (auto-refreshed if
    /// expired). Errors mapped to layer-friendly `(StatusCode, message)`.
    /// A missing provider must surface as `502 BAD_GATEWAY` with a message
    /// that names the provider, so the operator knows which
    /// `connect_oauth_account` to run.
    async fn fetch_for_providers(
        &self,
        providers: &[String],
    ) -> Result<Vec<oauth::OAuthAccount>, (StatusCode, String)>;
}

/// Default `OAuthLookup` impl used in production: pulls from the
/// `oauth_accounts` table and calls `oauth::refresh_oauth_if_needed` on
/// each row.
pub struct DbOAuthLookup {
    pool: PgPool,
}

impl DbOAuthLookup {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OAuthLookup for DbOAuthLookup {
    async fn fetch_for_providers(
        &self,
        providers: &[String],
    ) -> Result<Vec<oauth::OAuthAccount>, (StatusCode, String)> {
        use crate::core::oauth::AccountLookupError;
        let mut out = Vec::with_capacity(providers.len());
        for provider in providers {
            let account = oauth::get_account_with_fresh_token(&self.pool, provider)
                .await
                .map_err(|e| match e {
                    AccountLookupError::NotConnected => (
                        StatusCode::BAD_GATEWAY,
                        oauth::provider_not_connected_msg(provider),
                    ),
                    AccountLookupError::DbError(err) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!(
                            "failed to load OAuth account for provider '{}': {}",
                            provider, err
                        ),
                    ),
                    AccountLookupError::RefreshFailed(err) => (
                        StatusCode::BAD_GATEWAY,
                        format!(
                            "OAuth token refresh failed for provider '{}': {}",
                            provider, err
                        ),
                    ),
                })?;
            out.push(account);
        }
        Ok(out)
    }
}

pub struct ScriptHandshakeLayer {
    namespace: String,
    proxy_name: String,
    /// Optional: the credential whose `CRED_<NAME>*` env vars are injected
    /// before the script runs. `None` = inject no credential env vars (the
    /// script obtains its secret by other means).
    credential: Option<String>,
    script_rel_path: String,
    oauth_providers: Vec<String>,
    pool: PgPool,
    workspace_path: Arc<PathBuf>,
    token_cache: Arc<ProxyTokenCache>,
    oauth_lookup: Arc<dyn OAuthLookup>,
}

impl ScriptHandshakeLayer {
    // Plain constructor that stores each parameter into the matching struct
    // field — wrapping in a builder would only push the parameter list up one
    // level without simplifying the layer-construction site.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        namespace: String,
        proxy_name: String,
        credential: Option<String>,
        script_rel_path: String,
        oauth_providers: Vec<String>,
        pool: PgPool,
        workspace_path: Arc<PathBuf>,
        token_cache: Arc<ProxyTokenCache>,
        oauth_lookup: Arc<dyn OAuthLookup>,
    ) -> Self {
        Self {
            namespace,
            proxy_name,
            credential,
            script_rel_path,
            oauth_providers,
            pool,
            workspace_path,
            token_cache,
            oauth_lookup,
        }
    }

    /// Resolve the headers for this request: cache hit → reuse; miss →
    /// singleflight the script run, parse, cache, return. Bool is whether
    /// the headers came from a pre-existing cache entry (so a 401 from
    /// upstream is worth a retry-after-cache-invalidation).
    async fn resolve_headers(
        &self,
    ) -> Result<(Vec<(HeaderName, HeaderValue)>, bool), (StatusCode, String)> {
        let (token, was_hit) = self
            .token_cache
            .get_or_refresh(&self.proxy_name, || self.run_script())
            .await?;
        Ok((token.headers, was_hit))
    }

    /// Actually run the script: pull credential + OAuth tokens, build env
    /// vars, call the runner, map errors to (StatusCode, message). OAuth
    /// lookup runs first so a missing provider fails fast with a clear
    /// 502 before the credential store is touched.
    async fn run_script(
        &self,
    ) -> Result<(Vec<(HeaderName, HeaderValue)>, Duration), (StatusCode, String)> {
        use crate::api::proxy_script_runner::{run_handshake_script, RunError};

        let oauth_accounts = if self.oauth_providers.is_empty() {
            Vec::new()
        } else {
            self.oauth_lookup
                .fetch_for_providers(&self.oauth_providers)
                .await?
        };

        // Inject `CRED_<NAME>*` only when a credential is configured. When
        // it's absent the script sources its secret elsewhere (OS keychain,
        // OAuth-only exchange) — run it with no credential env vars from this
        // layer, and don't error on the missing credential.
        let mut env_vars = match &self.credential {
            Some(name) => {
                let cred = fetch_required_credential(&self.pool, name).await?;
                // `_for`, not the list version: the user named THIS credential
                // on this layer, so an `oauth_client` must still inject. The
                // list version's skip is about the blanket every-secret fan-out
                // into every subprocess, which this is not.
                crate::core::credentials::credential_env_vars_for(cred)
            }
            None => Vec::new(),
        };
        env_vars.extend(oauth::account_env_vars(oauth_accounts));

        match run_handshake_script(&self.workspace_path, &self.script_rel_path, env_vars).await {
            Ok(out) => {
                let ttl = Duration::from_secs(out.expires_in.max(MIN_HANDSHAKE_TTL_SECS));
                Ok((out.headers, ttl))
            }
            // Both name a broken `apis.json`, not a broken upstream, so they
            // answer 500 rather than the 502 every other arm gets.
            Err(e @ (RunError::NotFound(_) | RunError::PathRejected(_))) => {
                Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
            }
            Err(e) => Err((StatusCode::BAD_GATEWAY, e.to_string())),
        }
    }
}

#[async_trait]
impl AuthLayer for ScriptHandshakeLayer {
    fn output_namespace(&self) -> &str {
        &self.namespace
    }

    fn retry_on_401(&self) -> RetryHint {
        RetryHint::InvalidateAndRetry
    }

    async fn invalidate_cache(&self) {
        self.token_cache.invalidate(&self.proxy_name).await;
    }

    async fn apply(&self, _input: &LayerInput<'_>) -> Result<AuthMutation, (StatusCode, String)> {
        let (headers, was_hit) = self.resolve_headers().await?;

        // Convert to (name, String) once. A non-ASCII HeaderValue can't
        // be forwarded as a UTF-8 string; surface the error instead of
        // silently sending an empty header that would cause an opaque
        // upstream 401.
        let mut add_headers: Vec<(HeaderName, String)> = Vec::with_capacity(headers.len());
        for (name, value) in headers {
            let s = value.to_str().map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "proxy '{}' script_handshake returned non-ASCII header '{}': {}",
                        self.proxy_name,
                        name.as_str(),
                        e
                    ),
                )
            })?;
            add_headers.push((name, s.to_string()));
        }

        // Outputs published for downstream layers (e.g. a WasmSigner that
        // reads `prior["script_handshake"]["headers"]["x-cfc-auth-token"]`).
        let header_obj: serde_json::Map<String, serde_json::Value> = add_headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    serde_json::Value::String(value.clone()),
                )
            })
            .collect();

        Ok(AuthMutation {
            add_headers,
            cache_was_hit: was_hit,
            outputs: json!({ "headers": serde_json::Value::Object(header_obj) }),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::proxy_auth_layer::*;
    use axum::http::{HeaderName, HeaderValue, Method};
    use bytes::Bytes;
    use sqlx::postgres::PgPoolOptions;
    use std::collections::HashMap;
    use std::time::Duration;

    fn lazy_pool() -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/nobody")
            .expect("connect_lazy never errors on parse-only failure")
    }

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

    /// Errors with the production "provider not connected" message via
    /// the shared `oauth::provider_not_connected_msg` helper, so layer
    /// tests assert against the same string the user would see.
    struct MissingProviderLookup;

    #[async_trait]
    impl OAuthLookup for MissingProviderLookup {
        async fn fetch_for_providers(
            &self,
            providers: &[String],
        ) -> Result<Vec<oauth::OAuthAccount>, (StatusCode, String)> {
            let provider = providers.first().cloned().unwrap_or_default();
            Err((
                StatusCode::BAD_GATEWAY,
                oauth::provider_not_connected_msg(&provider),
            ))
        }
    }

    /// Panics on invocation — proves the layer skips OAuth lookup when
    /// `oauth_providers` is empty.
    struct PanicLookup;

    #[async_trait]
    impl OAuthLookup for PanicLookup {
        async fn fetch_for_providers(
            &self,
            _providers: &[String],
        ) -> Result<Vec<oauth::OAuthAccount>, (StatusCode, String)> {
            panic!("OAuth lookup must not be invoked when oauth_providers is empty");
        }
    }

    fn layer_with(
        namespace: &str,
        oauth_providers: Vec<String>,
        cache: Arc<ProxyTokenCache>,
        oauth_lookup: Arc<dyn OAuthLookup>,
    ) -> ScriptHandshakeLayer {
        ScriptHandshakeLayer::new(
            namespace.into(),
            "proxy".into(),
            Some("cred".into()),
            "scripts/x.sh".into(),
            oauth_providers,
            lazy_pool(),
            Arc::new(PathBuf::from("/ws")),
            cache,
            oauth_lookup,
        )
    }

    fn layer_no_oauth(cache: Arc<ProxyTokenCache>) -> ScriptHandshakeLayer {
        layer_with("script_handshake", Vec::new(), cache, Arc::new(PanicLookup))
    }

    #[tokio::test]
    async fn output_namespace_returns_configured_namespace() {
        let layer = layer_with(
            "ns",
            Vec::new(),
            Arc::new(ProxyTokenCache::new()),
            Arc::new(PanicLookup),
        );
        assert_eq!(layer.output_namespace(), "ns");
    }

    #[tokio::test]
    async fn retry_on_401_is_invalidate_and_retry() {
        let layer = layer_no_oauth(Arc::new(ProxyTokenCache::new()));
        assert_eq!(layer.retry_on_401(), RetryHint::InvalidateAndRetry);
    }

    #[tokio::test]
    async fn cache_hit_sets_cache_was_hit_and_returns_cached_headers() {
        let cache = Arc::new(ProxyTokenCache::new());
        cache
            .insert(
                "proxy",
                vec![(
                    HeaderName::from_static("authorization"),
                    HeaderValue::from_static("Bearer cached-token"),
                )],
                Duration::from_secs(60),
            )
            .await;

        let layer = layer_no_oauth(cache.clone());

        let body = Bytes::new();
        let prior = HashMap::new();
        let input = input_for(&body, "https://upstream.example/x", &prior);
        let m = layer.apply(&input).await.unwrap();

        assert!(
            m.cache_was_hit,
            "cache hit should set the AuthMutation flag"
        );
        assert_eq!(m.add_headers.len(), 1);
        assert_eq!(m.add_headers[0].0.as_str(), "authorization");
        assert_eq!(m.add_headers[0].1, "Bearer cached-token");

        // Outputs expose the produced headers (as an object — downstream
        // layers look up `prior["script_handshake"]["headers"]["x-foo"]`).
        let headers = m.outputs["headers"].as_object().unwrap();
        assert_eq!(
            headers.get("authorization").and_then(|v| v.as_str()),
            Some("Bearer cached-token")
        );
    }

    #[tokio::test]
    async fn rejects_non_ascii_header_value_with_502() {
        // A handshake script that returns a header with non-ASCII bytes
        // would silently get forwarded as an empty Authorization header
        // (and the upstream would 401 with no diagnostic). Surface as 502
        // instead so the operator sees an actionable error.
        let cache = Arc::new(ProxyTokenCache::new());
        let raw = HeaderValue::from_bytes(b"\xff non-ascii").unwrap();
        cache
            .insert(
                "proxy",
                vec![(HeaderName::from_static("authorization"), raw)],
                Duration::from_secs(60),
            )
            .await;
        let layer = layer_no_oauth(cache);
        let body = Bytes::new();
        let prior = HashMap::new();
        let input = input_for(&body, "https://x", &prior);
        match layer.apply(&input).await {
            Err((StatusCode::BAD_GATEWAY, msg)) => {
                assert!(msg.contains("non-ASCII"), "msg was: {msg}");
            }
            other => panic!("expected 502 BAD_GATEWAY, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invalidate_cache_removes_cached_entry() {
        let cache = Arc::new(ProxyTokenCache::new());
        cache
            .insert(
                "proxy",
                vec![(
                    HeaderName::from_static("authorization"),
                    HeaderValue::from_static("Bearer cached-token"),
                )],
                Duration::from_secs(60),
            )
            .await;

        let layer = layer_no_oauth(cache.clone());

        assert!(cache.get("proxy").await.is_some(), "precondition: cached");
        layer.invalidate_cache().await;
        assert!(cache.get("proxy").await.is_none(), "should be invalidated");
    }

    #[tokio::test]
    async fn run_script_accepts_all_credential_types_and_injects_correct_env_vars() {
        use crate::core::credentials::AuthType;
        use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};

        const ECHO_SCRIPT: &str = r#"
import os, json
print(json.dumps({
    "headers": {
        "x-cred-bare":     os.environ.get("CRED_TESTSVC", ""),
        "x-cred-username": os.environ.get("CRED_TESTSVC_USERNAME", ""),
        "x-cred-password": os.environ.get("CRED_TESTSVC_PASSWORD", ""),
    },
    "expires_in": 60,
}))
"#;

        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("scripts/auth")).unwrap();
        std::fs::write(tmp.path().join("scripts/auth/echo.py"), ECHO_SCRIPT).unwrap();

        let (pool, db_name) = setup_test_db().await;

        // (auth_type, auth_value, expected env-vars seen by script)
        type AuthCase = (
            AuthType,
            &'static str,
            &'static [(&'static str, &'static str)],
        );
        let cases: &[AuthCase] = &[
            (AuthType::ApiKey, "ak-123", &[("x-cred-bare", "ak-123")]),
            (AuthType::Bearer, "bear-456", &[("x-cred-bare", "bear-456")]),
            (
                AuthType::Basic,
                "alice:s3cret",
                &[("x-cred-bare", "alice:s3cret")],
            ),
            (
                AuthType::Password,
                r#"{"username":"alice","password":"s3cret"}"#,
                &[("x-cred-username", "alice"), ("x-cred-password", "s3cret")],
            ),
        ];

        for (auth_type, auth_value, expected_headers) in cases.iter() {
            seed_credential(
                &pool,
                "testsvc",
                "https://example.test",
                *auth_type,
                auth_value,
            )
            .await;

            // Fresh ProxyTokenCache + layer per iteration so the previous
            // case's headers can't satisfy this one's request.
            let layer = ScriptHandshakeLayer::new(
                "script_handshake".into(),
                "proxy".into(),
                Some("testsvc".into()),
                "scripts/auth/echo.py".into(),
                Vec::new(),
                pool.clone(),
                Arc::new(tmp.path().to_path_buf()),
                Arc::new(ProxyTokenCache::new()),
                Arc::new(PanicLookup),
            );

            let body = Bytes::new();
            let prior = HashMap::new();
            let input = input_for(&body, "https://example.test/x", &prior);
            let mutation = layer.apply(&input).await.unwrap_or_else(|(code, msg)| {
                panic!(
                    "{} credential should be accepted by script_handshake; got {} {}",
                    auth_type, code, msg
                )
            });

            let headers: HashMap<&str, &str> = mutation
                .add_headers
                .iter()
                .map(|(n, v)| (n.as_str(), v.as_str()))
                .collect();
            for (name, want) in *expected_headers {
                assert_eq!(
                    headers.get(name).copied(),
                    Some(*want),
                    "{} credential: header {name} should be {want:?}, got {:?}",
                    auth_type,
                    headers.get(name)
                );
            }
            let leaked: Vec<&str> = headers
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, _)| *k)
                .filter(|k| expected_headers.iter().all(|(want_k, _)| want_k != k))
                .collect();
            assert!(
                leaked.is_empty(),
                "{} credential leaked unexpected env vars into script: {:?}",
                auth_type,
                leaked
            );
        }

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn credential_none_runs_script_without_injecting_cred_env_vars() {
        // A script_handshake layer with no configured credential must run the
        // script (returning its headers) and inject NO `CRED_*` env vars — the
        // script sources its secret elsewhere. It must not error on the absent
        // credential, and must not touch the credential store (so a lazy pool
        // pointed at an unreachable DB is fine — it's never queried).
        //
        // The script lives under `data/scripts/auth/` while the config value is
        // `scripts/auth/echo.py`, so this also exercises the `data/`-relative
        // path resolution end-to-end through the layer.
        const ECHO_SCRIPT: &str = r#"
import os, json
print(json.dumps({
    "headers": {
        "x-auth":      "ok",
        "x-cred-seen": os.environ.get("CRED_TESTSVC", "absent"),
    },
    "expires_in": 60,
}))
"#;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("data/scripts/auth")).unwrap();
        std::fs::write(tmp.path().join("data/scripts/auth/echo.py"), ECHO_SCRIPT).unwrap();

        let layer = ScriptHandshakeLayer::new(
            "script_handshake".into(),
            "proxy".into(),
            None,
            "scripts/auth/echo.py".into(),
            Vec::new(),
            lazy_pool(),
            Arc::new(tmp.path().to_path_buf()),
            Arc::new(ProxyTokenCache::new()),
            Arc::new(PanicLookup),
        );

        let body = Bytes::new();
        let prior = HashMap::new();
        let input = input_for(&body, "https://example.test/x", &prior);
        let mutation = layer
            .apply(&input)
            .await
            .unwrap_or_else(|(code, msg)| panic!("credential-less handshake failed: {code} {msg}"));

        let headers: HashMap<&str, &str> = mutation
            .add_headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            headers.get("x-auth").copied(),
            Some("ok"),
            "script should still run and return its headers"
        );
        assert_eq!(
            headers.get("x-cred-seen").copied(),
            Some("absent"),
            "no CRED_* env var should be injected when credential is None"
        );
    }

    #[tokio::test]
    async fn missing_oauth_provider_returns_502_with_provider_name() {
        let layer = layer_with(
            "script_handshake",
            vec!["google".into()],
            Arc::new(ProxyTokenCache::new()),
            Arc::new(MissingProviderLookup),
        );
        let body = Bytes::new();
        let prior = HashMap::new();
        let input = input_for(&body, "https://x", &prior);
        match layer.apply(&input).await {
            Err((StatusCode::BAD_GATEWAY, msg)) => {
                assert!(
                    msg.contains("google"),
                    "missing-provider message must name the provider; got: {msg}"
                );
                assert!(
                    msg.contains("connect_oauth_account"),
                    "missing-provider message must hint at the recovery action; got: {msg}"
                );
            }
            other => panic!("expected 502 BAD_GATEWAY, got {other:?}"),
        }
    }
}
