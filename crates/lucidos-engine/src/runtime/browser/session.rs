use super::*;
use chromiumoxide::BrowserConfig;
use futures::StreamExt;
use std::path::Path;
use std::time::Duration;

impl BrowserRuntime {
    pub fn new(workspace_path: PathBuf, pool: PgPool) -> Self {
        Self {
            workspace_path,
            instances: Arc::new(Mutex::new(HashMap::new())),
            pool,
        }
    }

    /// Ensure a browser instance exists for the given session key, launch if needed.
    /// If visibility mode differs from the existing browser, close and relaunch.
    pub(super) async fn ensure_browser(
        &self,
        session_key: &str,
        visible: bool,
    ) -> Result<(), String> {
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
                log!(
                    "[Browser] Closed browser for session {} (profile lock)",
                    key
                );
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
                log!(
                    "[Browser] Launch failed ({}), cleaning up and retrying...",
                    e
                );
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
    ///
    /// Over-match defence: a bare `pkill -f <profile_path>` would also match any
    /// long-running process that happens to have the profile path in its argv
    /// (e.g. `cargo build` or an editor referencing the workspace tree). We
    /// instead `pgrep -f` to get PIDs, validate each via `ps -p PID -o comm=`
    /// that the process command name is actually `chrome` / `chromium` /
    /// `Chrome` / `Chromium` / `Google Chrome Helper`, and only then `kill`.
    /// `pgrep` not available (Windows) → best-effort no-op, same as before.
    fn kill_zombie_browsers(profile_dir: &Path) {
        let profile_marker = profile_dir.to_string_lossy();
        let pgrep_out = match std::process::Command::new("pgrep")
            .args(["-f", profile_marker.as_ref()])
            .output()
        {
            Ok(o) => o,
            Err(_) => return, // pgrep not available (e.g. Windows)
        };
        if !pgrep_out.status.success() {
            return; // exit 1 = no matches
        }

        let stdout = String::from_utf8_lossy(&pgrep_out.stdout);
        let mut killed = 0usize;
        for pid_str in stdout.split_whitespace() {
            // PID validation: skip anything non-numeric, and skip our own PID
            // (pgrep -f can match ourselves if our argv carries the path).
            let Ok(pid) = pid_str.parse::<u32>() else {
                continue;
            };
            if pid == std::process::id() {
                continue;
            }
            match Self::is_browser_process(pid_str) {
                BrowserCheck::Chromium => {}
                BrowserCheck::NotChromium => {
                    log!(
                        "[Browser] Skipping zombie kill for pid {} — not a Chrome/Chromium process",
                        pid_str
                    );
                    continue;
                }
                BrowserCheck::Gone => {
                    // PID exited between pgrep and ps — nothing to kill.
                    continue;
                }
            }
            if std::process::Command::new("kill")
                .arg(pid_str)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                killed += 1;
            }
        }

        if killed > 0 {
            log!(
                "[Browser] Killed {} zombie browser process(es) for {}",
                killed,
                profile_marker
            );
            // Brief pause for OS to release resources
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// `ps -p PID -o comm=` → process command name. macOS prints the full
    /// executable path (e.g. `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`);
    /// Linux prints the basename. Either way the substring check for
    /// `chrome` / `chromium` (case-insensitive) covers the main process and
    /// its helpers (`Google Chrome Helper`, `chromium-browser`,
    /// `chrome_crashpad_handler`, …). Anything else (an editor, build, etc.
    /// that happens to carry the profile path in its argv) is skipped.
    ///
    /// Returns `Gone` when `ps` fails for `pid` — typically because the
    /// process exited between `pgrep` and `ps` — so the caller can avoid
    /// logging a misleading "not a Chrome process" message for a PID that
    /// simply isn't there anymore.
    fn is_browser_process(pid: &str) -> BrowserCheck {
        let Ok(out) = std::process::Command::new("ps")
            .args(["-p", pid, "-o", "comm="])
            .output()
        else {
            return BrowserCheck::Gone;
        };
        if !out.status.success() {
            return BrowserCheck::Gone;
        }
        let name = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
        if name.contains("chrome") || name.contains("chromium") {
            BrowserCheck::Chromium
        } else {
            BrowserCheck::NotChromium
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
    pub(super) async fn with_timeout<F, Fut>(
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

            Ok(format!("Browser session {} closed", session_key))
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

        if let Err(e) = BrowserLogins::clear(&self.pool).await {
            log!("[Browser] Failed to clear browser_logins table: {}", e);
        }

        Ok("All browser data cleared (cookies, logins, localStorage, cache)".to_string())
    }

    /// Scan cookies and localStorage for auth tokens after a headful session closes.
    async fn scan_and_record_logins_inner(&self, browser: &Browser, current_page: Option<&Page>) {
        let pool = &self.pool;

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
        let mut auth_domains: std::collections::HashSet<String> = std::collections::HashSet::new();
        for cookie in &cookies {
            if !cookie.http_only {
                continue;
            }
            let domain = cookie.domain.trim_start_matches('.');
            if skip_domains.iter().any(|s| domain.contains(s)) {
                continue;
            }
            auth_domains.insert(domain.to_string());
        }

        for domain in &auth_domains {
            if let Err(e) = BrowserLogins::record(pool, domain, domain).await {
                log!(
                    "[Browser] Failed to record browser login for {}: {}",
                    domain,
                    e
                );
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
