use chromiumoxide::{Browser, Page};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const NAVIGABLE_SCHEMES: [&str; 2] = ["http", "https"];

/// INVARIANT: Parse a model-supplied URL, refusing any scheme outside
/// [`NAVIGABLE_SCHEMES`].
///
/// Chrome must receive the returned canonical form, never the raw string.
/// Chrome strips tabs, newlines and leading control characters before it
/// parses, so a raw string can carry a scheme the check never saw.
fn navigable_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw)
        .map_err(|e| format!("'{raw}' is not a full http or https URL ({e})."))?;
    if !NAVIGABLE_SCHEMES.contains(&url.scheme()) {
        return Err(format!(
            "The browser opens only http and https URLs. Refused the '{}:' scheme.",
            url.scheme()
        ));
    }
    Ok(url)
}

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
        // INVARIANT: Cloudflare's firewall block page, which says neither "attention
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

pub struct HeadlessBlocklist;

impl HeadlessBlocklist {
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

    pub async fn clear(pool: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM headless_blocked")
            .execute(pool)
            .await?;
        Ok(())
    }
}

/// INVARIANT: Tracks sites the user has logged into via the persistent browser profile.
///
/// Deliberately NOT surfaced to the LLM: the table is populated from whatever
/// the user's persistent browser profile happens to hold a session for, which
/// is unfiltered browsing data. The prompt section that listed it was removed
/// (see the note in `engine::chat::process::system_prompt`, and
/// `.claude/rules/no-private-data.md`). It exists so `browser_forget_login`
/// can act on a domain the user names explicitly.
pub struct BrowserLogins;

impl BrowserLogins {
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

struct BrowserState {
    browser: Browser,
    current_page: Option<Page>,
    handler_task: JoinHandle<()>,
    visible: bool,
}

pub struct BrowserRuntime {
    workspace_path: PathBuf,
    instances: Arc<Mutex<HashMap<String, BrowserState>>>,
    pool: PgPool,
}

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

    const SCHEME_REFUSAL: &str = "opens only http and https URLs";

    #[test]
    fn only_http_and_https_urls_are_navigable() {
        for ok in ["https://example.com/a?b#c", "http://example.com:8080/"] {
            assert!(navigable_url(ok).is_ok(), "url: {ok}");
        }
        for refused in [
            "file:///etc/hosts",
            "FILE:///etc/hosts",
            "data:text/html,<p>hi</p>",
            "chrome://settings",
            "javascript:alert(1)",
            "view-source:https://example.com",
            "about:blank",
        ] {
            let err = navigable_url(refused).expect_err(refused);
            assert!(err.contains(SCHEME_REFUSAL), "url: {refused}, err: {err}");
        }
    }

    #[test]
    fn a_disguised_file_scheme_is_still_refused() {
        for disguised in [" file:///etc/hosts", "fi\tle:///etc/hosts", "\nfile:///x"] {
            let err = navigable_url(disguised).expect_err(disguised);
            assert!(
                err.contains(SCHEME_REFUSAL),
                "url: {disguised:?}, err: {err}"
            );
        }
    }

    #[test]
    fn a_url_without_a_scheme_is_refused() {
        assert!(navigable_url("example.com").is_err());
        assert!(navigable_url("/etc/hosts").is_err());
    }

    #[tokio::test]
    async fn browser_open_refuses_a_file_url_and_opens_no_page() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let runtime = BrowserRuntime::new(
            workspace.path().to_path_buf(),
            crate::test_support::offline_pool(),
        );

        let err = runtime
            .open("s", "file:///etc/hosts", None, false)
            .await
            .expect_err("a file URL must be refused");
        assert!(err.contains(SCHEME_REFUSAL), "err: {err}");

        let err = runtime
            .screenshot("s", "shot.png", None, false, Some("file:///etc/hosts"))
            .await
            .expect_err("a file URL must be refused");
        assert!(err.contains(SCHEME_REFUSAL), "err: {err}");

        assert!(
            runtime.instances.lock().await.is_empty(),
            "a refused URL must not launch a browser"
        );
    }

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
        let firewall = "Access denied\nError 1020\nYou do not have access to example.test.\
                        \nCloudflare Ray ID: 8a1b2c3d4e5f";
        assert_eq!(
            detect_bot_block(firewall).as_deref(),
            Some("Cloudflare firewall rule")
        );
    }

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
