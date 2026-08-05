use chromiumoxide::{Browser, Page};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

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
    /// Defensive double-write — the migration owns this CREATE TABLE
    /// (see `20260517160627_consolidate_init_schema_tables.sql`). Slated
    /// for removal in `harden-init-schema-tables-vs-migrations-pattern-finish`.
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
///
/// Deliberately NOT surfaced to the LLM: the table is populated from whatever
/// the user's persistent browser profile happens to hold a session for, which
/// is unfiltered browsing data. The prompt section that listed it was removed
/// (see the note in `engine::chat::process::system_prompt`, and
/// `.claude/rules/no-private-data.md`). It exists so `browser_forget_login`
/// can act on a domain the user names explicitly.
pub struct BrowserLogins;

impl BrowserLogins {
    /// Defensive double-write — the migration owns this CREATE TABLE
    /// (see `20260517160627_consolidate_init_schema_tables.sql`). Slated
    /// for removal in `harden-init-schema-tables-vs-migrations-pattern-finish`.
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
    visible: bool,
}

/// Runtime for autonomous web browsing using headless Chromium.
/// Supports multiple concurrent browser instances, each keyed by a session ID.
/// Uses a persistent browser profile so logins and cookies carry over between sessions.
///
/// # Concurrency note — single global mutex
///
/// `instances` is wrapped in one global `tokio::sync::Mutex` for now, so
/// every browser op against any session serializes on this lock. That's
/// fine while at most a handful of triggers drive browsers concurrently
/// (typical for today's workspaces), but the moment parallel
/// browser-using triggers become common the lock becomes the bottleneck.
///
/// Planned fix: replace with `DashMap<String, Arc<Mutex<BrowserState>>>`
/// so the outer map is lock-free and only co-session ops serialize.
/// Tracked as `harden-browser-instances-sharded-locks-finish` —
/// deferred from this hardening pass because each of the ~14 call sites
/// is bespoke (interleaved with the `BrowserState` mutation it performs)
/// and the refactor wants its own dedicated PR with browser-e2e
/// validation.
pub struct BrowserRuntime {
    workspace_path: PathBuf,
    instances: Arc<Mutex<HashMap<String, BrowserState>>>,
    pool: PgPool,
}

/// Per-PID classification returned by `BrowserRuntime::is_browser_process` —
/// distinguishes a process that disappeared between `pgrep` and `ps` from one
/// that exists but isn't a Chrome/Chromium binary, so the caller can log
/// each case differently.
enum BrowserCheck {
    Chromium,
    NotChromium,
    Gone,
}

mod actions;
mod session;
