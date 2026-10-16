use super::*;
use chromiumoxide::browser::BrowserConfigBuilder;
use chromiumoxide::error::CdpError;
use chromiumoxide::BrowserConfig;
use futures::StreamExt;
use std::path::Path;
use std::time::Duration;

/// Why a browser launch failed, as far as the caller's retry cares.
#[derive(Debug)]
enum LaunchFailure {
    /// Chrome cannot start its sandbox on this host. That never clears on its
    /// own, so it is never retried (ADR 0457).
    SandboxUnavailable(SandboxBlocker),
    /// Chrome quit, timed out or closed its pipe before it answered. A stale
    /// profile lock or a leftover Chrome is the usual cause, so cleaning up and
    /// launching once more can help.
    BeforeReady(String),
    Other(String),
}

impl LaunchFailure {
    /// Reads Chrome's stderr before the variant, because a Chrome with no
    /// sandbox also quits before it is ready.
    fn from_launch_error(e: CdpError) -> Self {
        let stderr = match &e {
            CdpError::LaunchExit(_, stderr)
            | CdpError::LaunchTimeout(stderr)
            | CdpError::LaunchIo(_, stderr) => stderr.as_slice(),
            _ => &[],
        };
        if let Some(blocker) = SandboxBlocker::in_chrome_stderr(stderr) {
            log!("[Browser] Chrome could not start its sandbox: {}", e);
            return Self::SandboxUnavailable(blocker);
        }
        let message = format!("Failed to launch browser: {}", e);
        match e {
            CdpError::LaunchExit(..) | CdpError::LaunchIo(..) | CdpError::LaunchTimeout(..) => {
                Self::BeforeReady(message)
            }
            _ => Self::Other(message),
        }
    }
}

impl std::fmt::Display for LaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforeReady(message) | Self::Other(message) => f.write_str(message),
            Self::SandboxUnavailable(blocker) => write!(
                f,
                "The agent browser did not start. Lucidos runs Chrome only inside its sandbox, and {}",
                blocker.reason_and_fix()
            ),
        }
    }
}

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
            Err(LaunchFailure::BeforeReady(e)) => {
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
            Err(e) => Err(e.to_string()),
        }
    }

    /// Launch Chrome and return the Browser + handler task.
    /// When `visible` is true, Chrome opens with a GUI window the user can watch.
    /// Uses a persistent profile directory so cookies/sessions carry over.
    ///
    /// The vendored chromiumoxide speaks CDP over a pipe, so Chrome opens no
    /// debugging port another local user could drive. The profile holds every
    /// cookie, so its directory is this user's alone, on every launch.
    ///
    /// Chrome always runs inside its sandbox: the vendored crate cannot turn it
    /// off. A host that cannot give Chrome one gets no browser (ADR 0457).
    async fn launch_browser(
        visible: bool,
        profile_dir: &Path,
    ) -> Result<(Browser, JoinHandle<()>), LaunchFailure> {
        Self::launch_with(Self::browser_config(visible, profile_dir), profile_dir).await
    }

    async fn launch_with(
        builder: BrowserConfigBuilder,
        profile_dir: &Path,
    ) -> Result<(Browser, JoinHandle<()>), LaunchFailure> {
        use std::os::unix::fs::PermissionsExt;
        #[cfg(target_os = "linux")]
        // SAFETY: geteuid only reads this process's effective user id.
        if unsafe { libc::geteuid() } == 0 {
            return Err(LaunchFailure::SandboxUnavailable(
                SandboxBlocker::RunningAsRoot,
            ));
        }
        std::fs::create_dir_all(profile_dir)
            .and_then(|()| {
                std::fs::set_permissions(profile_dir, std::fs::Permissions::from_mode(0o700))
            })
            .map_err(|e| {
                LaunchFailure::Other(format!("Failed to create browser profile dir: {}", e))
            })?;

        let config = builder
            .build()
            .map_err(|e| LaunchFailure::Other(format!("Failed to build browser config: {}", e)))?;

        let (browser, mut handler) = Browser::launch(config)
            .await
            .map_err(LaunchFailure::from_launch_error)?;

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

    fn browser_config(visible: bool, profile_dir: &Path) -> BrowserConfigBuilder {
        let builder = BrowserConfig::builder()
            .launch_timeout(Self::CHROME_LAUNCH_TIMEOUT)
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
            builder.with_head()
        } else {
            // Stealth headless: evade common bot detection
            builder
                .arg("--disable-blink-features=AutomationControlled")
                .arg("--disable-features=AutomationControlled")
                .arg("--disable-infobars")
        }
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

    /// How long Chrome may take to answer on its pipe after it starts. A cold
    /// start on a busy host can outlast chromiumoxide's default.
    const CHROME_LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);

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

    /// Delete all browser data: profile directory, login records and the
    /// headless blocklist. Closes any running browsers first.
    pub async fn clear_data(&self) -> Result<String, String> {
        self.close_all().await?;

        let profile_dir = self.workspace_path.join(".lucidos/browser-profile");
        if profile_dir.exists() {
            std::fs::remove_dir_all(&profile_dir)
                .map_err(|e| format!("Failed to delete browser profile: {}", e))?;
        }

        // Propagate rather than log-and-claim-success: the message below states
        // logins were cleared, so a failed clear must not report as done.
        BrowserLogins::clear(&self.pool)
            .await
            .map_err(|e| format!("Failed to clear browser logins: {}", e))?;

        // The only way to undo a headless block. Nothing else removes a row,
        // so a false positive would otherwise outlive the profile it came from.
        HeadlessBlocklist::clear(&self.pool)
            .await
            .map_err(|e| format!("Failed to clear headless blocklist: {}", e))?;

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

/// What keeps Chrome from starting its sandbox on this host.
#[derive(Debug, PartialEq)]
enum SandboxBlocker {
    RunningAsRoot,
    NoUsableSandbox,
}

impl SandboxBlocker {
    /// Chrome's own words when it aborts for want of a sandbox. Its message
    /// also suggests `--no-sandbox`, so it reaches the engine log only.
    fn in_chrome_stderr(stderr: &[u8]) -> Option<Self> {
        let stderr = String::from_utf8_lossy(stderr);
        if stderr.contains("No usable sandbox") {
            Some(Self::NoUsableSandbox)
        } else if stderr.contains("Running as root without --no-sandbox") {
            Some(Self::RunningAsRoot)
        } else {
            None
        }
    }

    fn reason_and_fix(&self) -> &'static str {
        match self {
            Self::RunningAsRoot => {
                "Chrome refuses its sandbox to the root user. Run Lucidos as a non-root user."
            }
            Self::NoUsableSandbox => {
                "Chrome found no usable sandbox on this host. Install Google Chrome from its \
                 official package, whose setuid helper provides one. Or allow unprivileged user \
                 namespaces for the Chrome in use: on Ubuntu 23.10 and later, an AppArmor \
                 profile granting it `userns`."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Chrome's abort when user namespaces are restricted and no setuid helper
    /// exists, as Chrome prints it.
    const NO_USABLE_SANDBOX: &str = "[1:1:FATAL:zygote_host_impl_linux.cc(128)] No usable sandbox! \
        If you are running on Ubuntu 23.10+ or another Linux distro that has disabled unprivileged \
        user namespaces with AppArmor, see https://chromium.googlesource.com/chromium/src/+/main/docs/security/apparmor-userns-restrictions.md. \
        If you want to live dangerously and need an immediate workaround, you can try using --no-sandbox.";
    const RUNNING_AS_ROOT: &str = "[1:1:ERROR:zygote_host_impl_linux.cc(100)] Running as root \
        without --no-sandbox is not supported. See https://crbug.com/638180.";

    #[test]
    fn chrome_without_a_sandbox_is_a_sandbox_failure_never_retried() {
        use chromiumoxide::error::BrowserStderr;
        use std::os::unix::process::ExitStatusExt;
        let stderr = |text: &str| BrowserStderr::new(text.as_bytes().to_vec());
        let quit = std::process::ExitStatus::from_raw(1 << 8);

        for (launch_error, blocker) in [
            (
                CdpError::LaunchExit(quit, stderr(NO_USABLE_SANDBOX)),
                SandboxBlocker::NoUsableSandbox,
            ),
            (
                CdpError::LaunchTimeout(stderr(RUNNING_AS_ROOT)),
                SandboxBlocker::RunningAsRoot,
            ),
        ] {
            let failure = LaunchFailure::from_launch_error(launch_error);
            let message = failure.to_string();
            assert!(
                matches!(&failure, LaunchFailure::SandboxUnavailable(b) if *b == blocker),
                "{failure:?} must be a sandbox failure, which is never retried"
            );
            assert!(
                !message.contains("no-sandbox"),
                "the fix must never be to drop the sandbox: {message}"
            );
        }
        assert!(matches!(
            LaunchFailure::from_launch_error(CdpError::LaunchExit(quit, stderr("ordinary"))),
            LaunchFailure::BeforeReady(_)
        ));
    }

    /// Every switch that turns the sandbox off is refused before a process
    /// starts, so no caller can launch an unsandboxed Chrome by passing one.
    #[test]
    fn chrome_refuses_every_sandbox_weakening_switch() {
        for switch in chromiumoxide::browser::SANDBOX_WEAKENING_SWITCHES {
            let single_dash = switch.strip_prefix('-').expect("a switch starts with --");
            for form in [
                switch.to_string(),
                format!("{switch}=1"),
                single_dash.to_string(),
                format!("{single_dash}=1"),
            ] {
                let config = BrowserConfig::builder()
                    .chrome_executable("/usr/bin/true")
                    .arg(&form)
                    .build()
                    .expect("a config");
                let refused = config.launch().expect_err(&form);
                assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput, "{form}");
            }
        }
    }

    /// A cold Chrome on a busy host can need longer than chromiumoxide's
    /// default to answer, so a slow start must still launch. The fake Chrome
    /// answers the readiness probe on its pipe just after that default.
    #[tokio::test]
    async fn a_chrome_slower_than_the_library_default_still_launches() {
        #[cfg(target_os = "linux")]
        // SAFETY: geteuid takes no arguments and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            // Linux root never launches Chrome, so no launch can be slow.
            return;
        }
        let workspace = tempfile::tempdir().expect("tempdir");
        let answers_after =
            Duration::from_millis(chromiumoxide::browser::LAUNCH_TIMEOUT) + Duration::from_secs(2);
        let fake_chrome = workspace.path().join("slow-chrome");
        std::fs::write(
            &fake_chrome,
            format!(
                r#"#!/usr/bin/env bash
sleep {}
IFS= read -r -d '' probe <&3
[[ $probe =~ \"id\":([0-9]+) ]]
printf '{{"id":%s,"result":{{}}}}\0' "${{BASH_REMATCH[1]}}" >&4
exec sleep 600
"#,
                answers_after.as_secs()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake_chrome, std::fs::Permissions::from_mode(0o755)).unwrap();
        let profile = workspace.path().join("browser-profile");

        let (mut browser, handler_task) = BrowserRuntime::launch_with(
            BrowserRuntime::browser_config(false, &profile).chrome_executable(&fake_chrome),
            &profile,
        )
        .await
        .expect("a slow Chrome still launches");

        handler_task.abort();
        browser.kill().await;
    }

    /// The agent browser speaks CDP over a pipe, so no other local user can
    /// reach it through a port. Its profile is this user's alone. Needs a
    /// Chrome on the machine, and passes as a no-op without one.
    #[tokio::test]
    async fn the_agent_browser_listens_on_no_tcp_port() {
        if BrowserConfig::builder().build().is_err() {
            return;
        }
        let workspace = tempfile::tempdir().expect("tempdir");
        let profile = workspace.path().join("browser-profile");
        std::fs::create_dir(&profile).expect("an older 0755 profile");
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o755)).unwrap();

        let (mut browser, handler_task) = BrowserRuntime::launch_browser(false, &profile)
            .await
            .expect("launch Chrome");
        let page = browser
            .new_page("data:text/html,<title>piped</title>")
            .await
            .expect("drive Chrome over the pipe");
        assert_eq!(page.get_title().await.unwrap().as_deref(), Some("piped"));

        let pid = browser
            .get_mut_child()
            .expect("a launched child")
            .inner
            .id();
        let listening = std::process::Command::new("lsof")
            .args(["-a", "-p", &pid.to_string(), "-iTCP", "-sTCP:LISTEN", "-Fn"])
            .output()
            .expect("run lsof");
        assert!(
            listening.stdout.is_empty(),
            "Chrome listens on TCP: {}",
            String::from_utf8_lossy(&listening.stdout)
        );
        let mode = std::fs::metadata(&profile).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "the profile directory must be the user's alone"
        );

        browser.close().await.expect("close Chrome");
        handler_task.abort();
    }

    /// A renderer runs untrusted page code, so the OS must confine every one
    /// of them. Asks the kernel rather than reading Chrome's argv. Needs a
    /// Chrome on the machine, and passes as a no-op without one.
    #[tokio::test]
    async fn the_agent_browser_renderers_are_sandboxed() {
        if BrowserConfig::builder().build().is_err() {
            return;
        }
        let workspace = tempfile::tempdir().expect("tempdir");
        let (mut browser, handler_task) =
            BrowserRuntime::launch_browser(false, &workspace.path().join("browser-profile"))
                .await
                .expect("launch Chrome");
        browser
            .new_page("data:text/html,<title>untrusted</title>")
            .await
            .expect("open a page");

        let pid = browser
            .get_mut_child()
            .expect("a launched child")
            .inner
            .id();
        let renderers = renderer_pids(pid);
        assert!(!renderers.is_empty(), "Chrome started no renderer");
        for renderer in renderers {
            assert!(
                runs_in_os_sandbox(pid, renderer),
                "renderer {renderer} runs outside the OS sandbox"
            );
        }

        browser.close().await.expect("close Chrome");
        handler_task.abort();
    }

    /// Every live descendant of `root` whose argv says `--type=renderer`.
    fn renderer_pids(root: u32) -> Vec<u32> {
        let ps = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,args="])
            .output()
            .expect("run ps");
        let rows: Vec<(u32, u32, String)> = String::from_utf8_lossy(&ps.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?.parse().ok()?;
                let ppid = fields.next()?.parse().ok()?;
                Some((pid, ppid, fields.collect::<Vec<_>>().join(" ")))
            })
            .collect();
        let mut tree = vec![root];
        let mut renderers = Vec::new();
        while let Some(parent) = tree.pop() {
            for (pid, _, args) in rows.iter().filter(|(_, ppid, _)| *ppid == parent) {
                tree.push(*pid);
                if args.contains("--type=renderer") {
                    renderers.push(*pid);
                }
            }
        }
        renderers
    }

    /// macOS: the kernel's Seatbelt answer. A pid that no longer exists also
    /// reads as sandboxed, so liveness is checked first.
    #[cfg(target_os = "macos")]
    fn runs_in_os_sandbox(_browser: u32, renderer: u32) -> bool {
        extern "C" {
            fn sandbox_check(pid: libc::pid_t, operation: *const libc::c_char, kind: i32) -> i32;
        }
        let pid = renderer as libc::pid_t;
        // SAFETY: both calls only read kernel state for `pid`; a null
        // operation asks whether the process is sandboxed at all.
        unsafe { libc::kill(pid, 0) == 0 && sandbox_check(pid, std::ptr::null(), 0) == 1 }
    }

    /// Linux: Chrome's two layers, a seccomp-BPF filter and a PID namespace of
    /// its own (user namespaces or the setuid helper).
    #[cfg(target_os = "linux")]
    fn runs_in_os_sandbox(browser: u32, renderer: u32) -> bool {
        let status =
            std::fs::read_to_string(format!("/proc/{renderer}/status")).unwrap_or_default();
        let filtered = status
            .lines()
            .any(|line| line.split_whitespace().eq(["Seccomp:", "2"]));
        let pid_ns = |pid: u32| std::fs::read_link(format!("/proc/{pid}/ns/pid")).ok();
        filtered && pid_ns(renderer).is_some() && pid_ns(renderer) != pid_ns(browser)
    }
}
