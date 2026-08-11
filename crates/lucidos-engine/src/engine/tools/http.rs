use super::super::LucidosEngine;
use super::bulk_limits::{bulk_threshold_error, BulkContext, MAX_BULK_BYTES};
use crate::core::oauth::{self, provider_for_url};
use crate::core::WriteAnnouncement;
use crate::core::{AuthType, CredentialStore, OAuthStore};
use bytes::Bytes;
use std::time::Duration;

/// Per-request timeout for the `http_request` LLM tool. A server that accepts
/// the connection and never responds would otherwise pin the agent loop until
/// the surrounding turn is canceled; 15s matches `tools/web.rs`. The retry
/// loop multiplies this against `MAX_RETRIES` for the upper bound.
const HTTP_TOOL_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Build the reqwest client used by `execute_http_tool`. Extracted so the
/// timeout is set in one place and tests can verify the timeout fires against
/// a hung server without depending on the full tool surface.
pub(crate) fn build_http_tool_client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// Read a response body as the bytes the server actually sent.
///
/// Deliberately NOT `Response::text()`. That decodes the body as UTF-8
/// *lossily*, so every byte the decoder can't map becomes U+FFFD, which is
/// three bytes on the way back out. `output_path` and `temp_path` persist this
/// body verbatim, so the decode silently rewrote every binary download the
/// tool has ever saved: a 220,656-byte WebP product photo landed on disk as
/// 399,717 bytes of replacement characters (2026-08-11), `file` no longer
/// recognised it as an image, and the model's `read_file` of it came back from
/// the provider as `400 Could not process image`.
///
/// Nothing is lost on the text side, and that is checkable rather than a
/// judgement call: we build reqwest with `default-features = false` and no
/// `charset` feature, so `Response::text()` is not the charset-aware decoder
/// its name suggests. It is `self.bytes()` followed by
/// `String::from_utf8_lossy` (reqwest 0.12.28 `async_impl/response.rs`), which
/// is exactly what the two model-facing branches below now do for themselves,
/// at the end, where lossiness is harmless. Enabling `charset` later would
/// change that, and those branches would then need the header.
///
/// Returns `Bytes` rather than `Vec<u8>` so the body is never copied: the
/// persisted paths want `&[u8]`, and a download may run to `MAX_BULK_BYTES`.
pub(crate) async fn read_response_body(response: reqwest::Response) -> reqwest::Result<Bytes> {
    response.bytes().await
}

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

        // Build and send request with retry for transient errors. A per-request
        // timeout bounds each attempt — without one, a server that accepts the
        // connection and never responds would pin the agent loop indefinitely.
        let client = match build_http_tool_client(HTTP_TOOL_REQUEST_TIMEOUT) {
            Ok(c) => c,
            Err(e) => {
                return Ok(format!(
                    "Error: failed to build HTTP client for http_request: {}",
                    e
                ));
            }
        };
        const MAX_RETRIES: u32 = 3;
        let mut status: u16 = 0;
        let mut body_bytes = Bytes::new();
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
                    // A body-read failure (connection dropped mid-body, decode
                    // error) is NOT a successful empty response. The old
                    // `.unwrap_or_default()` left the body empty and then
                    // cleared `request_error` unconditionally on the next line,
                    // so the tool reported success: with `output_path` set and a
                    // 2xx status, the block below wrote a ZERO-BYTE artifact
                    // over the user's file and announced it as an update.
                    body_bytes = match read_response_body(response).await {
                        Ok(b) => b,
                        Err(e) => {
                            request_error = Some(format!("reading response body: {}", e));
                            break;
                        }
                    };
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
                        "Error: temp_path must be relative with no '..' components".to_string()
                    );
                }
                let tmp_dir = self.workspace_path.join(crate::core::TMP_DIR);
                if let Err(e) = std::fs::create_dir_all(&tmp_dir) {
                    return Ok(format!(
                        "Error: failed to create tmp dir {}: {}",
                        crate::core::home_path::abbreviate(&tmp_dir),
                        e
                    ));
                }
                let full_path = tmp_dir.join(path);
                if let Some(parent) = full_path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return Ok(format!(
                            "Error: failed to create parent dir {}: {}",
                            crate::core::home_path::abbreviate(parent),
                            e
                        ));
                    }
                }
                if let Err(e) = std::fs::write(&full_path, &body_bytes) {
                    return Ok(format!(
                        "Error: failed to write {}: {}",
                        crate::core::home_path::abbreviate(&full_path),
                        e
                    ));
                }
                // The exact string `read_file` resolves, so the path this
                // announces back to the LLM is one it can act on.
                Some(format!("{}/{}", crate::core::TMP_DIR, path))
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
                let downloaded = body_bytes.len() as u64;
                if downloaded > MAX_BULK_BYTES {
                    return Ok(bulk_threshold_error(
                        BulkContext::HttpResponse { dest: data_path },
                        None,
                        downloaded,
                    ));
                }
                // The store captures the pre-write existence and announces.
                // This call site used to do both by hand, which is why the
                // helper it called (`SystemEvent::artifact_change`) existed at
                // all, and why the image tool could forget the pair entirely.
                self.artifact_manager
                    .write_and_commit(
                        &self.event_bus,
                        artifact_path,
                        &body_bytes,
                        &format!("HTTP response: {}", artifact_path),
                        WriteAnnouncement::Entity {
                            source: Some("http_request".to_string()),
                        },
                    )
                    .await?;
                // Re-read rather than publish `body_bytes`: this tool is the one
                // artifact writer that does NOT take `lock_workspace_repo`, so
                // a concurrent writer may already have landed newer bytes, and
                // the cache must never hold content the file does not.
                self.user_profile
                    .artifact_written_on_disk(self.workspace_path(), artifact_path)
                    .await;
            }
        }

        // Only the branches that hand the body back to the model decode it.
        // The model reads prose, so a replacement character in a stray byte
        // costs nothing, while the persisted copy above had to stay
        // byte-exact.
        if (200..300).contains(&status) {
            if let Some(path) = saved_temp {
                Ok(format!("[SAVED] {} ({} bytes)", path, body_bytes.len()))
            } else if let Some(ref data_path) = resolved_output {
                Ok(format!(
                    "[SAVED] {} ({} bytes)",
                    data_path,
                    body_bytes.len()
                ))
            } else {
                let body_text = String::from_utf8_lossy(&body_bytes);
                if body_text.len() > 50000 {
                    Ok(format!(
                        "{}...\n[truncated, {} total bytes]",
                        &body_text[..body_text.floor_char_boundary(45000)],
                        body_text.len()
                    ))
                } else {
                    Ok(body_text.into_owned())
                }
            }
        } else {
            Ok(format!(
                "HTTP Error {}: {}",
                status,
                String::from_utf8_lossy(&body_bytes)
                    .chars()
                    .take(500)
                    .collect::<String>()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard for `harden-engine-tools-http-no-timeout`. Before the
    /// fix, `execute_http_tool` built its reqwest client with
    /// `Client::new()` (no timeout); a peer that accepted the TCP connection
    /// and never wrote a response would pin the agent loop until the turn
    /// was canceled.
    ///
    /// We exercise `build_http_tool_client` directly with a short timeout
    /// against a `TcpListener` that accepts the connection and never writes
    /// — the production code path uses the same builder, just with the
    /// 15s `HTTP_TOOL_REQUEST_TIMEOUT`. The assertion: the request errors
    /// out within twice the configured timeout and the error reports a
    /// timeout. The test itself must finish well under a second so it can
    /// run inline with the rest of the unit suite.
    #[tokio::test]
    async fn build_http_tool_client_times_out_against_hung_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept connections but never write a response — the client must
        // bail itself. We hold each accepted stream in a sleeping task so the
        // OS doesn't see a graceful close (which would surface as `Connection
        // reset` / `IncompleteMessage` instead of a timeout).
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let timeout = Duration::from_millis(200);
        let client = build_http_tool_client(timeout).expect("client builds");
        let start = std::time::Instant::now();
        let result = client.get(format!("http://{}/", addr)).send().await;
        let elapsed = start.elapsed();

        let err = result.expect_err("hung server must surface as Err");
        assert!(err.is_timeout(), "expected timeout error, got: {:?}", err);
        assert!(
            elapsed < Duration::from_secs(1),
            "timeout fired but took {:?} — should be near {:?}",
            elapsed,
            timeout
        );
    }

    /// A client built without a timeout would have no `Instant::now()` upper
    /// bound and `.timeout()` defaults to none. We assert the builder we ship
    /// returns Ok so the production path can rely on it.
    #[test]
    fn build_http_tool_client_returns_ok_for_real_timeout() {
        assert!(build_http_tool_client(HTTP_TOOL_REQUEST_TIMEOUT).is_ok());
    }

    /// `http_request` injects stored credentials before sending. Reqwest's
    /// default redirect policy only strips sensitive headers on host/port
    /// changes, so a same-host HTTPS→HTTP downgrade would replay Authorization
    /// over plaintext. The tool client must never auto-follow after injection;
    /// callers get the 30x response and can make an explicit second request.
    #[tokio::test]
    async fn build_http_tool_client_does_not_follow_redirects_with_authorization() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        let seen_target_auth = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen_target_auth_task = seen_target_auth.clone();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = target.accept().await {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                if req.to_ascii_lowercase().contains("authorization:") {
                    seen_target_auth_task.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
            }
        });

        let redirector = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirector_addr = redirector.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = redirector.accept().await {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{}/leak\r\nContent-Length: 0\r\n\r\n",
                    target_addr
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        let client = build_http_tool_client(Duration::from_secs(1)).expect("client builds");
        let response = client
            .get(format!("http://{redirector_addr}/start"))
            .header(reqwest::header::AUTHORIZATION, "Bearer injected-secret")
            .send()
            .await
            .expect("redirect response should be returned");

        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !seen_target_auth.load(std::sync::atomic::Ordering::SeqCst),
            "client followed redirect and replayed Authorization"
        );
    }

    /// A WebP header followed by every byte value, i.e. a body that is not
    /// valid UTF-8 in the same way a real image isn't.
    fn binary_payload() -> Vec<u8> {
        let mut body = b"RIFF\x90\x5d\x03\x00WEBPVP8 ".to_vec();
        body.extend((0u16..=255).map(|b| b as u8));
        body
    }

    /// Serve `body` once over a loopback listener and return its address.
    async fn serve_once(body: Vec<u8>) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/webp\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            }
        });
        addr
    }

    /// Regression guard: `http_request` persists the response body verbatim,
    /// so the body read must preserve every byte. Reading it as text instead
    /// replaced each non-UTF-8 byte with U+FFFD and wrote that to
    /// `artifacts/`, which is how a downloaded WebP reached the provider as an
    /// undecodable file (`400 Could not process image`).
    #[tokio::test]
    async fn read_response_body_preserves_non_utf8_bytes() {
        let body = binary_payload();
        let addr = serve_once(body.clone()).await;

        let client = build_http_tool_client(Duration::from_secs(5)).expect("client builds");
        let response = client
            .get(format!("http://{addr}/image.webp"))
            .send()
            .await
            .expect("response");
        let read = read_response_body(response).await.expect("body reads");

        assert_eq!(read, body, "body must round-trip byte for byte");
    }

    /// The trap the guard above exists for, pinned so the two stay comparable:
    /// the lossy decode both mangles the bytes AND inflates the length, which
    /// is why the corruption showed up as a file larger than the original.
    #[tokio::test]
    async fn reading_the_same_body_as_text_would_corrupt_and_inflate_it() {
        let body = binary_payload();
        let addr = serve_once(body.clone()).await;

        let client = build_http_tool_client(Duration::from_secs(5)).expect("client builds");
        let text = client
            .get(format!("http://{addr}/image.webp"))
            .send()
            .await
            .expect("response")
            .text()
            .await
            .expect("body reads");

        assert_ne!(text.as_bytes(), body.as_slice());
        assert!(
            text.len() > body.len(),
            "lossy decode should inflate {} bytes, got {}",
            body.len(),
            text.len()
        );
        assert!(text.contains('\u{FFFD}'));
    }

    /// The two tests above only exercise the helper. This one pins the tool
    /// itself to it: `execute_http_tool` must not reach for `.text()` on the
    /// response, because that body is what `output_path` / `temp_path` write
    /// to disk. The error-formatting branches decode `body_bytes` at the end
    /// instead, so no `.text()` call belongs in this file's production code.
    #[test]
    fn the_http_tool_never_reads_its_response_body_as_text() {
        let source = crate::test_support::source_scan::read_production_source(
            &crate::test_support::source_scan::src_root().join("engine/tools/http.rs"),
        );
        assert!(
            !source.contains(".text().await"),
            "engine/tools/http.rs reads the response body as text again. That is \
             a lossy UTF-8 decode and the body is persisted verbatim, so use \
             read_response_body instead."
        );
    }
}
