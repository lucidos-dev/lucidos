//! `HmacSignedLayer` — HMAC-signed-query-string auth as an `AuthLayer`
//! impl. Stateless (`retry_on_401 = Never`, no cache); on `apply` it looks
//! up the key + secret credentials, signs the request's query string, and
//! emits one header (key) plus two query params (timestamp + signature).

use crate::api::proxy::{compute_hmac_hex, lookup_credential_value, HmacAlgorithm};
use crate::api::proxy_auth_layer::{AuthLayer, AuthMutation, LayerInput};
use async_trait::async_trait;
use axum::http::{HeaderName, StatusCode};
use sqlx::PgPool;

pub struct HmacSignedLayer {
    namespace: String,
    key_credential: String,
    secret_credential: String,
    key_header: String,
    algorithm: HmacAlgorithm,
    signature_param: String,
    timestamp_param: Option<String>,
    pool: PgPool,
}

impl HmacSignedLayer {
    // Plain field-by-field constructor; a builder would mirror the struct shape
    // without simplifying anything at the proxy-pipeline construction site.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        namespace: String,
        key_credential: String,
        secret_credential: String,
        key_header: String,
        algorithm: HmacAlgorithm,
        signature_param: String,
        timestamp_param: Option<String>,
        pool: PgPool,
    ) -> Self {
        Self {
            namespace,
            key_credential,
            secret_credential,
            key_header,
            algorithm,
            signature_param,
            timestamp_param,
            pool,
        }
    }

    /// Sign a query string using the held secret. Pulled out so tests can
    /// drive deterministic timestamps without going through the credential
    /// store.
    fn sign_with(&self, query: &str, secret: &[u8], now_ms: u64) -> (Option<u64>, String) {
        let mut to_sign = query.to_string();
        let ts = self.timestamp_param.as_deref().map(|_| now_ms);
        if let (Some(ts_param), Some(ts_val)) = (self.timestamp_param.as_deref(), ts) {
            if !to_sign.is_empty() {
                to_sign.push('&');
            }
            to_sign.push_str(&format!("{}={}", ts_param, ts_val));
        }
        let sig = compute_hmac_hex(self.algorithm, secret, to_sign.as_bytes());
        (ts, sig)
    }
}

#[async_trait]
impl AuthLayer for HmacSignedLayer {
    fn output_namespace(&self) -> &str {
        &self.namespace
    }

    async fn apply(
        &self,
        input: &LayerInput<'_>,
    ) -> Result<AuthMutation, (StatusCode, String)> {
        let key = lookup_credential_value(&self.pool, &self.key_credential).await?;
        let secret = lookup_credential_value(&self.pool, &self.secret_credential).await?;

        // Existing query string from the request URL (without the leading `?`).
        let existing_query = input
            .url
            .split_once('?')
            .map(|(_, q)| q)
            .unwrap_or("");
        let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
        let (ts_opt, sig) = self.sign_with(existing_query, secret.as_bytes(), now_ms);

        let header_name = HeaderName::from_bytes(self.key_header.as_bytes()).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("invalid key_header '{}': {}", self.key_header, e),
            )
        })?;

        let mut add_query = Vec::new();
        if let (Some(ts_param), Some(ts_val)) = (self.timestamp_param.as_deref(), ts_opt) {
            add_query.push((ts_param.to_string(), ts_val.to_string()));
        }
        add_query.push((self.signature_param.clone(), sig));

        Ok(AuthMutation {
            add_headers: vec![(header_name, key)],
            add_query,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::proxy::compute_hmac_hex;

    fn lazy_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/nobody")
            .unwrap()
    }

    #[tokio::test]
    async fn sign_with_appends_timestamp_then_computes_sig_over_combined_query() {
        let layer = HmacSignedLayer::new(
            "ns".into(),
            "key".into(),
            "secret".into(),
            "X-Mbx-Apikey".into(),
            HmacAlgorithm::Sha256,
            "signature".into(),
            Some("timestamp".into()),
            lazy_pool(),
        );
        let secret = b"my-test-secret";
        let (ts, sig) = layer.sign_with("symbol=BTCUSDT&recvWindow=5000", secret, 1_700_000_000_000);
        assert_eq!(ts, Some(1_700_000_000_000));
        // The signed payload includes the timestamp.
        let expected = compute_hmac_hex(
            HmacAlgorithm::Sha256,
            secret,
            b"symbol=BTCUSDT&recvWindow=5000&timestamp=1700000000000",
        );
        assert_eq!(sig, expected);
    }

    #[tokio::test]
    async fn sign_with_empty_query_and_no_timestamp_signs_empty_string() {
        let layer = HmacSignedLayer::new(
            "ns".into(),
            "key".into(),
            "secret".into(),
            "X-Mbx-Apikey".into(),
            HmacAlgorithm::Sha256,
            "signature".into(),
            None,
            lazy_pool(),
        );
        let (ts, sig) = layer.sign_with("", b"s", 0);
        assert_eq!(ts, None);
        assert_eq!(sig, compute_hmac_hex(HmacAlgorithm::Sha256, b"s", b""));
    }

    #[tokio::test]
    async fn output_namespace_returns_configured_namespace() {
        let layer = HmacSignedLayer::new(
            "binance".into(),
            "key".into(),
            "secret".into(),
            "X-Mbx-Apikey".into(),
            HmacAlgorithm::Sha256,
            "signature".into(),
            Some("timestamp".into()),
            lazy_pool(),
        );
        assert_eq!(layer.output_namespace(), "binance");
    }
}
