//! `proxy_request` LLM tool — call a backend configured in
//! `data/config/apis.json` through the engine proxy. The credential value
//! never reaches the model; only the configured proxy *name* does.

use super::super::LucidosEngine;
use crate::api::proxy as api_proxy;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method};

/// If the proxy is configured with `credential_bundle` auth, return the
/// error string the LLM should see (asking it to switch to the script-side
/// CLI). Otherwise `None`.
pub(crate) fn refuse_credential_bundle_for_llm(
    config: &api_proxy::ProxyConfig,
    name: &str,
) -> Option<String> {
    if matches!(
        config.auth,
        Some(api_proxy::ProxyAuth::CredentialBundle { .. })
    ) {
        Some(format!(
            "Error: proxy '{}' uses credential_bundle auth — this mode never returns raw \
             credentials to the LLM. Run 'lucidos proxy {} --credentials' from a script \
             (e.g. via run_python or run_bash) to get the bundle.",
            name, name
        ))
    } else {
        None
    }
}

impl LucidosEngine {
    pub(crate) async fn execute_proxy_tool(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => return Ok("Error: 'name' is required".to_string()),
        };
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if api_proxy::has_traversal(path) {
            return Ok("Error: path may not contain '..' or backslash segments".to_string());
        }
        let method_str = args.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .map(|s| Bytes::copy_from_slice(s.as_bytes()))
            .unwrap_or_default();

        let method = match Method::from_bytes(method_str.to_uppercase().as_bytes()) {
            Ok(m) => m,
            Err(_) => return Ok(format!("Error: invalid HTTP method '{}'", method_str)),
        };

        let config = match api_proxy::resolve_proxy_target(&self.workspace_path, name).await {
            Ok(c) => c,
            Err((_, msg)) => return Ok(format!("Error: {}", msg)),
        };

        if let Some(msg) = refuse_credential_bundle_for_llm(&config, name) {
            return Ok(msg);
        }

        let mut headers = HeaderMap::new();
        if let Some(h) = args.get("headers").and_then(|v| v.as_object()) {
            for (key, value) in h {
                if let Some(v) = value.as_str() {
                    if let (Ok(name), Ok(val)) = (
                        HeaderName::from_bytes(key.as_bytes()),
                        HeaderValue::from_str(v),
                    ) {
                        headers.insert(name, val);
                    }
                }
            }
        }

        let resolved = match api_proxy::apply_auth(
            &self.pool,
            config.auth.as_ref(),
            &config.base_url,
            path,
            None,
        )
        .await
        {
            Ok(r) => r,
            Err((_, msg)) => return Ok(format!("Error: {}", msg)),
        };
        log!(
            "[Proxy LLM] {} {} → {}",
            method.as_str(),
            name,
            resolved.log_url
        );
        let response = api_proxy::forward_request(
            method,
            &resolved.url,
            &resolved.log_url,
            headers,
            resolved.header,
            body,
        )
        .await;

        let status = response.status();
        let body_bytes = match axum::body::to_bytes(response.into_body(), 100 * 1024 * 1024).await {
            Ok(b) => b,
            Err(e) => return Ok(format!("Error: failed to read response body: {}", e)),
        };
        let body_text = String::from_utf8_lossy(&body_bytes).into_owned();

        if (200..300).contains(&status.as_u16()) {
            if body_text.len() > 50_000 {
                let cut = body_text.floor_char_boundary(45_000);
                Ok(format!(
                    "{}...\n[truncated, {} total bytes]",
                    &body_text[..cut],
                    body_text.len()
                ))
            } else {
                Ok(body_text)
            }
        } else {
            Ok(format!(
                "HTTP Error {}: {}",
                status.as_u16(),
                body_text.chars().take(500).collect::<String>()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::proxy::{ProxyAuth, ProxyConfig};

    #[test]
    fn refuses_credential_bundle_with_actionable_error() {
        let config = ProxyConfig {
            base_url: String::new(),
            auth: Some(ProxyAuth::CredentialBundle {
                credentials: vec!["a".to_string()],
            }),
        };
        let msg = refuse_credential_bundle_for_llm(&config, "comfort_creds")
            .expect("should refuse credential_bundle");
        assert!(msg.contains("comfort_creds"), "msg: {}", msg);
        assert!(msg.contains("--credentials"), "msg: {}", msg);
        assert!(msg.contains("lucidos proxy"), "msg: {}", msg);
    }

    #[test]
    fn allows_bearer_through() {
        let config = ProxyConfig {
            base_url: "https://x".to_string(),
            auth: Some(ProxyAuth::Bearer {
                credential: "x".to_string(),
            }),
        };
        assert!(refuse_credential_bundle_for_llm(&config, "x").is_none());
    }

    #[test]
    fn allows_unauthenticated_through() {
        let config = ProxyConfig {
            base_url: "https://x".to_string(),
            auth: None,
        };
        assert!(refuse_credential_bundle_for_llm(&config, "x").is_none());
    }
}
