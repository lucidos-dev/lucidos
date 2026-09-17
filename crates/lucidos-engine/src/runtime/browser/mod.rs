use chromiumoxide::{Browser, Page};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Extract the host/domain from a URL using simple string parsing.
fn extract_domain(url: &str) -> Option<String> {
    // Drop the scheme prefix ("scheme://"), keeping the whole string when absent.
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
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
///
/// Every signal needs ALL of its markers. One match alone is too cheap: a hit
/// blocklists the whole DOMAIN for every later headless open. A short page
/// carrying one block-like word used to count, and a plain nginx 403 body met
/// that bar.
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
        // Cloudflare's firewall block page, which says neither "attention
        // required" nor "automated". The error code is what keeps this pair
        // off an ordinary 403.
        (&["error 1020", "cloudflare"], "Cloudflare firewall rule"),
    ];
    for (markers, reason) in checks {
        if markers.iter().all(|m| lower.contains(m)) {
            return Some(reason.to_string());
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

    /// Forget every blocked domain, so `browser_open` retries them headless.
    /// A block is a guess, and the user's escape hatch is clearing browser data.
    pub async fn clear(pool: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM headless_blocked")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain 4xx page is short and says "forbidden", and it is no bot wall.
    /// One such page used to blocklist the whole domain for every headless
    /// open, in every thread and every trigger, with nothing able to undo it.
    #[test]
    fn short_ordinary_error_pages_are_not_bot_detection() {
        for body in [
            "403 Forbidden\nnginx/1.18.0",
            "Access Denied",
            "Your account is blocked. Contact support.",
            "{\"captcha_required\": false}",
        ] {
            assert_eq!(detect_bot_block(body), None, "body: {body}");
        }
    }

    /// Fixtures built from the markers each surviving branch requires, so the
    /// contract under test is the function's own.
    #[test]
    fn genuine_bot_walls_are_still_detected() {
        let cloudflare = "Just a moment...\nEnable JavaScript and cookies to continue\
                          <div class=\"cf-browser-verification\"></div>";
        assert_eq!(
            detect_bot_block(cloudflare).as_deref(),
            Some("Cloudflare challenge")
        );
        assert_eq!(
            detect_bot_block("Attention Required! | Cloudflare").as_deref(),
            Some("Cloudflare challenge")
        );
        assert_eq!(
            detect_bot_block("Verify you are human by solving the captcha below.").as_deref(),
            Some("CAPTCHA verification")
        );
        assert_eq!(
            detect_bot_block("Access denied: automated traffic detected.").as_deref(),
            Some("bot detection")
        );
        // A firewall rule, not a challenge. Short, and it carries neither
        // "attention required" nor "automated", so every rule above misses it.
        let firewall = "Access denied\nError 1020\nYou do not have access to example.test.\
                        \nCloudflare Ray ID: 8a1b2c3d4e5f";
        assert_eq!(
            detect_bot_block(firewall).as_deref(),
            Some("Cloudflare firewall rule")
        );
    }

    /// Clearing browser data is the only way back from a wrong block, and the
    /// success message already promises it. Asserts the sibling login clear
    /// too, because one message covers both tables.
    #[tokio::test]
    async fn clear_data_empties_the_headless_blocklist() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let runtime = BrowserRuntime::new(workspace.path().to_path_buf(), pool.clone());

        HeadlessBlocklist::block(&pool, "example.com", "bot detection")
            .await
            .expect("record the block");
        BrowserLogins::record(&pool, "example.com", "example.com")
            .await
            .expect("record the login");
        assert!(HeadlessBlocklist::is_blocked(&pool, "example.com")
            .await
            .expect("read the blocklist")
            .is_some());

        runtime.clear_data().await.expect("clear browser data");

        assert_eq!(
            HeadlessBlocklist::is_blocked(&pool, "example.com")
                .await
                .expect("read the blocklist"),
            None,
            "a cleared workspace must retry the domain headless"
        );
        let logins: i64 = sqlx::query_scalar("SELECT count(*) FROM browser_logins")
            .fetch_one(&pool)
            .await
            .expect("count logins");
        assert_eq!(logins, 0);

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
