use crate::core::ARTIFACTS_DIR;
use chromiumoxide::{Browser, BrowserConfig, Page};
use chrono::Local;
use futures::StreamExt;
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Extract the host/domain from a URL using simple string parsing.
fn extract_domain(url: &str) -> Option<String> {
    // Find the start after the scheme ("://")
    let after_scheme = url.find("://").map(|i| i + 3).unwrap_or(0);
    let rest = &url[after_scheme..];
    // Take everything before the first '/' or '?' or '#'
    let host = rest.split(&['/', '?', '#'][..]).next().unwrap_or(rest);
    // Strip port
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

/// Detect common bot-blocking patterns in page content.
/// Returns a human-readable reason if the page looks like a bot challenge.
fn detect_bot_block(content: &str) -> Option<String> {
    let lower = content.to_lowercase();
    let checks: &[(&[&str], &str)] = &[
        (
            &["just a moment", "enable javascript and cookies", "cf-"],
            "Cloudflare challenge",
        ),
        (
            &["attention required", "cloudflare"],
            "Cloudflare challenge",
        ),
        (&["verify you are human", "captcha"], "CAPTCHA verification"),
        (&["access denied", "automated"], "bot detection"),
    ];
    for (markers, reason) in checks {
        if markers.iter().all(|m| lower.contains(m)) {
            return Some(reason.to_string());
        }
    }
    // Suspiciously short page with block-like words
    if content.len() < 200 {
        for word in ["blocked", "denied", "forbidden", "captcha"] {
            if lower.contains(word) {
                return Some("bot detection".to_string());
            }
        }
    }
    None
}

/// Tracks domains that block headless browsers so we can fast-fail
/// instead of wasting LLM iterations on repeated retries.
pub struct HeadlessBlocklist;

impl HeadlessBlocklist {
    pub async fn init_schema(
        pool: &PgPool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS headless_blocked (
                domain TEXT PRIMARY KEY,
                reason TEXT NOT NULL,
                created_at TIMESTAMPTZ DEFAULT NOW()
            )",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn is_blocked(pool: &PgPool, domain: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT reason FROM headless_blocked WHERE domain = $1")
                .bind(domain)
                .fetch_optional(pool)
                .await?;
        Ok(row.map(|r| r.0))
    }

    pub async fn block(pool: &PgPool, domain: &str, reason: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO headless_blocked (domain, reason) VALUES ($1, $2)
             ON CONFLICT (domain) DO UPDATE SET reason = $2, created_at = NOW()",
        )
        .bind(domain)
        .bind(reason)
        .execute(pool)
        .await?;
        Ok(())
    }
}

/// Tracks sites the user has logged into via the persistent browser profile.
/// Included in the system prompt so the LLM knows which sites have active sessions.
pub struct BrowserLogins;

impl BrowserLogins {
    pub async fn init_schema(
        pool: &PgPool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS browser_logins (
                domain TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                logged_in_at TIMESTAMPTZ DEFAULT NOW()
            )",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn record(pool: &PgPool, domain: &str, label: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO browser_logins (domain, label) VALUES ($1, $2)
             ON CONFLICT (domain) DO UPDATE SET label = $2, logged_in_at = NOW()",
        )
        .bind(domain)
        .bind(label)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn remove(pool: &PgPool, domain: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM browser_logins WHERE domain = $1")
            .bind(domain)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as("SELECT domain, label FROM browser_logins ORDER BY domain")
            .fetch_all(pool)
            .await
    }

    pub async fn clear(pool: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM browser_logins")
            .execute(pool)
            .await?;
        Ok(())
    }
}

/// Internal state for an active browser session
struct BrowserState {
    browser: Browser,
    current_page: Option<Page>,
    handler_task: JoinHandle<()>,
    session_id: Uuid,
    visible: bool,
}

/// Runtime for autonomous web browsing using headless Chromium.
/// Supports multiple concurrent browser instances, each keyed by a session ID.
/// Uses a persistent browser profile so logins and cookies carry over between sessions.
pub struct BrowserRuntime {
    workspace_path: PathBuf,
    instances: Arc<Mutex<HashMap<String, BrowserState>>>,
    pool: Option<PgPool>,
}

impl BrowserRuntime {
    pub fn new(workspace_path: PathBuf, pool: PgPool) -> Self {
        Self {
            workspace_path,
            instances: Arc::new(Mutex::new(HashMap::new())),
            pool: Some(pool),
        }
    }

    /// Ensure a browser instance exists for the given session key, launch if needed.
    /// If visibility mode differs from the existing browser, close and relaunch.
    async fn ensure_browser(&self, session_key: &str, visible: bool) -> Result<(), String> {
        let mut instances = self.instances.lock().await;
        let profile_dir = self.workspace_path.join(".lucidos/browser-profile");

        // Check if this session already has a browser
        if let Some(state) = instances.get(session_key) {
            if !state.handler_task.is_finished() {
                if state.visible == visible {
                    // Browser is running with matching visibility
                    return Ok(());
                }
                // Visibility mismatch — scan logins if headful, then close
                log!(
                    "[Browser] Browser visibility mismatch (was {}, need {}), relaunching",
                    state.visible,
                    visible
                );
            }
            // Close existing browser (scan logins if it was headful)
            if let Some(state) = instances.remove(session_key) {
                if state.visible {
                    self.scan_and_record_logins_inner(&state.browser, state.current_page.as_ref())
                        .await;
                }
                state.handler_task.abort();
            }
        }

        // Chrome profile lock: only one browser instance can use a profile dir.
        // Close ALL other sessions' browsers before launching.
        let other_keys: Vec<String> = instances.keys().cloned().collect();
        for key in other_keys {
            if let Some(state) = instances.remove(&key) {
                if state.visible {
                    self.scan_and_record_logins_inner(&state.browser, state.current_page.as_ref())
                        .await;
                }
                state.handler_task.abort();
                log!("[Browser] Closed browser for session {} (profile lock)", key);
            }
        }

        match Self::launch_browser(visible, &profile_dir).await {
            Ok((browser, handler_task)) => {
                instances.insert(
                    session_key.to_string(),
                    BrowserState {
                        browser,
                        current_page: None,
                        handler_task,
                        session_id: Uuid::new_v4(),
                        visible,
                    },
                );
                Ok(())
            }
            Err(e)
                if e.contains("SingletonLock")
                    || e.contains("File exists")
                    || e.contains("Timeout")
                    || e.contains("websocket") =>
            {
                // Stale state from a previous crash — clean up and retry
                log!("[Browser] Launch failed ({}), cleaning up and retrying...", e);
                Self::cleanup_stale_profile(&profile_dir);
                Self::kill_zombie_browsers(&profile_dir);
                let (browser, handler_task) = Self::launch_browser(visible, &profile_dir)
                    .await
                    .map_err(|e2| format!("Failed to launch browser after cleanup: {}", e2))?;
                instances.insert(
                    session_key.to_string(),
                    BrowserState {
                        browser,
                        current_page: None,
                        handler_task,
                        session_id: Uuid::new_v4(),
                        visible,
                    },
                );
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Launch Chrome and return the Browser + handler task.
    /// When `visible` is true, Chrome opens with a GUI window the user can watch.
    /// Uses a persistent profile directory so cookies/sessions carry over.
    async fn launch_browser(
        visible: bool,
        profile_dir: &Path,
    ) -> Result<(Browser, JoinHandle<()>), String> {
        std::fs::create_dir_all(profile_dir)
            .map_err(|e| format!("Failed to create browser profile dir: {}", e))?;

        let mut builder = BrowserConfig::builder()
            .no_sandbox()
            .user_data_dir(profile_dir)
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .window_size(2560, 1600)
            .viewport(chromiumoxide::handler::viewport::Viewport {
                width: 2560,
                height: 1600,
                device_scale_factor: Some(1.0),
                emulating_mobile: false,
                is_landscape: true,
                has_touch: false,
            });

        if visible {
            builder = builder.with_head();
        } else {
            // Stealth headless: evade common bot detection
            builder = builder
                .arg("--disable-blink-features=AutomationControlled")
                .arg("--disable-features=AutomationControlled")
                .arg("--disable-infobars");
        }

        let config = builder
            .build()
            .map_err(|e| format!("Failed to build browser config: {}", e))?;

        let (browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|e| format!("Failed to launch browser: {}", e))?;

        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    let err_msg = e.to_string();
                    if !err_msg.contains("untagged enum Message") {
                        log!("[Browser] Handler error: {}", e);
                    }
                }
            }
        });

        Ok((browser, handler_task))
    }

    /// Remove stale SingletonLock and other lock files from the persistent
    /// browser profile directory. Called when a previous engine crash left Chrome zombies.
    fn cleanup_stale_profile(profile_dir: &Path) {
        if profile_dir.exists() {
            for name in &["SingletonLock", "SingletonSocket", "SingletonCookie"] {
                let p = profile_dir.join(name);
                if p.exists() {
                    log!("[Browser] Removing stale {}", p.display());
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }

    /// Kill orphaned Chrome/Chromium processes that were using *this workspace's*
    /// browser profile. Scoping to the absolute profile path prevents us from
    /// killing browsers belonging to other concurrently-running workspaces (which
    /// all share the literal substring "browser-profile" in their profile dir).
    fn kill_zombie_browsers(profile_dir: &Path) {
        let profile_marker = profile_dir.to_string_lossy();
        match std::process::Command::new("pkill")
            .args(["-f", profile_marker.as_ref()])
            .output()
        {
            Ok(output) => {
                if output.status.success() {
                    log!(
                        "[Browser] Killed zombie browser processes for {}",
                        profile_marker
                    );
                    // Brief pause for OS to release resources
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
            Err(_) => {
                // pkill not available (e.g. Windows) — best-effort
            }
        }
    }

    /// Timeout for individual browser operations (page load, navigation, etc.)
    const BROWSER_OP_TIMEOUT: Duration = Duration::from_secs(30);

    /// Force-kill a specific browser instance so the next call will relaunch.
    /// Caller must NOT hold the instances lock.
    async fn force_kill_browser(&self, session_key: &str) {
        let mut instances = self.instances.lock().await;
        if let Some(s) = instances.remove(session_key) {
            s.handler_task.abort();
        }
        log!(
            "[Browser] Force-killed browser for session {} due to connection failure",
            session_key
        );
    }

    /// Run a browser operation with a timeout. On timeout or connection error,
    /// force-kills the browser so the next call relaunches cleanly.
    async fn with_timeout<F, Fut>(
        &self,
        session_key: &str,
        op_name: &str,
        f: F,
    ) -> Result<String, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<String, String>>,
    {
        match tokio::time::timeout(Self::BROWSER_OP_TIMEOUT, f()).await {
            Ok(result) => {
                if let Err(ref e) = result {
                    if e.contains("closed connection") || e.contains("channel closed") {
                        self.force_kill_browser(session_key).await;
                    }
                }
                result
            }
            Err(_) => {
                self.force_kill_browser(session_key).await;
                Err(format!(
                    "Browser {} timed out after {}s",
                    op_name,
                    Self::BROWSER_OP_TIMEOUT.as_secs()
                ))
            }
        }
    }

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
            if let (Some(ref pool), Some(ref domain)) = (&self.pool, &domain) {
                if let Ok(Some(reason)) = HeadlessBlocklist::is_blocked(pool, domain).await {
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
                    if let (Some(ref pool), Some(ref domain)) = (&self.pool, &domain) {
                        if let Err(e) = HeadlessBlocklist::block(pool, domain, &reason).await {
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
        super::browser_consent::dismiss_cookie_consent(&page).await;

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
        super::browser_consent::dismiss_cookie_consent(page).await;

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

    /// Close a specific browser session.
    /// If the session was headful, scans cookies/localStorage for auth tokens before closing.
    pub async fn close(&self, session_key: &str) -> Result<String, String> {
        let mut instances = self.instances.lock().await;

        if let Some(browser_state) = instances.remove(session_key) {
            if browser_state.visible {
                self.scan_and_record_logins_inner(
                    &browser_state.browser,
                    browser_state.current_page.as_ref(),
                )
                .await;
            }
            browser_state.handler_task.abort();

            Ok(format!(
                "Browser session {} closed",
                browser_state.session_id
            ))
        } else {
            Ok("No browser session to close".to_string())
        }
    }

    /// Close all browser instances (for shutdown)
    pub async fn close_all(&self) -> Result<String, String> {
        let mut instances = self.instances.lock().await;
        let count = instances.len();

        for (_, state) in instances.drain() {
            if state.visible {
                self.scan_and_record_logins_inner(&state.browser, state.current_page.as_ref())
                    .await;
            }
            state.handler_task.abort();
        }

        Ok(format!("Closed {} browser instance(s)", count))
    }

    /// Delete all browser data: profile directory and login records.
    /// Closes any running browsers first.
    pub async fn clear_data(&self) -> Result<String, String> {
        self.close_all().await?;

        let profile_dir = self.workspace_path.join(".lucidos/browser-profile");
        if profile_dir.exists() {
            std::fs::remove_dir_all(&profile_dir)
                .map_err(|e| format!("Failed to delete browser profile: {}", e))?;
        }

        if let Some(ref pool) = self.pool {
            if let Err(e) = BrowserLogins::clear(pool).await {
                log!("[Browser] Failed to clear browser_logins table: {}", e);
            }
        }

        Ok("All browser data cleared (cookies, logins, localStorage, cache)".to_string())
    }

    /// Scan cookies and localStorage for auth tokens after a headful session closes.
    async fn scan_and_record_logins_inner(&self, browser: &Browser, current_page: Option<&Page>) {
        let pool = match &self.pool {
            Some(p) => p,
            None => return,
        };

        // Get all cookies via the Browser's built-in method
        let cookies = match browser.get_cookies().await {
            Ok(c) => c,
            Err(e) => {
                log!("[Browser] Failed to scan cookies: {}", e);
                return;
            }
        };

        // Group HttpOnly cookies by domain, skip known analytics/tracking domains
        let skip_domains = [
            "google-analytics.com",
            "doubleclick.net",
            "facebook.com/tr",
            "analytics.",
            "tracking.",
            ".cdn.",
        ];
        let mut auth_domains: HashMap<String, bool> = HashMap::new();
        for cookie in &cookies {
            if !cookie.http_only {
                continue;
            }
            let domain = cookie.domain.trim_start_matches('.');
            if skip_domains.iter().any(|s| domain.contains(s)) {
                continue;
            }
            auth_domains.insert(domain.to_string(), true);
        }

        for domain in auth_domains.keys() {
            if let Err(e) = BrowserLogins::record(pool, domain, domain).await {
                log!("[Browser] Failed to record browser login for {}: {}", domain, e);
            }
        }

        if !auth_domains.is_empty() {
            log!(
                "[Browser] Recorded {} probable login domains from cookies",
                auth_domains.len()
            );
        }

        // Also check localStorage on the current page for JWT tokens
        if let Some(page) = current_page {
            if let Some(domain) = Self::scan_localstorage_for_auth(page).await {
                if let Err(e) = BrowserLogins::record(pool, &domain, &domain).await {
                    log!(
                        "[Browser] Failed to record login from localStorage for {}: {}",
                        domain,
                        e
                    );
                } else {
                    log!("[Browser] Recorded login from localStorage: {}", domain);
                }
            }
        }
    }

    /// Check localStorage on the current page for JWT tokens or auth-related keys.
    async fn scan_localstorage_for_auth(page: &Page) -> Option<String> {
        let js = r#"
        (function() {
            try {
                for (let i = 0; i < localStorage.length; i++) {
                    const key = localStorage.key(i);
                    const value = localStorage.getItem(key);
                    const k = key.toLowerCase();
                    if (value && value.startsWith('eyJ')) return window.location.hostname;
                    if (['token', 'jwt', 'auth', 'access_token', 'id_token', 'refresh_token', 'session']
                        .some(t => k.includes(t))) return window.location.hostname;
                }
            } catch(e) {}
            return null;
        })()
        "#;
        match page.evaluate(js).await {
            Ok(result) => result.into_value::<Option<String>>().ok().flatten(),
            Err(_) => None,
        }
    }

}
