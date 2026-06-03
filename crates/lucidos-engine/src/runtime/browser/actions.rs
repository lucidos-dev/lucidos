use super::*;
use crate::core::ARTIFACTS_DIR;
use chrono::Local;
use std::path::Path;
use std::time::Duration;

impl BrowserRuntime {
    pub async fn open(
        &self,
        session_key: &str,
        url: &str,
        wait_for: Option<&str>,
        visible: bool,
    ) -> Result<String, String> {
        let domain = extract_domain(url);

        // If headless, check blocklist first
        if !visible {
            if let Some(ref domain) = domain {
                if let Ok(Some(reason)) = HeadlessBlocklist::is_blocked(&self.pool, domain).await {
                    return Err(format!(
                        "{} blocks headless browsers ({}). Retry with visible=true to browse this site.",
                        domain, reason
                    ));
                }
            }
        }

        self.ensure_browser(session_key, visible).await?;
        let result = self
            .with_timeout(session_key, "open", || {
                self.open_inner(session_key, url, wait_for)
            })
            .await;

        // If headless, check result for bot detection indicators
        if !visible {
            if let Ok(ref content) = result {
                if let Some(reason) = detect_bot_block(content) {
                    // Record for future fast-fail
                    if let Some(ref domain) = domain {
                        if let Err(e) =
                            HeadlessBlocklist::block(&self.pool, domain, &reason).await
                        {
                            log!("[Browser] Failed to record headless block for {}: {}", domain, e);
                        } else {
                            log!("[Browser] Recorded headless block for {}: {}", domain, reason);
                        }
                    }
                    return Err(format!(
                        "{} blocks headless browsers ({}). Retry with visible=true to browse this site.",
                        domain.as_deref().unwrap_or(url),
                        reason
                    ));
                }
            }
        }

        result
    }

    async fn open_inner(
        &self,
        session_key: &str,
        url: &str,
        wait_for: Option<&str>,
    ) -> Result<String, String> {
        let mut instances = self.instances.lock().await;
        let browser_state = instances
            .get_mut(session_key)
            .ok_or("Browser not initialized")?;

        // Close previous page to avoid leaking tabs
        if let Some(old_page) = browser_state.current_page.take() {
            let _ = old_page.close().await;
        }

        // Create new page
        let page = browser_state
            .browser
            .new_page(url)
            .await
            .map_err(|e| format!("Failed to open page: {}", e))?;

        // Stealth: clear navigator.webdriver before page scripts run
        let _ = page
            .evaluate("Object.defineProperty(navigator, 'webdriver', {get: () => undefined})")
            .await;

        // Wait for optional selector
        if let Some(selector) = wait_for {
            page.wait_for_navigation()
                .await
                .map_err(|e| format!("Navigation timeout: {}", e))?;
            page.find_element(selector)
                .await
                .map_err(|e| format!("Element '{}' not found: {}", selector, e))?;
        } else {
            // Wait for page to load
            if let Err(e) = page.wait_for_navigation().await {
                log!("[Browser] Navigation timeout (continuing): {}", e);
            }
        }

        // Auto-dismiss cookie consent dialogs
        super::super::browser_consent::dismiss_cookie_consent(&page).await;

        // Get page text content
        let content = page
            .evaluate("document.body.innerText")
            .await
            .map_err(|e| format!("Failed to get page content: {}", e))?
            .into_value::<String>()
            .unwrap_or_default();

        // Store current page
        browser_state.current_page = Some(page);

        // Truncate if too long
        let content = if content.len() > 50000 {
            format!(
                "{}...\n\n[Content truncated, {} total characters]",
                &content[..content.floor_char_boundary(45000)],
                content.len()
            )
        } else {
            content
        };

        Ok(content)
    }

    /// Extract content from elements matching a selector
    pub async fn extract(
        &self,
        session_key: &str,
        selector: &str,
        format: &str, // "text", "html", "links", "table"
    ) -> Result<String, String> {
        let selector = selector.to_string();
        let format = format.to_string();
        let session_key2 = session_key.to_string();
        self.with_timeout(session_key, "extract", || {
            self.extract_inner(&session_key2, &selector, &format)
        })
        .await
    }

    async fn extract_inner(
        &self,
        session_key: &str,
        selector: &str,
        format: &str,
    ) -> Result<String, String> {
        let mut instances = self.instances.lock().await;
        let browser_state = instances
            .get_mut(session_key)
            .ok_or("Browser not initialized")?;

        let page = browser_state
            .current_page
            .as_ref()
            .ok_or("No page open. Use browser_open first.")?;

        match format {
            "text" => {
                let js = format!(
                    r#"
                    Array.from(document.querySelectorAll('{}'))
                        .map(el => el.innerText)
                        .join('\n---\n')
                    "#,
                    selector.replace('\'', "\\'")
                );
                let result = page
                    .evaluate(js)
                    .await
                    .map_err(|e| format!("Failed to extract text: {}", e))?
                    .into_value::<String>()
                    .unwrap_or_default();
                Ok(result)
            }
            "html" => {
                let js = format!(
                    r#"
                    Array.from(document.querySelectorAll('{}'))
                        .map(el => el.outerHTML)
                        .join('\n')
                    "#,
                    selector.replace('\'', "\\'")
                );
                let result = page
                    .evaluate(js)
                    .await
                    .map_err(|e| format!("Failed to extract HTML: {}", e))?
                    .into_value::<String>()
                    .unwrap_or_default();
                Ok(result)
            }
            "links" => {
                let js = format!(
                    r#"
                    Array.from(document.querySelectorAll('{}'))
                        .map(el => {{
                            if (el.tagName === 'A') {{
                                return el.href + ' | ' + (el.innerText || el.title || '').trim();
                            }}
                            return Array.from(el.querySelectorAll('a'))
                                .map(a => a.href + ' | ' + (a.innerText || a.title || '').trim())
                                .join('\n');
                        }})
                        .join('\n')
                    "#,
                    selector.replace('\'', "\\'")
                );
                let result = page
                    .evaluate(js)
                    .await
                    .map_err(|e| format!("Failed to extract links: {}", e))?
                    .into_value::<String>()
                    .unwrap_or_default();
                Ok(result)
            }
            "table" => {
                let js = format!(
                    r#"
                    (function() {{
                        const table = document.querySelector('{}');
                        if (!table) return 'No table found';
                        const rows = Array.from(table.querySelectorAll('tr'));
                        return rows.map(row => {{
                            const cells = Array.from(row.querySelectorAll('th, td'));
                            return cells.map(cell => cell.innerText.trim()).join(' | ');
                        }}).join('\n');
                    }})()
                    "#,
                    selector.replace('\'', "\\'")
                );
                let result = page
                    .evaluate(js)
                    .await
                    .map_err(|e| format!("Failed to extract table: {}", e))?
                    .into_value::<String>()
                    .unwrap_or_default();
                Ok(result)
            }
            _ => Err(format!(
                "Unknown format '{}'. Use: text, html, links, or table",
                format
            )),
        }
    }

    /// Click an element
    pub async fn click(
        &self,
        session_key: &str,
        selector: &str,
        wait_navigation: bool,
    ) -> Result<String, String> {
        let selector = selector.to_string();
        let session_key2 = session_key.to_string();
        self.with_timeout(session_key, "click", || {
            self.click_inner(&session_key2, &selector, wait_navigation)
        })
        .await
    }

    async fn click_inner(
        &self,
        session_key: &str,
        selector: &str,
        wait_navigation: bool,
    ) -> Result<String, String> {
        let mut instances = self.instances.lock().await;
        let browser_state = instances
            .get_mut(session_key)
            .ok_or("Browser not initialized")?;

        let page = browser_state
            .current_page
            .as_ref()
            .ok_or("No page open. Use browser_open first.")?;

        // Find and click the element
        let element = page
            .find_element(selector)
            .await
            .map_err(|e| format!("Element '{}' not found: {}", selector, e))?;

        element
            .click()
            .await
            .map_err(|e| format!("Failed to click: {}", e))?;

        if wait_navigation {
            page.wait_for_navigation().await.ok(); // Ignore timeout
        } else {
            // Small delay for JS to process click
            tokio::time::sleep(Duration::from_millis(300)).await;
        }

        Ok(format!("Clicked element: {}", selector))
    }

    /// Type text into an input field
    pub async fn type_text(
        &self,
        session_key: &str,
        selector: &str,
        text: &str,
        clear: bool,
        press_enter: bool,
    ) -> Result<String, String> {
        let selector = selector.to_string();
        let text = text.to_string();
        let session_key2 = session_key.to_string();
        self.with_timeout(session_key, "type_text", || {
            self.type_text_inner(&session_key2, &selector, &text, clear, press_enter)
        })
        .await
    }

    async fn type_text_inner(
        &self,
        session_key: &str,
        selector: &str,
        text: &str,
        clear: bool,
        press_enter: bool,
    ) -> Result<String, String> {
        let mut instances = self.instances.lock().await;
        let browser_state = instances
            .get_mut(session_key)
            .ok_or("Browser not initialized")?;

        let page = browser_state
            .current_page
            .as_ref()
            .ok_or("No page open. Use browser_open first.")?;

        let element = page
            .find_element(selector)
            .await
            .map_err(|e| format!("Element '{}' not found: {}", selector, e))?;

        if clear {
            // Clear existing content
            element
                .click()
                .await
                .map_err(|e| format!("Failed to focus: {}", e))?;
            page.evaluate(format!(
                "document.querySelector('{}').value = ''",
                selector.replace('\'', "\\'")
            ))
            .await
            .ok();
        }

        // Type the text
        element
            .type_str(text)
            .await
            .map_err(|e| format!("Failed to type: {}", e))?;

        if press_enter {
            page.evaluate(format!(
                "document.querySelector('{}').dispatchEvent(new KeyboardEvent('keydown', {{key: 'Enter', code: 'Enter', keyCode: 13, which: 13}}))",
                selector.replace('\'', "\\'")
            ))
            .await
            .ok();
            // Also try form submission
            page.evaluate(format!(
                "document.querySelector('{}').form?.submit()",
                selector.replace('\'', "\\'")
            ))
            .await
            .ok();

            // Wait for potential navigation
            page.wait_for_navigation().await.ok();
        }

        Ok(format!("Typed {} characters into {}", text.len(), selector))
    }

    /// Execute JavaScript and return the result
    pub async fn evaluate(&self, session_key: &str, script: &str) -> Result<String, String> {
        let script = script.to_string();
        let session_key2 = session_key.to_string();
        self.with_timeout(session_key, "evaluate", || {
            self.evaluate_inner(&session_key2, &script)
        })
        .await
    }

    async fn evaluate_inner(&self, session_key: &str, script: &str) -> Result<String, String> {
        let mut instances = self.instances.lock().await;
        let browser_state = instances
            .get_mut(session_key)
            .ok_or("Browser not initialized")?;

        let page = browser_state
            .current_page
            .as_ref()
            .ok_or("No page open. Use browser_open first.")?;

        let result = page
            .evaluate(script)
            .await
            .map_err(|e| format!("JavaScript error: {}", e))?;

        // Try to get result as string, JSON, or describe it
        if let Ok(s) = result.clone().into_value::<String>() {
            Ok(s)
        } else if let Ok(n) = result.clone().into_value::<f64>() {
            Ok(n.to_string())
        } else if let Ok(b) = result.clone().into_value::<bool>() {
            Ok(b.to_string())
        } else if let Ok(v) = result.clone().into_value::<serde_json::Value>() {
            Ok(serde_json::to_string_pretty(&v).unwrap_or_else(|_| "null".to_string()))
        } else {
            Ok("undefined".to_string())
        }
    }

    /// Take a screenshot
    /// If url is provided, navigates to that URL first (useful for taking screenshots of multiple sites)
    pub async fn screenshot(
        &self,
        session_key: &str,
        path: &str,
        selector: Option<&str>,
        full_page: bool,
        url: Option<&str>,
    ) -> Result<String, String> {
        // If URL provided, navigate to it first (open() has its own timeout)
        if let Some(url) = url {
            // Inherit the current session's visibility, default to headless
            let visible = {
                let instances = self.instances.lock().await;
                instances
                    .get(session_key)
                    .map(|s| s.visible)
                    .unwrap_or(false)
            };
            self.open(session_key, url, None, visible).await?;
        }

        let path = path.to_string();
        let selector = selector.map(ToString::to_string);
        let session_key2 = session_key.to_string();
        self.with_timeout(session_key, "screenshot", || {
            self.screenshot_inner(&session_key2, &path, selector.as_deref(), full_page)
        })
        .await
    }

    async fn screenshot_inner(
        &self,
        session_key: &str,
        path: &str,
        selector: Option<&str>,
        full_page: bool,
    ) -> Result<String, String> {
        let mut instances = self.instances.lock().await;
        let browser_state = instances
            .get_mut(session_key)
            .ok_or("Browser not initialized")?;

        let page = browser_state
            .current_page
            .as_ref()
            .ok_or("No page open. Use browser_open first.")?;

        // Dismiss any cookie dialogs before taking screenshot
        super::super::browser_consent::dismiss_cookie_consent(page).await;

        // For full page screenshots, we need to scroll and capture the entire page
        // chromiumoxide 0.7 doesn't have full_page support, so we'll use a simpler approach
        let screenshot_data: Vec<u8> = if let Some(sel) = selector {
            // Screenshot specific element
            let element = page
                .find_element(sel)
                .await
                .map_err(|e| format!("Element '{}' not found: {}", sel, e))?;
            element
                .screenshot(
                    chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat::Png,
                )
                .await
                .map_err(|e| format!("Failed to take screenshot: {}", e))?
        } else if full_page {
            // For full page, first scroll to capture everything via JavaScript
            // Set a larger viewport to capture more content
            let _ = page.evaluate("window.scrollTo(0, 0)").await;

            // Use capture_screenshot with clip to get the full document
            let params =
                chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotParams::builder()
                    .format(
                        chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat::Png,
                    )
                    .capture_beyond_viewport(true)
                    .build();
            page.screenshot(params)
                .await
                .map_err(|e| format!("Failed to take screenshot: {}", e))?
        } else {
            // Viewport screenshot
            let params =
                chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotParams::builder()
                    .format(
                        chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat::Png,
                    )
                    .build();
            page.screenshot(params)
                .await
                .map_err(|e| format!("Failed to take screenshot: {}", e))?
        };

        // Add timestamp to filename (before extension)
        let path_obj = Path::new(path);
        let stem = path_obj
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("screenshot");
        let ext = path_obj
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("png");
        let parent = path_obj.parent().and_then(|p| p.to_str()).unwrap_or("");
        let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
        let timestamped_filename = format!("{}_{}.{}", stem, timestamp, ext);
        let timestamped_path = if parent.is_empty() {
            timestamped_filename.clone()
        } else {
            format!("{}/{}", parent, timestamped_filename)
        };

        // Save to artifacts directory
        let artifact_path = self
            .workspace_path
            .join(ARTIFACTS_DIR)
            .join(&timestamped_path);

        // Ensure parent directory exists
        if let Some(parent) = artifact_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }

        std::fs::write(&artifact_path, &screenshot_data)
            .map_err(|e| format!("Failed to save screenshot: {}", e))?;

        Ok(format!(
            "Screenshot saved to artifacts/{} ({} bytes)",
            timestamped_path,
            screenshot_data.len()
        ))
    }
}
