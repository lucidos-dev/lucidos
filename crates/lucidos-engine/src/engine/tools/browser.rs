use super::super::LucidosEngine;
use crate::runtime::BrowserLogins;
use uuid::Uuid;

/// The `artifacts/`-relative path out of `browser_screenshot`'s result line,
/// `"Screenshot saved to artifacts/<path> (<n> bytes)"`. `None` when the line
/// does not carry one.
///
/// The runtime stamps a timestamp into the name, so the caller's requested path
/// is not the one on disk. Two consumers read it back: the commit below, and
/// the agentic loop's screenshot list.
///
/// `rfind`, because the size marker is the LAST ` (` on the line. Under `find`
/// a screenshot named `report (final).png` truncated at its own parenthesis,
/// and the commit then named a file that does not exist.
pub(crate) fn screenshot_artifact_path(result: &str) -> Option<&str> {
    const PREFIX: &str = "artifacts/";
    let start = result.find(PREFIX)?;
    let rest = &result[start + PREFIX.len()..];
    Some(&rest[..rest.rfind(" (")?])
}

impl LucidosEngine {
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
                        let actual_path = screenshot_artifact_path(&result)
                            .unwrap_or(path)
                            .to_string();

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

#[cfg(test)]
mod tests {
    use super::screenshot_artifact_path;

    #[test]
    fn reads_the_timestamped_path_the_runtime_reports() {
        assert_eq!(
            screenshot_artifact_path(
                "Screenshot saved to artifacts/shots/page-20260829.png (5120 bytes)"
            ),
            Some("shots/page-20260829.png")
        );
    }

    /// The regression: `find(" (")` stopped at the name's own parenthesis, so
    /// the commit named a file that does not exist.
    #[test]
    fn a_parenthesis_in_the_name_does_not_truncate_the_path() {
        assert_eq!(
            screenshot_artifact_path("Screenshot saved to artifacts/report (final).png (99 bytes)"),
            Some("report (final).png")
        );
    }

    #[test]
    fn a_line_carrying_no_path_answers_none() {
        assert_eq!(screenshot_artifact_path("Error: page did not load"), None);
        assert_eq!(screenshot_artifact_path("saved to artifacts/x.png"), None);
    }
}
