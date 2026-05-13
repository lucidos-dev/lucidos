//! `proxy_request` + `reload_proxy_modules` LLM tools.
//!
//! `proxy_request` calls a backend configured in `data/config/apis.json`
//! through the engine proxy — the credential value never reaches the
//! model; only the configured proxy *name* does.
//!
//! `reload_proxy_modules` re-scans `data/auth-modules/` and atomically
//! swaps the engine's compiled-module map. Same code path as the HTTP
//! `POST /api/v1/proxy-modules/reload` endpoint and as the post-install
//! reload triggered by `install_plugin` when the plugin ships
//! `auth-modules/` content.

use super::super::LucidosEngine;
use crate::api::proxy as api_proxy;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method};

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

        log!("[Proxy LLM] {} {}", method.as_str(), name);
        let engine_arc = self.clone_arc();
        let response = match api_proxy::dispatch_proxy_request(
            &engine_arc,
            name,
            &config,
            method,
            path.to_string(),
            None,
            headers,
            body,
        )
        .await
        {
            Ok(r) => r,
            Err((_, msg)) => return Ok(format!("Error: {}", msg)),
        };

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

    pub(crate) async fn execute_reload_proxy_modules_tool(
        &self,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match crate::api::proxy::reload_proxy_modules_into(self, &self.workspace_path).await {
            Ok(names) => Ok(serde_json::json!({"loaded": names}).to_string()),
            Err(e) => Ok(format!("Error: WASM module reload failed: {}", e)),
        }
    }
}

