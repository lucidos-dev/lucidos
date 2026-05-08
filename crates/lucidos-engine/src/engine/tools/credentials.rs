use super::super::LucidosEngine;
use crate::core::oauth;
use crate::core::CredentialStore;

/// Sentinel prefix on a tool result that the agentic loop strips off and
/// re-emits as a `CredentialRequest` SSE event for the frontend modal.
pub(crate) const CREDENTIAL_REQUEST_PREFIX: &str = "[CREDENTIAL_REQUEST]";

/// Build a `[CREDENTIAL_REQUEST]<json>` tool result for the frontend to intercept.
/// The JSON is built via `serde_json` so any characters (newlines, quotes,
/// backslashes) in the inputs are escaped correctly — `format!`-based
/// interpolation produces invalid JSON the moment a prompt has a newline.
pub(crate) fn credential_request_payload(
    service: &str,
    prompt: &str,
    base_url: &str,
    auth_type: &str,
) -> String {
    let payload = serde_json::json!({
        "service": service,
        "prompt": prompt,
        "base_url": base_url,
        "auth_type": auth_type,
    });
    format!("{CREDENTIAL_REQUEST_PREFIX}{payload}")
}

impl LucidosEngine {
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

                Ok(credential_request_payload(
                    service_name,
                    prompt,
                    base_url,
                    auth_type,
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
                    return Ok(credential_request_payload(
                        &cred_service,
                        &format!("Enter your OAuth client credentials for {provider}."),
                        &format!("https://{provider}.com"),
                        "oauth_client",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_payload(s: &str) -> serde_json::Value {
        let json_part = s
            .strip_prefix(CREDENTIAL_REQUEST_PREFIX)
            .expect("missing CREDENTIAL_REQUEST_PREFIX");
        serde_json::from_str(json_part).expect("payload must be valid JSON")
    }

    #[test]
    fn payload_is_valid_json_with_multiline_prompt() {
        let prompt = "1. Open dashboard\n2. Create API key\n3. Paste it below";
        let result = credential_request_payload(
            "binance",
            prompt,
            "https://api.binance.com",
            "api_key",
        );
        let parsed = parse_payload(&result);
        assert_eq!(parsed["service"], "binance");
        assert_eq!(parsed["prompt"], prompt);
        assert_eq!(parsed["base_url"], "https://api.binance.com");
        assert_eq!(parsed["auth_type"], "api_key");
    }

    #[test]
    fn payload_escapes_quotes_and_backslashes_in_prompt() {
        let prompt = r#"Use the "API Key" field, escape backslashes like \n correctly"#;
        let result =
            credential_request_payload("svc", prompt, "https://api.example.com", "api_key");
        let parsed = parse_payload(&result);
        assert_eq!(parsed["prompt"], prompt);
    }

    #[test]
    fn payload_handles_special_chars_in_other_fields() {
        let result = credential_request_payload(
            r#"weird"service"#,
            "prompt",
            r#"https://example.com/path with "quotes""#,
            "api_key",
        );
        let parsed = parse_payload(&result);
        assert_eq!(parsed["service"], r#"weird"service"#);
        assert_eq!(parsed["base_url"], r#"https://example.com/path with "quotes""#);
    }
}
