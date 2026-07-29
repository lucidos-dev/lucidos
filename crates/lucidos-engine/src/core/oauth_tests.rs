use super::*;

#[test]
fn merge_scopes_adds_new_without_duplicates() {
    let existing = "openid email https://www.googleapis.com/auth/gmail.readonly";
    let requested = "https://www.googleapis.com/auth/calendar.readonly";
    let merged = merge_scopes(existing, requested);
    assert_eq!(
        merged,
        "openid email https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/calendar.readonly"
    );
}

#[test]
fn merge_scopes_deduplicates() {
    let existing = "openid email";
    let requested = "email https://www.googleapis.com/auth/calendar.readonly";
    let merged = merge_scopes(existing, requested);
    assert_eq!(
        merged,
        "openid email https://www.googleapis.com/auth/calendar.readonly"
    );
}

#[test]
fn merge_scopes_empty_existing() {
    let merged = merge_scopes("", "openid email");
    assert_eq!(merged, "openid email");
}

#[test]
fn merge_scopes_empty_requested() {
    let merged = merge_scopes("openid email", "");
    assert_eq!(merged, "openid email");
}

#[test]
fn merge_scopes_all_duplicates() {
    let merged = merge_scopes("openid email", "openid email");
    assert_eq!(merged, "openid email");
}

fn make_account(
    provider: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    token_expiry: Option<DateTime<Utc>>,
) -> OAuthAccount {
    OAuthAccount {
        id: Uuid::new_v4(),
        provider: provider.to_string(),
        email: Some("test@example.com".to_string()),
        display_name: None,
        access_token: access_token.to_string(),
        refresh_token: refresh_token.map(|s| s.to_string()),
        token_expiry,
        scopes: "openid email".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[test]
fn token_needs_refresh_when_expired() {
    let expired = Utc::now() - chrono::Duration::seconds(300);
    let account = make_account("google", "old-token", Some("refresh-tok"), Some(expired));
    assert!(
        token_needs_refresh(&account),
        "expired token should need refresh"
    );
}

#[test]
fn token_needs_refresh_when_expiring_within_60s() {
    let soon = Utc::now() + chrono::Duration::seconds(30);
    let account = make_account("google", "token", Some("refresh-tok"), Some(soon));
    assert!(
        token_needs_refresh(&account),
        "token expiring in 30s should need refresh"
    );
}

#[test]
fn token_does_not_need_refresh_when_valid() {
    let future = Utc::now() + chrono::Duration::seconds(3600);
    let account = make_account("google", "token", Some("refresh-tok"), Some(future));
    assert!(
        !token_needs_refresh(&account),
        "token valid for 1h should not need refresh"
    );
}

#[test]
fn token_needs_refresh_when_expiry_null_with_refresh_token() {
    let account = make_account("google", "token", Some("refresh-tok"), None);
    assert!(
        token_needs_refresh(&account),
        "null expiry with refresh token should need refresh"
    );
}

#[test]
fn token_does_not_need_refresh_when_expiry_null_without_refresh_token() {
    // GitHub-style: no expiry, no refresh token — token is long-lived
    let account = make_account("github", "ghp_token", None, None);
    assert!(
        !token_needs_refresh(&account),
        "null expiry without refresh token should not refresh"
    );
}

#[test]
fn token_does_not_need_refresh_well_beyond_boundary() {
    // Token expiring in 61s — comfortably beyond the 60s buffer, no refresh needed
    let expiry = Utc::now() + chrono::Duration::seconds(61);
    let account = make_account("google", "token", Some("refresh-tok"), Some(expiry));
    assert!(
        !token_needs_refresh(&account),
        "token expiring in 61s should not need refresh"
    );
}

#[test]
fn token_needs_refresh_at_59s() {
    let expiry = Utc::now() + chrono::Duration::seconds(59);
    let account = make_account("google", "token", Some("refresh-tok"), Some(expiry));
    assert!(
        token_needs_refresh(&account),
        "token expiring in 59s should need refresh"
    );
}

fn make_env_account(provider: &str, email: Option<&str>, token: &str) -> OAuthAccount {
    OAuthAccount {
        id: Uuid::new_v4(),
        provider: provider.to_string(),
        email: email.map(String::from),
        display_name: None,
        access_token: token.to_string(),
        refresh_token: None,
        token_expiry: None,
        scopes: String::new(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[test]
fn account_env_vars_injects_access_token_and_email() {
    let accounts = vec![make_env_account(
        "google",
        Some("user@gmail.com"),
        "ya29.test-token",
    )];
    let map: std::collections::HashMap<_, _> = account_env_vars(accounts).into_iter().collect();
    assert_eq!(
        map.get("OAUTH_GOOGLE_ACCESS_TOKEN").unwrap(),
        "ya29.test-token"
    );
    assert_eq!(map.get("OAUTH_GOOGLE_EMAIL").unwrap(), "user@gmail.com");
}

#[test]
fn account_env_vars_skips_email_when_none() {
    let accounts = vec![make_env_account("github", None, "ghp_test123")];
    let map: std::collections::HashMap<_, _> = account_env_vars(accounts).into_iter().collect();
    assert_eq!(map.get("OAUTH_GITHUB_ACCESS_TOKEN").unwrap(), "ghp_test123");
    assert!(!map.contains_key("OAUTH_GITHUB_EMAIL"));
}

#[test]
fn account_env_vars_normalizes_provider_name() {
    let accounts = vec![make_env_account("my-provider", None, "tok")];
    let map: std::collections::HashMap<_, _> = account_env_vars(accounts).into_iter().collect();
    assert_eq!(map.get("OAUTH_MY_PROVIDER_ACCESS_TOKEN").unwrap(), "tok");
}

#[test]
fn provider_not_connected_msg_names_provider_and_recovery() {
    let msg = provider_not_connected_msg("google");
    assert!(msg.contains("google"));
    assert!(msg.contains("connect_oauth_account"));
}

#[test]
fn oauth_client_request_attaches_supplied_endpoints_as_defaults() {
    // The agent looked the endpoints up in the oauth-providers knowhow and
    // passes them through. They must land in `defaults` so the modal pre-fills
    // (and does not require) the endpoint fields — even though "ghealth" is not
    // a name the engine knows anything about.
    let overrides = OAuthClientOverrides {
        base_url: Some("https://healthcare.googleapis.com".to_string()),
        auth_url: Some("https://accounts.google.com/o/oauth2/v2/auth".to_string()),
        token_url: Some("https://oauth2.googleapis.com/token".to_string()),
        userinfo_url: Some("https://openidconnect.googleapis.com/v1/userinfo".to_string()),
        scopes: Some("https://www.googleapis.com/auth/cloud-healthcare".to_string()),
    };
    let req = oauth_client_request("ghealth", &overrides);
    assert_eq!(req["service"], "oauth:ghealth");
    assert_eq!(req["auth_type"], "oauth_client");
    assert_eq!(req["base_url"], "https://healthcare.googleapis.com");
    assert_eq!(
        req["defaults"]["auth_url"],
        "https://accounts.google.com/o/oauth2/v2/auth"
    );
    assert_eq!(req["defaults"]["token_url"], "https://oauth2.googleapis.com/token");
    assert_eq!(
        req["defaults"]["userinfo_url"],
        "https://openidconnect.googleapis.com/v1/userinfo"
    );
    assert_eq!(
        req["defaults"]["scopes"],
        "https://www.googleapis.com/auth/cloud-healthcare"
    );
}

#[test]
fn oauth_client_request_without_overrides_has_no_defaults_and_falls_back_base_url() {
    // No endpoints supplied → no `defaults` block, so the modal treats it as a
    // custom provider and expands the endpoint section for manual entry. The
    // base_url falls back to a best-effort guess.
    let req = oauth_client_request("acme", &OAuthClientOverrides::default());
    assert_eq!(req["service"], "oauth:acme");
    assert_eq!(req["base_url"], "https://acme.com");
    assert!(
        req.get("defaults").is_none(),
        "no defaults block expected when nothing was supplied: {req}"
    );
}

#[test]
fn oauth_client_request_partial_overrides_only_include_supplied_keys() {
    // Only auth_url + token_url supplied — userinfo_url and scopes must be absent
    // from `defaults`, not present-as-null.
    let overrides = OAuthClientOverrides {
        auth_url: Some("https://login.example.com/authorize".to_string()),
        token_url: Some("https://login.example.com/token".to_string()),
        ..OAuthClientOverrides::default()
    };
    let req = oauth_client_request("example", &overrides);
    assert_eq!(req["base_url"], "https://example.com");
    assert_eq!(req["defaults"]["auth_url"], "https://login.example.com/authorize");
    assert_eq!(req["defaults"]["token_url"], "https://login.example.com/token");
    let defaults = req["defaults"].as_object().expect("defaults is an object");
    assert!(!defaults.contains_key("userinfo_url"), "userinfo_url should be absent: {req}");
    assert!(!defaults.contains_key("scopes"), "scopes should be absent: {req}");
}

#[test]
fn debug_redacts_oauth_account_tokens() {
    let mut account = make_env_account("google", Some("user@gmail.com"), "ya29.super-secret-access");
    account.refresh_token = Some("1//super-secret-refresh".to_string());
    let dbg = format!("{:?}", account);
    // Secrets must never appear in a `{:?}` rendering.
    assert!(
        !dbg.contains("ya29.super-secret-access"),
        "access token leaked: {dbg}"
    );
    assert!(
        !dbg.contains("1//super-secret-refresh"),
        "refresh token leaked: {dbg}"
    );
    assert!(dbg.contains("<redacted>"), "expected redaction marker: {dbg}");
    // Non-secret fields stay useful, and refresh-token *presence* is visible.
    assert!(dbg.contains("google"));
    assert!(dbg.contains("user@gmail.com"));
    assert!(
        dbg.contains("Some(\"<redacted>\")"),
        "refresh-token presence should still show: {dbg}"
    );
}

#[test]
fn debug_shows_refresh_token_none_when_absent() {
    let account = make_env_account("github", None, "gho_secret_value");
    let dbg = format!("{:?}", account);
    assert!(!dbg.contains("gho_secret_value"), "access token leaked: {dbg}");
    assert!(
        dbg.contains("refresh_token: None"),
        "absent refresh token should render as None: {dbg}"
    );
}

// ---------------------------------------------------------------------------
// DB-backed store tests (resolver ordering + upsert semantics).
// These need a real Postgres — run via `./scripts/test-engine.sh`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_by_provider_returns_most_recently_connected() {
    // The reported bug: an old narrow-scope `drive.file` account (no email →
    // NULL) shadowed a newer full-`drive` account because the resolver ordered
    // by `created_at ASC`. Resolution must prefer the most-recently-CONNECTED
    // (newest `created_at`) account so a fresh connect wins.
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    let old_id = OAuthStore::insert(
        &pool,
        "google",
        None,
        None,
        "old-narrow-access",
        Some("old-refresh"),
        None,
        "https://www.googleapis.com/auth/drive.file",
    )
    .await
    .unwrap();

    let new_id = OAuthStore::insert(
        &pool,
        "google",
        Some("user@example.com"),
        Some("User"),
        "new-broad-access",
        Some("new-refresh"),
        None,
        "openid email https://www.googleapis.com/auth/drive",
    )
    .await
    .unwrap();

    // Pin the creation order deterministically — two back-to-back inserts can
    // land in the same clock tick, which would make the assertion flaky.
    sqlx::query("UPDATE oauth_accounts SET created_at = NOW() - INTERVAL '1 day' WHERE id = $1")
        .bind(old_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE oauth_accounts SET created_at = NOW() WHERE id = $1")
        .bind(new_id)
        .execute(&pool)
        .await
        .unwrap();

    let resolved = OAuthStore::get_by_provider(&pool, "google")
        .await
        .unwrap()
        .expect("an account should resolve for google");
    assert_eq!(
        resolved.id, new_id,
        "resolver must return the newest connection, not the stale narrow-scope shadow"
    );
    assert_eq!(resolved.access_token, "new-broad-access");

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn insert_upserts_same_provider_email_in_place() {
    // Re-connecting the SAME account (same provider+email) must broaden/refresh
    // the existing row, never create a shadow. `refresh_token` survives a
    // None-passing upsert via COALESCE.
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    let first = OAuthStore::insert(
        &pool,
        "google",
        Some("user@example.com"),
        None,
        "access-1",
        Some("refresh-1"),
        None,
        "openid email",
    )
    .await
    .unwrap();

    let second = OAuthStore::insert(
        &pool,
        "google",
        Some("user@example.com"),
        None,
        "access-2",
        None,
        None,
        "openid email https://www.googleapis.com/auth/drive",
    )
    .await
    .unwrap();

    assert_eq!(
        first, second,
        "re-connecting the same provider+email must update in place, not insert a shadow row"
    );
    let all = OAuthStore::list(&pool).await.unwrap();
    assert_eq!(all.len(), 1, "same provider+email must collapse to one row");

    let acct = OAuthStore::get_by_id(&pool, first).await.unwrap().unwrap();
    assert_eq!(acct.access_token, "access-2");
    assert_eq!(
        acct.refresh_token.as_deref(),
        Some("refresh-1"),
        "a None refresh token on re-connect must preserve the stored one"
    );
    assert!(acct.scopes.contains("drive"), "scopes should be broadened");

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn insert_upserts_no_email_account_in_place() {
    // The no-email path (provider yields no userinfo) collapses to a single
    // (provider, NULL) row via the partial unique index.
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    let first = OAuthStore::insert(
        &pool,
        "google",
        None,
        None,
        "access-1",
        Some("refresh-1"),
        None,
        "https://www.googleapis.com/auth/drive.file",
    )
    .await
    .unwrap();

    let second = OAuthStore::insert(
        &pool,
        "google",
        None,
        None,
        "access-2",
        None,
        None,
        "https://www.googleapis.com/auth/drive.file",
    )
    .await
    .unwrap();

    assert_eq!(
        first, second,
        "re-connecting a no-email provider must update the single (provider, NULL) row"
    );
    let all = OAuthStore::list(&pool).await.unwrap();
    assert_eq!(all.len(), 1, "no-email provider must collapse to one row");

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn insert_keeps_distinct_emails_as_separate_rows() {
    // Genuinely distinct accounts (different non-null emails) stay separate.
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    OAuthStore::insert(
        &pool,
        "google",
        Some("a@example.com"),
        None,
        "access-a",
        None,
        None,
        "openid email",
    )
    .await
    .unwrap();
    OAuthStore::insert(
        &pool,
        "google",
        Some("b@example.com"),
        None,
        "access-b",
        None,
        None,
        "openid email",
    )
    .await
    .unwrap();

    let all = OAuthStore::list(&pool).await.unwrap();
    assert_eq!(
        all.len(),
        2,
        "distinct emails for one provider must remain separate rows"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn debug_redacts_token_response() {
    let resp = TokenResponse {
        access_token: "access-secret-xyz".to_string(),
        refresh_token: Some("refresh-secret-abc".to_string()),
        expires_in: Some(3600),
        token_type: Some("Bearer".to_string()),
        scope: Some("email".to_string()),
    };
    let dbg = format!("{:?}", resp);
    assert!(
        !dbg.contains("access-secret-xyz"),
        "access token leaked: {dbg}"
    );
    assert!(
        !dbg.contains("refresh-secret-abc"),
        "refresh token leaked: {dbg}"
    );
    assert!(dbg.contains("<redacted>"), "expected redaction marker: {dbg}");
    // Non-secret fields preserved for debugging.
    assert!(dbg.contains("3600"));
    assert!(dbg.contains("Bearer"));
}
