//! `proxy_request` LLM tool — call a backend configured in
//! `data/config/apis.json` through the engine proxy. The credential value
//! never reaches the model; only the configured proxy *name* does.

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

        let config =
            match api_proxy::resolve_proxy_target(&self.workspace_path, name).await {
                Ok(c) => c,
                Err((_, msg)) => return Ok(format!("Error: {}", msg)),
            };

        let auth_header = if let Some(auth) = &config.auth {
            let cred = match api_proxy::resolve_credential(&self.pool, auth).await {
                Ok(c) => c,
                Err((_, msg)) => return Ok(format!("Error: {}", msg)),
            };
            api_proxy::build_auth_header(cred.auth_type, &cred.auth_value, auth.header.as_deref())
        } else {
            None
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

        let target_url = api_proxy::build_target_url(&config.base_url, path, None);
        log!(
            "[Proxy LLM] {} {} → {}",
            method.as_str(),
            name,
            target_url
        );
        let response =
            api_proxy::forward_request(method, &target_url, headers, auth_header, body).await;

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
