use super::super::CognosEngine;
use crate::runtime::BrowserLogins;
use uuid::Uuid;

impl CognosEngine {
    pub(crate) async fn execute_browser_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        request_id: Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "browser_open" => {
                let url = args["url"].as_str().unwrap_or("");
                if url.is_empty() {
                    return Ok("Error: url is required".to_string());
                }
                let wait_for = args.get("wait_for").and_then(|v| v.as_str());
                let visible = args
                    .get("visible")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let skey = request_id.to_string();
                match self
                    .browser_runtime
                    .open(&skey, url, wait_for, visible)
                    .await
                {
                    Ok(content) => Ok(content),
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "browser_extract" => {
                let selector = args["selector"].as_str().unwrap_or("");
                let format = args["format"].as_str().unwrap_or("text");
                if selector.is_empty() {
                    return Ok("Error: selector is required".to_string());
                }
                let skey = request_id.to_string();
                match self.browser_runtime.extract(&skey, selector, format).await {
                    Ok(content) => Ok(content),
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "browser_click" => {
                let selector = args["selector"].as_str().unwrap_or("");
                if selector.is_empty() {
                    return Ok("Error: selector is required".to_string());
                }
                let wait_navigation = args
                    .get("wait_navigation")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let skey = request_id.to_string();
                match self
                    .browser_runtime
                    .click(&skey, selector, wait_navigation)
                    .await
                {
                    Ok(result) => Ok(result),
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "browser_type" => {
                let selector = args["selector"].as_str().unwrap_or("");
                let text = args["text"].as_str().unwrap_or("");
                if selector.is_empty() || text.is_empty() {
                    return Ok("Error: selector and text are required".to_string());
                }
                let clear = args.get("clear").and_then(|v| v.as_bool()).unwrap_or(false);
                let press_enter = args.get("enter").and_then(|v| v.as_bool()).unwrap_or(false);
                let skey = request_id.to_string();
                match self
                    .browser_runtime
                    .type_text(&skey, selector, text, clear, press_enter)
                    .await
                {
                    Ok(result) => Ok(result),
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "browser_eval" => {
                let script = args["script"].as_str().unwrap_or("");
                if script.is_empty() {
                    return Ok("Error: script is required".to_string());
                }
                let skey = request_id.to_string();
                match self.browser_runtime.evaluate(&skey, script).await {
                    Ok(result) => Ok(result),
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "browser_screenshot" => {
                let raw_path = args["path"].as_str().unwrap_or("");
                if raw_path.is_empty() {
                    return Ok("Error: path is required".to_string());
                }
                // Normalize through resolve_data_path, then extract artifact-relative portion
                // for the browser runtime (which prepends ARTIFACTS_DIR internally)
                let (data_path, _) = match self.resolve_data_path(raw_path) {
                    Ok(p) => p,
                    Err(e) => return Ok(format!("Error: {}", e)),
                };
                let path = match data_path.strip_prefix("artifacts/") {
                    Some(p) => p,
                    None => {
                        return Ok(format!(
                            "Error: screenshot path must be under artifacts/, got: {}",
                            data_path
                        ))
                    }
                };
                let selector = args.get("selector").and_then(|v| v.as_str());
                let full_page = args
                    .get("full_page")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let url = args.get("url").and_then(|v| v.as_str());
                let skey = request_id.to_string();
                match self
                    .browser_runtime
                    .screenshot(&skey, path, selector, full_page, url)
                    .await
                {
                    Ok(result) => {
                        // Extract the actual timestamped path from the result
                        // Format: "Screenshot saved to artifacts/{path} ({size} bytes)"
                        let actual_path = result
                            .find("artifacts/")
                            .and_then(|start| {
                                result[start..]
                                    .find(" (")
                                    .map(|end| result[start + 10..start + end].to_string())
                            })
                            .unwrap_or_else(|| path.to_string());

                        // Commit the screenshot to git
                        if let Err(e) = self
                            .artifact_manager
                            .commit(&actual_path, &format!("Screenshot: {}", actual_path))
                            .await
                        {
                            log!(@screenshot, "Failed to commit: {}", e);
                        }
                        Ok(result)
                    }
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "browser_close" => {
                let skey = request_id.to_string();
                match self.browser_runtime.close(&skey).await {
                    Ok(result) => Ok(result),
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "browser_forget_login" => {
                let domain = args["domain"].as_str().unwrap_or("");
                if domain.is_empty() {
                    return Ok("Error: domain is required".to_string());
                }
                match BrowserLogins::remove(&self.pool, domain).await {
                    Ok(()) => Ok(format!("Forgot login for {}", domain)),
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "browser_clear_data" => match self.browser_runtime.clear_data().await {
                Ok(result) => Ok(result),
                Err(e) => Ok(format!("Error: {}", e)),
            },
            _ => Ok(format!("Unknown browser tool: {}", name)),
        }
    }
}
