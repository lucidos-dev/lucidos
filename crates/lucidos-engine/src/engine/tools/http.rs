use super::super::LucidosEngine;
use super::bulk_limits::{bulk_threshold_error, BulkContext, MAX_BULK_BYTES};
use crate::core::oauth::{self, provider_for_url};
use crate::core::{AuthType, CredentialStore, OAuthStore};
use crate::engine::event_bus::{BusEvent, SystemEvent};

impl LucidosEngine {
    pub(crate) async fn execute_http_tool(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let method = args["method"].as_str().unwrap_or("GET");
        let url = args["url"].as_str().unwrap_or("");

        if url.is_empty() {
            return Ok("Error: URL is required".to_string());
        }

        // Parse headers if provided
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(h) = args.get("headers").and_then(|v| v.as_object()) {
            for (key, value) in h {
                if let Some(v) = value.as_str() {
                    if let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                        reqwest::header::HeaderValue::from_str(v),
                    ) {
                        headers.insert(name, val);
                    }
                }
            }
        }

        // Always inject stored credentials when available (overrides LLM-provided
        // auth headers which may contain stale tokens or template placeholders).
        if let Ok(Some(cred)) = CredentialStore::find_by_url(&self.pool, url).await {
            let auth_value = match cred.auth_type {
                AuthType::Bearer => format!("Bearer {}", cred.auth_value),
                AuthType::Basic => format!("Basic {}", cred.auth_value),
                AuthType::Password => {
                    // Password credentials store {"username":"...","password":"..."} —
                    // inject as Basic auth (base64 of username:password).
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&cred.auth_value)
                    {
                        let user = parsed["username"].as_str().unwrap_or("");
                        let pass = parsed["password"].as_str().unwrap_or("");
                        use base64::Engine as _;
                        let encoded = base64::engine::general_purpose::STANDARD
                            .encode(format!("{}:{}", user, pass));
                        format!("Basic {}", encoded)
                    } else {
                        cred.auth_value.clone()
                    }
                }
                _ => cred.auth_value.clone(), // api_key: use raw value
            };
            if let Ok(header_name) =
                reqwest::header::HeaderName::from_bytes(cred.auth_header.as_bytes())
            {
                if let Ok(header_val) = reqwest::header::HeaderValue::from_str(&auth_value) {
                    headers.insert(header_name, header_val);
                    log!(@http_request, "Injected {} credentials for {}", cred.service_name, url);
                }
            }
        }

        // OAuth token injection (overrides static credentials for OAuth providers)
        let oauth_provider = provider_for_url(url);
        if let Some(oauth_provider) = oauth_provider {
            match OAuthStore::get_by_provider(&self.pool, oauth_provider).await {
                Ok(Some(mut account)) => {
                    // Refresh if expired or expiring within 60s
                    match oauth::refresh_oauth_if_needed(&self.pool, &mut account).await {
                        Ok(()) => {}
                        Err(e) => {
                            log!("[OAuth] Token refresh failed for {}: {}", oauth_provider, e)
                        }
                    }

                    headers.insert(
                        reqwest::header::AUTHORIZATION,
                        reqwest::header::HeaderValue::from_str(&format!(
                            "Bearer {}",
                            account.access_token
                        ))
                        .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static("")),
                    );
                    log!("[OAuth] Injected {} token for {}", oauth_provider, url);
                }
                Ok(None) => log!(
                    "[OAuth] No {} account found, skipping token injection for {}",
                    oauth_provider,
                    url
                ),
                Err(e) => log!(
                    "[OAuth] Failed to look up {} account: {}",
                    oauth_provider,
                    e
                ),
            }
        }

        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Resolve persistence targets up-front so the request loop can pre-check
        // Content-Length against the bulk threshold without buffering a huge body.
        let output_path = args["output_path"].as_str();
        let temp_path = args["temp_path"].as_str();
        let resolved_output = if let Some(raw_path) = output_path {
            match self.resolve_data_path(raw_path) {
                Ok((data_path, _)) => Some(data_path),
                Err(e) => return Ok(format!("Error: {}", e)),
            }
        } else {
            None
        };

        // Build and send request with retry for transient errors
        let client = reqwest::Client::new();
        const MAX_RETRIES: u32 = 3;
        let mut status: u16 = 0;
        let mut body_text = String::new();
        let mut request_error: Option<String> = None;

        for attempt in 0..=MAX_RETRIES {
            let request_builder = match method {
                "GET" => client.get(url),
                "POST" => client.post(url),
                "PUT" => client.put(url),
                "DELETE" => client.delete(url),
                _ => return Ok(format!("Error: Unsupported method: {}", method)),
            };

            let request_builder = request_builder.headers(headers.clone());
            let request_builder = if let Some(ref b) = body {
                request_builder.body(b.clone())
            } else {
                request_builder
            };

            match request_builder.send().await {
                Ok(response) => {
                    status = response.status().as_u16();
                    // Pre-check Content-Length against the bulk threshold before
                    // reading the body — avoids allocating hundreds of MB when an
                    // honest server tells us the size and the LLM asked us to
                    // persist it under data/artifacts/.
                    if (200..300).contains(&status) {
                        if let (Some(data_path), Some(len)) =
                            (resolved_output.as_ref(), response.content_length())
                        {
                            if len > MAX_BULK_BYTES {
                                return Ok(bulk_threshold_error(
                                    BulkContext::HttpResponse { dest: data_path },
                                    None,
                                    len,
                                ));
                            }
                        }
                    }
                    body_text = response.text().await.unwrap_or_default();
                    request_error = None;

                    // Retry on transient errors: 401, 429, 5xx
                    let retryable = status == 401 || status == 429 || status >= 500;
                    if retryable && attempt < MAX_RETRIES {
                        // On 401 with OAuth provider, force-refresh the token before retrying
                        if status == 401 {
                            if let Some(provider) = oauth_provider {
                                match OAuthStore::get_by_provider(&self.pool, provider).await {
                                    Ok(Some(mut account)) => {
                                        match oauth::force_refresh_oauth(&self.pool, &mut account).await {
                                            Ok(()) => {
                                                if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", account.access_token)) {
                                                    headers.insert(reqwest::header::AUTHORIZATION, val);
                                                }
                                                log!("[OAuth] Re-refreshed {} token after 401 (attempt {}/{})",
                                                    provider, attempt + 1, MAX_RETRIES);
                                            }
                                            Err(e) => log!("[OAuth] Force-refresh failed for {} after 401 (attempt {}/{}): {}",
                                                provider, attempt + 1, MAX_RETRIES, e),
                                        }
                                    }
                                    Ok(None) => {
                                        log!("[OAuth] No {} account found for 401 retry", provider)
                                    }
                                    Err(e) => log!(
                                        "[OAuth] Failed to look up {} account for 401 retry: {}",
                                        provider,
                                        e
                                    ),
                                }
                            }
                        }
                        let delay = 1u64 << attempt; // 1s, 2s, 4s
                        log!(@http_request, "HTTP {} from {} — retrying in {}s (attempt {}/{})",
                            status, url, delay, attempt + 1, MAX_RETRIES);
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                        continue;
                    }
                    break;
                }
                Err(e) => {
                    request_error = Some(format!("{}", e));
                    if attempt < MAX_RETRIES {
                        let delay = 1u64 << attempt;
                        log!(@http_request, "Request failed for {} — retrying in {}s (attempt {}/{}): {}",
                            url, delay, attempt + 1, MAX_RETRIES, e);
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                        continue;
                    }
                    break;
                }
            }
        }

        // Connection error after all retries
        if let Some(err) = request_error {
            return Ok(format!(
                "Error: Request failed after {} retries: {}",
                MAX_RETRIES, err
            ));
        }

        // Handle temp_path - save to .lucidos/tmp/ (not git-tracked)
        let saved_temp = if let Some(path) = temp_path {
            if (200..300).contains(&status) {
                if crate::api::is_path_traversal(path) {
                    return Ok(
                        "Error: temp_path must be relative with no '..' components".to_string(),
                    );
                }
                let tmp_dir = self.workspace_path.join(".lucidos").join("tmp");
                if let Err(e) = std::fs::create_dir_all(&tmp_dir) {
                    return Ok(format!(
                        "Error: failed to create tmp dir {}: {}",
                        tmp_dir.display(),
                        e
                    ));
                }
                let full_path = tmp_dir.join(path);
                if let Some(parent) = full_path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return Ok(format!(
                            "Error: failed to create parent dir {}: {}",
                            parent.display(),
                            e
                        ));
                    }
                }
                if let Err(e) = std::fs::write(&full_path, &body_text) {
                    return Ok(format!(
                        "Error: failed to write {}: {}",
                        full_path.display(),
                        e
                    ));
                }
                Some(format!(".lucidos/tmp/{}", path))
            } else {
                None
            }
        } else {
            None
        };

        // Handle output_path - save to artifacts/ (git-tracked). Pre-checked
        // Content-Length above; this re-checks actual body size as a safety net
        // for servers that lie or omit the header.
        if let Some(ref data_path) = resolved_output {
            if (200..300).contains(&status) {
                let artifact_path = match data_path.strip_prefix("artifacts/") {
                    Some(p) => p,
                    None => {
                        return Ok(format!(
                            "Error: output_path must be under artifacts/, got: {}",
                            data_path
                        ))
                    }
                };
                let body_bytes = body_text.len() as u64;
                if body_bytes > MAX_BULK_BYTES {
                    return Ok(bulk_threshold_error(
                        BulkContext::HttpResponse { dest: data_path },
                        None,
                        body_bytes,
                    ));
                }
                let file_exists = self.artifact_manager.artifact_exists(artifact_path);
                let commit_sha = self
                    .artifact_manager
                    .write_and_commit(
                        artifact_path,
                        &body_text,
                        &format!("HTTP response: {}", artifact_path),
                    )
                    .await?;

                self.event_bus
                    .emit(BusEvent::System(SystemEvent::artifact_change(
                        file_exists,
                        artifact_path.to_string(),
                        commit_sha.clone(),
                        Some("http_request".to_string()),
                    )))
                    .await?;
            }
        }

        if (200..300).contains(&status) {
            if let Some(path) = saved_temp {
                Ok(format!("[SAVED] {} ({} bytes)", path, body_text.len()))
            } else if let Some(ref data_path) = resolved_output {
                Ok(format!("[SAVED] {} ({} bytes)", data_path, body_text.len()))
            } else if body_text.len() > 50000 {
                Ok(format!(
                    "{}...\n[truncated, {} total bytes]",
                    &body_text[..body_text.floor_char_boundary(45000)],
                    body_text.len()
                ))
            } else {
                Ok(body_text)
            }
        } else {
            Ok(format!(
                "HTTP Error {}: {}",
                status,
                body_text.chars().take(500).collect::<String>()
            ))
        }
    }
}
