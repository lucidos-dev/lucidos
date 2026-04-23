use super::super::CognosEngine;
use crate::core::oauth;
use crate::core::CredentialStore;

impl CognosEngine {
    pub(crate) async fn execute_credential_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "request_credential" => {
                let service_name = args["service_name"].as_str().unwrap_or("");
                let prompt = args["prompt"].as_str().unwrap_or("");
                let base_url = args["base_url"].as_str().unwrap_or("");
                let auth_type = args["auth_type"].as_str().unwrap_or("api_key");

                if service_name.is_empty() || prompt.is_empty() || base_url.is_empty() {
                    return Ok("Error: service_name, prompt, and base_url are required".to_string());
                }

                // Check if credential already exists
                if let Ok(Some(_)) = CredentialStore::get(&self.pool, service_name).await {
                    return Ok(format!(
                        "Credentials for '{}' are already configured. You can proceed with API requests.",
                        service_name
                    ));
                }

                // Return a special response that the frontend will intercept to show a modal
                // The SSE handler in the frontend will parse this and show a credential input modal
                Ok(format!(
                    "[CREDENTIAL_REQUEST]{{\"service\":\"{}\",\"prompt\":\"{}\",\"base_url\":\"{}\",\"auth_type\":\"{}\"}}",
                    service_name,
                    prompt.replace('\"', "\\\""),
                    base_url,
                    auth_type
                ))
            }
            "connect_oauth_account" => {
                let provider = args["provider"].as_str().unwrap_or("").to_lowercase();
                let scopes = args["scopes"].as_str().unwrap_or("");

                if provider.is_empty() || scopes.is_empty() {
                    return Ok("Error: provider and scopes are required".to_string());
                }

                // Check if client credentials exist for this provider
                let cred_service = format!("oauth:{}", provider);
                if CredentialStore::get(&self.pool, &cred_service)
                    .await?
                    .is_none()
                {
                    // Request client credentials via the credential modal
                    return Ok(format!(
                        "[CREDENTIAL_REQUEST]{{\"service\":\"oauth:{provider}\",\"prompt\":\"Enter your OAuth client credentials for {provider}.\",\"base_url\":\"https://{provider}.com\",\"auth_type\":\"oauth_client\"}}"
                    ));
                }

                let (email, _display_name, _merged_scopes) =
                    oauth::run_oauth_flow(&self.pool, &provider, scopes).await?;

                let email_display = email.as_deref().unwrap_or("unknown");
                Ok(format!(
                    "Successfully connected {} account ({}).",
                    provider, email_display
                ))
            }
            _ => Ok(format!("Unknown credential tool: {}", name)),
        }
    }
}

/// Wait for an OAuth callback on the given listener, extract the authorization code.
pub(crate) async fn wait_for_oauth_callback(
    listener: tokio::net::TcpListener,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let (stream, _) = listener.accept().await?;
    let mut buf = vec![0u8; 4096];
    stream.readable().await?;
    let n = stream.try_read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse the GET request line to extract the code parameter
    let first_line = request.lines().next().unwrap_or("");
    let path = first_line.split_whitespace().nth(1).unwrap_or("");
    let code = path
        .split('?')
        .nth(1)
        .and_then(|q| {
            q.split('&')
                .find(|p| p.starts_with("code="))
                .map(|p| p.trim_start_matches("code=").to_string())
        })
        .ok_or("No authorization code in callback")?;

    // Send a success response to the browser
    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h2>Authorization successful!</h2><p>You can close this tab and return to CognOS.</p></body></html>";
    stream.writable().await?;
    let _ = stream.try_write(response.as_bytes());

    // URL-decode the code
    let code = urlencoding::decode(&code)?.into_owned();

    Ok(code)
}
