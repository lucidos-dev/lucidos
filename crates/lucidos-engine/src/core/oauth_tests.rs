use super::*;

/// What a credential that says nothing about authorization parameters sends,
/// i.e. what every connection made before the field existed sends. Spelled as
/// `parse(None)` rather than a literal so a test can never assert against a
/// default the parser does not actually produce.
fn default_authorize_params() -> AuthorizeParams {
    AuthorizeParams::parse(None).expect("the default parses")
}

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
        desired_scopes: None,
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
        desired_scopes: None,
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

/// The provider name reaches here from `connect_oauth_account`'s free-text
/// argument, so it can carry anything. Shares `env_var_segment` with `CRED_*`
/// so a provider that isn't already identifier-shaped can't inject a variable
/// bash refuses to expand.
#[test]
fn account_env_vars_replaces_every_non_identifier_character() {
    let accounts = vec![make_env_account("acme:cloud+eu", None, "tok")];
    let map: std::collections::HashMap<_, _> = account_env_vars(accounts).into_iter().collect();
    assert_eq!(map.get("OAUTH_ACME_CLOUD_EU_ACCESS_TOKEN").unwrap(), "tok");
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
        userinfo_method: None,
        authorize_params: None,
        scopes: Some("https://www.googleapis.com/auth/cloud-healthcare".to_string()),
        redirect_uri: Some("http://localhost:14981/oauth/callback".to_string()),
    };
    let req = oauth_client_request("ghealth", &overrides);
    assert_eq!(req["service"], "ghealth");
    assert_eq!(req["auth_type"], "oauth_client");
    assert_eq!(req["base_url"], "https://healthcare.googleapis.com");
    assert_eq!(
        req["defaults"]["auth_url"],
        "https://accounts.google.com/o/oauth2/v2/auth"
    );
    assert_eq!(
        req["defaults"]["token_url"],
        "https://oauth2.googleapis.com/token"
    );
    assert_eq!(
        req["defaults"]["userinfo_url"],
        "https://openidconnect.googleapis.com/v1/userinfo"
    );
    assert_eq!(
        req["defaults"]["scopes"],
        "https://www.googleapis.com/auth/cloud-healthcare"
    );
    assert_eq!(
        req["defaults"]["redirect_uri"],
        "http://localhost:14981/oauth/callback"
    );
}

#[test]
fn oauth_client_request_without_overrides_has_no_defaults_and_falls_back_base_url() {
    // No endpoints supplied → no `defaults` block, so the modal treats it as a
    // custom provider and expands the endpoint section for manual entry. The
    // base_url falls back to a best-effort guess.
    let req = oauth_client_request("acme", &OAuthClientOverrides::default());
    assert_eq!(req["service"], "acme");
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
    assert_eq!(
        req["defaults"]["auth_url"],
        "https://login.example.com/authorize"
    );
    assert_eq!(
        req["defaults"]["token_url"],
        "https://login.example.com/token"
    );
    let defaults = req["defaults"].as_object().expect("defaults is an object");
    assert!(
        !defaults.contains_key("userinfo_url"),
        "userinfo_url should be absent: {req}"
    );
    assert!(
        !defaults.contains_key("scopes"),
        "scopes should be absent: {req}"
    );
    // An absent redirect_uri is what selects the default loopback-IP callback,
    // so it must not be pre-filled as empty.
    assert!(
        !defaults.contains_key("redirect_uri"),
        "redirect_uri should be absent: {req}"
    );
}

#[test]
fn debug_redacts_oauth_account_tokens() {
    let mut account =
        make_env_account("google", Some("user@gmail.com"), "ya29.super-secret-access");
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
    assert!(
        dbg.contains("<redacted>"),
        "expected redaction marker: {dbg}"
    );
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
    assert!(
        !dbg.contains("gho_secret_value"),
        "access token leaked: {dbg}"
    );
    assert!(
        dbg.contains("refresh_token: None"),
        "absent refresh token should render as None: {dbg}"
    );
}

// ---------------------------------------------------------------------------
// DB-backed store tests (resolver ordering + upsert semantics).
// These need a real Postgres — run via `./scripts/test-engine.sh`.
// ---------------------------------------------------------------------------

/// The load-bearing guarantee, and the half that was missing: connecting an
/// account announces, so every OTHER device reloads its Accounts list instead
/// of waiting for a page refresh. A token rotation deliberately stays silent.
#[tokio::test]
async fn connect_announces_and_a_token_refresh_does_not() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _callback_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    async fn emitted(pool: &sqlx::PgPool, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
            .bind(event_type)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    let id = OAuthStore::connect(
        &pool,
        &bus,
        "google",
        Some("user@example.com"),
        Some("User"),
        "access",
        Some("refresh"),
        None,
        "openid email",
        "openid email",
        None,
    )
    .await
    .unwrap();
    assert_eq!(emitted(&pool, "OAuthAccountConnected").await, 1);

    // A re-authorization grants scopes, so it announces even though the row
    // already existed.
    OAuthStore::connect(
        &pool,
        &bus,
        "google",
        Some("user@example.com"),
        Some("User"),
        "access2",
        Some("refresh"),
        None,
        "openid email https://www.googleapis.com/auth/drive",
        "openid email https://www.googleapis.com/auth/drive",
        None,
    )
    .await
    .unwrap();
    assert_eq!(emitted(&pool, "OAuthAccountConnected").await, 2);

    OAuthStore::update_tokens(&pool, id, "rotated", None, None)
        .await
        .unwrap();
    assert_eq!(
        emitted(&pool, "OAuthAccountConnected").await,
        2,
        "a token rotation changes nothing the user can see"
    );

    assert!(OAuthStore::delete(&pool, &bus, id, None).await.unwrap());
    assert_eq!(emitted(&pool, "OAuthAccountDeleted").await, 1);
    assert!(!OAuthStore::delete(&pool, &bus, id, None).await.unwrap());
    assert_eq!(
        emitted(&pool, "OAuthAccountDeleted").await,
        1,
        "second delete removes nothing and therefore announces nothing"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn get_by_provider_returns_most_recently_connected() {
    // The reported bug: an old narrow-scope `drive.file` account (no email →
    // NULL) shadowed a newer full-`drive` account because the resolver ordered
    // by `created_at ASC`. Resolution must prefer the most-recently-CONNECTED
    // (newest `created_at`) account so a fresh connect wins.
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    let old_id = crate::test_support::seed_oauth_account(
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

    let new_id = crate::test_support::seed_oauth_account(
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

    let first = crate::test_support::seed_oauth_account(
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

    let second = crate::test_support::seed_oauth_account(
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

    let first = crate::test_support::seed_oauth_account(
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

    let second = crate::test_support::seed_oauth_account(
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

    crate::test_support::seed_oauth_account(
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
    crate::test_support::seed_oauth_account(
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
    assert!(
        dbg.contains("<redacted>"),
        "expected redaction marker: {dbg}"
    );
    // Non-secret fields preserved for debugging.
    assert!(dbg.contains("3600"));
    assert!(dbg.contains("Bearer"));
}

// ---------------------------------------------------------------------------
// Redirect URI — one source for both legs of the flow
// ---------------------------------------------------------------------------

/// Pull a percent-decoded query parameter out of a built authorization URL.
/// Deliberately decodes with `urlencoding` directly rather than reusing the
/// production `query_param`, so the assertion doesn't lean on the helper it is
/// there to check.
fn authorize_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    let raw = query
        .split('&')
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))?;
    Some(
        urlencoding::decode(raw)
            .expect("valid percent-encoding")
            .into_owned(),
    )
}

fn form_value<'a>(form: &'a [(&'static str, &'a str)], key: &str) -> Option<&'a str> {
    form.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn config_with(redirect_uri: Option<&str>, client_secret: Option<&str>) -> serde_json::Value {
    let mut config = serde_json::json!({ "client_id": "cid-123" });
    if let Some(uri) = redirect_uri {
        config["redirect_uri"] = serde_json::Value::String(uri.to_string());
    }
    if let Some(secret) = client_secret {
        config["client_secret"] = serde_json::Value::String(secret.to_string());
    }
    config
}

#[test]
fn resolve_redirect_uri_defaults_to_the_loopback_ip() {
    // Absent, blank and whitespace-only must all mean "unchanged default", so
    // every already-connected provider keeps the URI it was registered with.
    for stored in [None, Some(""), Some("   ")] {
        let resolved = resolve_redirect_uri(&config_with(stored, Some("s"))).unwrap();
        assert_eq!(
            resolved, "http://127.0.0.1:14981/oauth/callback",
            "stored redirect_uri {stored:?} must resolve to the default"
        );
    }
    assert_eq!(
        default_redirect_uri(),
        "http://127.0.0.1:14981/oauth/callback"
    );
}

#[test]
fn resolve_redirect_uri_accepts_every_host_form_the_listener_serves() {
    // The listener binds both loopback families, so all three host spellings
    // are genuinely receivable — Microsoft's portal only accepts the `localhost`
    // form under its Web platform, Spotify only the IP.
    for host in ["127.0.0.1", "localhost", "[::1]"] {
        let uri = format!("http://{host}:14981/oauth/callback");
        let resolved = resolve_redirect_uri(&config_with(Some(&uri), Some("s"))).unwrap();
        assert_eq!(resolved, uri, "{uri} must be accepted verbatim");
    }
}

#[test]
fn resolve_redirect_uri_rejects_a_uri_the_listener_cannot_receive() {
    // Each of these would produce a browser redirect that never reaches us —
    // a 120s silent hang. They must fail immediately and name the alternatives.
    let bad = [
        "http://127.0.0.1:3000/oauth/callback",    // wrong port
        "http://127.0.0.1:14981/callback",         // wrong path
        "http://example.com:14981/oauth/callback", // not loopback
        "https://127.0.0.1:14981/oauth/callback",  // we serve plain HTTP
        "http://127.0.0.1:14981/oauth/callback/",  // trailing-slash drift
    ];
    for uri in bad {
        let err = resolve_redirect_uri(&config_with(Some(uri), Some("s")))
            .expect_err("{uri} must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains(uri),
            "error should name the offending value: {msg}"
        );
        assert!(
            msg.contains("http://127.0.0.1:14981/oauth/callback"),
            "error should list the accepted forms: {msg}"
        );
    }
}

#[test]
fn authorize_url_and_token_exchange_send_an_identical_redirect_uri() {
    // The headline invariant: OAuth 2.0 §4.1.3 requires the redemption to repeat
    // the authorization request's redirect_uri byte for byte, and Entra compares
    // it literally. Checked across both client shapes and both URI forms.
    for stored in [None, Some("http://localhost:14981/oauth/callback")] {
        for secret in [Some("client-secret"), None] {
            let config = config_with(stored, secret);
            let redirect_uri = resolve_redirect_uri(&config).unwrap();
            let auth = ClientAuth::from_secret(config["client_secret"].as_str());

            let url = build_authorize_url(
                "https://login.example.com/authorize",
                "cid-123",
                &redirect_uri,
                "openid email",
                "the-state",
                &auth,
                &default_authorize_params(),
            );
            let form = exchange_form("the-code", "cid-123", &redirect_uri, &auth);

            let sent_on_authorize =
                authorize_param(&url, "redirect_uri").expect("authorize URL carries redirect_uri");
            let sent_on_exchange =
                form_value(&form, "redirect_uri").expect("exchange carries redirect_uri");

            assert_eq!(
                sent_on_authorize,
                sent_on_exchange,
                "legs diverged for stored={stored:?} secret={:?}",
                secret.is_some()
            );
            assert_eq!(sent_on_authorize, redirect_uri);
        }
    }
}

// ---------------------------------------------------------------------------
// Client authentication — confidential (unchanged) vs public (PKCE)
// ---------------------------------------------------------------------------

#[test]
fn a_credential_with_a_secret_is_a_confidential_client() {
    // The regression guard for every provider already connected: same params,
    // same order, no PKCE.
    let auth = ClientAuth::from_secret(Some("client-secret"));
    assert_eq!(auth.client_secret(), Some("client-secret"));
    assert_eq!(auth.code_challenge(), None);
    assert_eq!(auth.code_verifier(), None);

    let url = build_authorize_url(
        "https://accounts.google.com/o/oauth2/v2/auth",
        "cid-123",
        "http://127.0.0.1:14981/oauth/callback",
        "openid email",
        "the-state",
        &auth,
        &default_authorize_params(),
    );
    assert_eq!(
        url,
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?client_id=cid-123\
         &redirect_uri=http%3A%2F%2F127.0.0.1%3A14981%2Foauth%2Fcallback\
         &response_type=code\
         &scope=openid%20email\
         &state=the-state\
         &access_type=offline&prompt=consent",
        "the confidential authorize URL must be exactly what it has always been, \
         plus the per-flow state"
    );

    let form = exchange_form(
        "the-code",
        "cid-123",
        "http://127.0.0.1:14981/oauth/callback",
        &auth,
    );
    assert_eq!(
        form,
        vec![
            ("grant_type", "authorization_code"),
            ("code", "the-code"),
            ("client_id", "cid-123"),
            ("client_secret", "client-secret"),
            ("redirect_uri", "http://127.0.0.1:14981/oauth/callback"),
        ],
        "the confidential exchange body must be unchanged, including field order"
    );
}

#[test]
fn a_credential_without_a_secret_is_a_public_client_using_pkce() {
    // Blank, whitespace and absent all mean "public" — a desktop app that ships
    // no secret authenticates the redemption with PKCE (RFC 8252).
    for stored in [None, Some(""), Some("  ")] {
        let auth = ClientAuth::from_secret(stored);
        assert_eq!(
            auth.client_secret(),
            None,
            "no secret may be sent for {stored:?}"
        );
        let challenge = auth
            .code_challenge()
            .expect("public client sends a challenge");
        let verifier = auth
            .code_verifier()
            .expect("public client keeps a verifier");

        let url = build_authorize_url(
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            "cid-123",
            "http://127.0.0.1:14981/oauth/callback",
            "offline_access User.Read",
            "the-state",
            &auth,
            &default_authorize_params(),
        );
        assert_eq!(
            authorize_param(&url, "code_challenge").as_deref(),
            Some(challenge)
        );
        assert_eq!(
            authorize_param(&url, "code_challenge_method").as_deref(),
            Some("S256")
        );

        let form = exchange_form(
            "the-code",
            "cid-123",
            "http://127.0.0.1:14981/oauth/callback",
            &auth,
        );
        assert_eq!(
            form_value(&form, "client_secret"),
            None,
            "a public client must send no secret"
        );
        assert_eq!(form_value(&form, "code_verifier"), Some(verifier));

        // The verifier redeemed must be the one whose hash was advertised.
        assert_eq!(
            Pkce::challenge_for(verifier),
            challenge,
            "the redeemed verifier must hash to the advertised challenge"
        );
    }
}

#[test]
fn pkce_verifier_matches_rfc7636_shape() {
    let pkce = Pkce::generate();
    let len = pkce.verifier.len();
    assert!(
        (43..=128).contains(&len),
        "verifier length {len} outside RFC 7636's 43..=128"
    );
    assert!(
        pkce.verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')),
        "verifier must use only the unreserved set: {}",
        pkce.verifier
    );
    assert_ne!(
        Pkce::generate().verifier,
        pkce.verifier,
        "each flow must get a fresh verifier"
    );
}

#[test]
fn pkce_challenge_matches_the_rfc7636_test_vector() {
    // RFC 7636 Appendix B.
    assert_eq!(
        Pkce::challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn debug_redacts_the_pkce_verifier_and_client_secret() {
    // The verifier is a one-time secret (RFC 7636 §5) — it must not reach a log.
    let pkce = Pkce::generate();
    let verifier = pkce.verifier.clone();
    let dbg = format!("{:?}", ClientAuth::Public(pkce));
    assert!(!dbg.contains(&verifier), "PKCE verifier leaked: {dbg}");
    assert!(
        dbg.contains("<redacted>"),
        "expected redaction marker: {dbg}"
    );

    let dbg = format!("{:?}", ClientAuth::from_secret(Some("super-secret-value")));
    assert!(
        !dbg.contains("super-secret-value"),
        "client secret leaked: {dbg}"
    );
}

#[test]
fn refresh_omits_the_client_secret_for_a_public_client() {
    // PKCE plays no part in a refresh, but the client type still does: a public
    // client that sends a secret here is rejected the same way it would be at
    // redemption — which would look like "connect worked, then died in an hour".
    let confidential = refresh_form("rt", "cid-123", Some("client-secret"));
    assert_eq!(
        confidential,
        vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", "rt"),
            ("client_id", "cid-123"),
            ("client_secret", "client-secret"),
        ],
        "the confidential refresh body must be unchanged"
    );

    let public = refresh_form("rt", "cid-123", None);
    assert_eq!(form_value(&public, "client_secret"), None);
    assert_eq!(form_value(&public, "refresh_token"), Some("rt"));
}

// ---------------------------------------------------------------------------
// Provider error surfacing
// ---------------------------------------------------------------------------

#[test]
fn token_error_surfaces_the_providers_own_words() {
    // The exact body Entra returns for the failure this work chased down.
    let body = r#"{"error":"invalid_request","error_description":"AADSTS90023: The provided value for the input parameter 'redirect_uri' is not valid.","error_codes":[90023],"trace_id":"t","correlation_id":"c"}"#;
    let msg = describe_token_error("Token exchange", reqwest::StatusCode::BAD_REQUEST, body);
    assert!(msg.contains("400 Bad Request"), "{msg}");
    assert!(msg.contains("invalid_request"), "{msg}");
    assert!(msg.contains("AADSTS90023"), "{msg}");
    assert!(msg.contains("error_codes: [90023]"), "{msg}");
    assert_eq!(
        msg.matches("Token exchange failed").count(),
        1,
        "the leg must be named exactly once, not double-wrapped: {msg}"
    );
}

#[test]
fn token_error_falls_back_to_the_raw_body_when_it_is_not_an_oauth_error() {
    let msg = describe_token_error(
        "Token refresh",
        reqwest::StatusCode::BAD_GATEWAY,
        "<html><body>Bad gateway</body></html>",
    );
    assert!(msg.contains("502 Bad Gateway"), "{msg}");
    assert!(msg.contains("Bad gateway"), "{msg}");

    let empty = describe_token_error("Token exchange", reqwest::StatusCode::FORBIDDEN, "   ");
    assert!(empty.contains("empty response body"), "{empty}");
}

#[test]
fn token_error_truncates_a_huge_body_on_a_char_boundary() {
    let body = "é".repeat(5000);
    let msg = describe_token_error("Token exchange", reqwest::StatusCode::BAD_REQUEST, &body);
    assert!(
        msg.len() < body.len(),
        "an oversized body must be truncated"
    );
}

// ---------------------------------------------------------------------------
// Callback query parsing
// ---------------------------------------------------------------------------

#[test]
fn callback_query_extracts_the_code_without_mangling_it() {
    assert_eq!(
        parse_callback_query("code=abc123&session_state=xyz").unwrap(),
        "abc123"
    );
    // Percent escapes decode; a literal `+` in an opaque code must NOT become a
    // space (RFC 3986 query semantics, unlike form encoding).
    assert_eq!(
        parse_callback_query("code=a%2Fb%2Bc&state=s").unwrap(),
        "a/b+c"
    );
    assert_eq!(
        parse_callback_query("state=s&code=trailing").unwrap(),
        "trailing"
    );
    // A key that merely starts with "code" must not be mistaken for it.
    assert_eq!(
        parse_callback_query("code_verifier=v&code=real").unwrap(),
        "real"
    );
}

#[test]
fn callback_query_reports_a_denial_with_the_providers_reason() {
    let err = parse_callback_query("error=access_denied&error_description=The+user+denied+consent")
        .expect_err("a denial must be an error");
    let msg = err.to_string();
    assert!(msg.contains("access_denied"), "{msg}");
    assert!(
        msg.contains("The user denied consent"),
        "+ must decode to a space here: {msg}"
    );

    let bare = parse_callback_query("error=server_error").expect_err("still an error");
    assert!(bare.to_string().contains("server_error"));
}

#[test]
fn callback_query_without_code_or_error_is_still_reported() {
    let err = parse_callback_query("state=only").expect_err("nothing usable");
    assert!(err.to_string().contains("No authorization code"));
}

// ---------------------------------------------------------------------------
// Callback listener (socket-level)
// ---------------------------------------------------------------------------

/// The nonce these listener tests pretend their flow put on the authorization
/// request. A callback that does not echo it is not this flow's.
const TEST_STATE: &str = "test-flow-state";

/// Drive one HTTP request into the listener, optionally split mid-stream.
async fn send_callback_request(addr: std::net::SocketAddr, head: String, tail: Option<String>) {
    use tokio::io::AsyncWriteExt;
    tokio::spawn(async move {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
        if let Some(tail) = tail {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            stream.write_all(tail.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();
        }
        // Hold the socket open so the listener's response write can't fail.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    });
}

#[tokio::test]
async fn callback_recovers_a_long_code_split_across_reads() {
    // Microsoft authorization codes run to kilobytes and the code sits in the
    // request line, so a read-once listener returns a truncated prefix — which
    // the provider then rejects as a malformed request, long after the browser
    // said "Authorization successful!".
    let listener = CallbackListener::bind(0).await.unwrap();
    let addr = listener.local_addrs()[0];

    let code = "M.C123_BAY.2.U.".repeat(400);
    let (head, tail) = code.split_at(code.len() / 2);
    let head = format!("GET /oauth/callback?code={head}");
    let tail =
        format!("{tail}&session_state=abc&state={TEST_STATE} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");

    send_callback_request(addr, head, Some(tail)).await;

    let got = wait_for_oauth_callback(listener, "testprov", TEST_STATE)
        .await
        .unwrap();
    assert_eq!(
        got, code,
        "the full authorization code must survive a split read"
    );
}

#[tokio::test]
async fn callback_surfaces_a_provider_denial() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let addr = listener.local_addrs()[0];

    send_callback_request(
        addr,
        format!(
            "GET /oauth/callback?error=access_denied&error_description=The+user+denied+consent\
             &state={TEST_STATE} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        ),
        None,
    )
    .await;

    let err = wait_for_oauth_callback(listener, "testprov", TEST_STATE)
        .await
        .expect_err("a denial callback must be an error");
    let msg = err.to_string();
    assert!(
        msg.contains("access_denied"),
        "provider error code missing: {msg}"
    );
    assert!(
        msg.contains("denied consent"),
        "provider description missing: {msg}"
    );
}

#[tokio::test]
async fn callback_listener_binds_both_loopback_families_on_one_port() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let addrs = listener.local_addrs();
    assert!(!addrs.is_empty(), "at least the IPv4 loopback must bind");
    let port = addrs[0].port();
    assert!(
        addrs.iter().all(|addr| addr.port() == port),
        "every family must share one port: {addrs:?}"
    );
    assert!(
        addrs.iter().any(|addr| addr.is_ipv4()),
        "IPv4 loopback is required: {addrs:?}"
    );
}

#[tokio::test]
async fn callback_is_received_on_every_bound_loopback_family() {
    // `localhost` resolves to ::1 first on a dual-stack host, so an IPv4-only
    // socket silently never sees the callback. On a host without IPv6 this
    // degrades to exercising IPv4 alone, which is the documented fallback.
    let family_count = CallbackListener::bind(0).await.unwrap().local_addrs().len();

    for index in 0..family_count {
        let listener = CallbackListener::bind(0).await.unwrap();
        let addr = listener.local_addrs()[index];
        send_callback_request(
            addr,
            format!(
                "GET /oauth/callback?code=family-code&state={TEST_STATE} \
                 HTTP/1.1\r\nHost: localhost\r\n\r\n"
            ),
            None,
        )
        .await;
        let got = wait_for_oauth_callback(listener, "testprov", TEST_STATE)
            .await
            .unwrap();
        assert_eq!(got, "family-code", "callback over {addr} must be received");
    }
}

#[tokio::test]
async fn callback_ignores_browser_noise_before_the_real_redirect() {
    // Favicon probes and speculative preconnects hit the same origin. Treating
    // one as the callback would consume the accept and strand the flow.
    let listener = CallbackListener::bind(0).await.unwrap();
    let addr = listener.local_addrs()[0];

    send_callback_request(
        addr,
        "GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_string(),
        None,
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    send_callback_request(
        addr,
        format!(
            "GET /oauth/callback?code=the-real-code&state={TEST_STATE} \
             HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        ),
        None,
    )
    .await;

    let got = wait_for_oauth_callback(listener, "testprov", TEST_STATE)
        .await
        .unwrap();
    assert_eq!(got, "the-real-code");
}

#[test]
fn authorize_url_appends_to_an_endpoint_that_already_has_a_query() {
    // Azure AD B2C pins its user flow with `?p=<policy>`; a second `?` would
    // make the whole query string unparseable.
    let auth = ClientAuth::from_secret(Some("s"));
    let url = build_authorize_url(
        "https://example.b2clogin.com/example.onmicrosoft.com/oauth2/v2.0/authorize?p=B2C_1_signin",
        "cid-123",
        "http://127.0.0.1:14981/oauth/callback",
        "openid",
        "the-state",
        &auth,
        &default_authorize_params(),
    );
    assert_eq!(
        url.matches('?').count(),
        1,
        "exactly one query separator: {url}"
    );
    assert!(url.contains("?p=B2C_1_signin&client_id=cid-123"), "{url}");
    // The existing param must still parse out alongside ours.
    assert_eq!(
        authorize_param(&url, "client_id").as_deref(),
        Some("cid-123")
    );
}

#[tokio::test]
async fn callback_survives_a_connection_that_closes_without_sending() {
    // Browsers open speculative connections to the callback origin. One that
    // opens and closes without a request must not consume the flow.
    let listener = CallbackListener::bind(0).await.unwrap();
    let addr = listener.local_addrs()[0];

    tokio::spawn(async move {
        // Connect and drop immediately — the listener sees EOF, no request line.
        drop(tokio::net::TcpStream::connect(addr).await.unwrap());
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        let mut real = tokio::net::TcpStream::connect(addr).await.unwrap();
        use tokio::io::AsyncWriteExt;
        real.write_all(
            format!(
                "GET /oauth/callback?code=survived&state={TEST_STATE} \
                 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        real.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    });

    let got = wait_for_oauth_callback(listener, "testprov", TEST_STATE)
        .await
        .unwrap();
    assert_eq!(got, "survived");
}

// ---------------------------------------------------------------------------
// The callback port has one owner, and a new authorization supersedes it
// ---------------------------------------------------------------------------

/// Build a slot entry the way `prepare_oauth_flow` does.
fn active_flow(task: tokio::task::JoinHandle<()>, holds_port: bool) -> Option<ActiveCallbackFlow> {
    Some(ActiveCallbackFlow {
        task,
        holds_port: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(holds_port)),
    })
}

/// The headline invariant: pressing *Grant access* a second time must always be
/// able to start. It could not, for up to 120 seconds, because the previous
/// flow's task was spawned detached and held the fixed port with nothing able to
/// reclaim it.
///
/// The rebind here is immediate and unretried on purpose. That is what makes the
/// test fail if `release_callback_port` ever drops its `await` and only calls
/// `abort()`: cancellation would then be in flight while the bind runs, and the
/// socket would still be open often enough to matter.
#[tokio::test]
async fn releasing_the_callback_port_frees_it_for_an_immediate_rebind() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let port = listener.local_addrs()[0].port();

    // Stand in for a flow parked on the port waiting out its 120s timeout.
    let mut slot = active_flow(
        tokio::spawn(async move {
            let _held = listener;
            std::future::pending::<()>().await;
        }),
        true,
    );
    // Let it reach the await point, as a real waiting flow has.
    tokio::task::yield_now().await;

    assert!(
        release_callback_port(&mut slot).await,
        "a flow that was still waiting counts as superseded"
    );
    assert!(slot.is_none(), "the slot is emptied, not left dangling");

    CallbackListener::bind(port)
        .await
        .expect("the port must be free the moment the release returns");
}

/// A flow's task outlives its ownership of the socket: the listener is dropped
/// when the callback lands, and the token exchange, userinfo call and account
/// write all run afterwards with the port already free. Superseding must NOT
/// kill that tail. Aborting there would cancel a redemption the user has already
/// consented to, and could land between the account row committing and its
/// `OAuthAccountConnected` event, leaving a connected account no device is told
/// about.
#[tokio::test]
async fn a_flow_that_already_released_the_port_is_detached_not_aborted() {
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel::<()>();
    let mut slot = active_flow(
        tokio::spawn(async move {
            // Stands in for the token exchange: slow, and not holding the port.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let _ = finished_tx.send(());
        }),
        false,
    );

    assert!(
        !release_callback_port(&mut slot).await,
        "a flow that no longer holds the port supersedes nothing"
    );
    assert!(slot.is_none());

    // The work ran to completion despite its JoinHandle being dropped. An
    // `abort()` here would leave this receiver empty.
    tokio::time::timeout(std::time::Duration::from_secs(2), finished_rx)
        .await
        .expect("the detached flow must not be cancelled")
        .expect("the detached flow must run to completion");
}

#[tokio::test]
async fn releasing_an_empty_slot_supersedes_nothing() {
    let mut empty: Option<ActiveCallbackFlow> = None;
    assert!(!release_callback_port(&mut empty).await);
}

/// `EADDRINUSE` is the one bind failure with a remedy, and it reaches the user
/// as a toast. The raw `Address already in use (os error 48)` they were shown
/// named nothing they could act on.
#[test]
fn a_port_clash_explains_itself_and_every_other_error_keeps_its_words() {
    let msg = callback_bind_error(
        14981,
        std::io::Error::new(std::io::ErrorKind::AddrInUse, "Address already in use"),
    )
    .to_string();
    assert!(msg.contains("14981"), "the port must be named: {msg}");
    assert!(
        msg.contains("another Lucidos workspace"),
        "the likely cause must be named: {msg}"
    );
    assert!(
        msg.contains("try again"),
        "the message must say what to do: {msg}"
    );

    // Anything else is not a contention problem and must not be described as
    // one.
    let other = callback_bind_error(
        14981,
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied"),
    )
    .to_string();
    assert_eq!(other, "permission denied");
}

// ---------------------------------------------------------------------------
// `state`: the nonce that binds a callback to the flow that asked for it
// ---------------------------------------------------------------------------

/// Two jobs, and the test covers both: it must be unpredictable (it is the only
/// thing stopping anything that can reach the loopback port from injecting a
/// code) and it must differ per flow (it is how a listener tells its own
/// redirect from the one a superseded flow is still carrying).
#[test]
fn each_flow_gets_its_own_unguessable_state() {
    let a = generate_oauth_state();
    let b = generate_oauth_state();
    assert_ne!(a, b, "two flows must not share a state");
    // 32 bytes base64url-nopad, same entropy and alphabet as the PKCE verifier.
    assert_eq!(a.len(), 43, "unexpected length: {a}");
    assert!(
        a.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "state must be URL-safe without escaping: {a}"
    );
}

#[test]
fn a_callback_matches_only_on_this_flows_exact_state() {
    assert!(callback_state_matches("code=x&state=abc", "abc"));
    assert!(callback_state_matches("state=abc&code=x", "abc"));
    // Another flow's value, or none at all, is not ours. A conforming provider
    // always echoes what it was sent (RFC 6749 §4.1.2), so absent means the
    // request did not come from our authorization.
    assert!(!callback_state_matches("code=x&state=other", "abc"));
    assert!(!callback_state_matches("code=x", "abc"));
    assert!(!callback_state_matches("code=x&state=", "abc"));
    // A parameter that merely starts with the key is a different parameter.
    // Microsoft sends `session_state` on every callback.
    assert!(!callback_state_matches("code=x&session_state=abc", "abc"));
    // Percent-encoded on the way out, so decoded on the way in. `+` stays a
    // literal `+`: the value is an opaque token, not form-encoded prose.
    assert!(callback_state_matches("state=a%2Fb", "a/b"));
    assert!(callback_state_matches("state=a+b", "a+b"));
}

/// The reason `state` is not optional once a new authorization supersedes an
/// older one: the abandoned flow's redirect can land on a listener that never
/// issued it. It must be answered and skipped, NOT returned (the code belongs to
/// another client and PKCE verifier) and NOT failed on (the authorization the
/// user is completing right now is still on its way).
#[tokio::test]
async fn a_callback_from_another_flow_is_ignored_and_the_real_one_still_lands() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let addr = listener.local_addrs()[0];

    send_callback_request(
        addr,
        "GET /oauth/callback?code=someone-elses-code&state=a-superseded-flow \
         HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
            .to_string(),
        None,
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    // A forged callback with no state at all gets the same treatment.
    send_callback_request(
        addr,
        "GET /oauth/callback?code=forged HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_string(),
        None,
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    send_callback_request(
        addr,
        format!(
            "GET /oauth/callback?code=our-code&state={TEST_STATE} \
             HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        ),
        None,
    )
    .await;

    let got = wait_for_oauth_callback(listener, "testprov", TEST_STATE)
        .await
        .unwrap();
    assert_eq!(
        got, "our-code",
        "only the callback carrying this flow's state may be redeemed"
    );
}

/// The URL half of the same invariant: what the listener demands back is what
/// the authorization request actually sent, percent-encoded.
#[test]
fn the_authorize_url_carries_this_flows_state() {
    let state = generate_oauth_state();
    let url = build_authorize_url(
        "https://accounts.example.com/authorize",
        "cid",
        "http://127.0.0.1:14981/oauth/callback",
        "openid",
        &state,
        &ClientAuth::from_secret(Some("s")),
        &default_authorize_params(),
    );
    assert_eq!(authorize_param(&url, "state").as_deref(), Some(&*state));
    // And a value needing escapes survives the round trip as one parameter.
    let url = build_authorize_url(
        "https://accounts.example.com/authorize",
        "cid",
        "http://127.0.0.1:14981/oauth/callback",
        "openid",
        "a b&c=d",
        &ClientAuth::from_secret(Some("s")),
        &default_authorize_params(),
    );
    assert_eq!(authorize_param(&url, "state").as_deref(), Some("a b&c=d"));
}

// ---------------------------------------------------------------------------
// client_provider_name: what survived of the deleted `oauth:` canonicalization
// ---------------------------------------------------------------------------

/// The name is just the provider now. `auth_type = oauth_client` is what marks
/// the row, so nothing manufactures a namespace to distinguish it from a
/// same-named API key.
#[test]
fn client_provider_name_leaves_a_bare_provider_alone() {
    assert_eq!(client_provider_name("dropbox"), "dropbox");
    assert_eq!(client_provider_name("google"), "google");
}

/// The 2026-08-05 duplicate-credential bug, pinned from the other direction.
/// The chat system prompt said `service: "oauth:<provider>"` for as long as the
/// tool existed, so both spellings are still in circulation among agents and
/// workspace knowhow. They must land on ONE row: an agent saying `oauth:google`
/// and a user typing `google` into the Add Credential form cannot end up with a
/// pair the user has to tell apart by hand.
#[test]
fn client_provider_name_strips_a_legacy_prefix() {
    assert_eq!(client_provider_name("oauth:google"), "google");
    assert_eq!(client_provider_name("OAuth:Google"), "google");
    assert_eq!(
        client_provider_name(&client_provider_name("oauth:dropbox")),
        "dropbox"
    );
}

/// `connect_oauth_account` lowercases its `provider` arg before looking the
/// credential up, so a differently-cased write would store a row that lookup
/// never finds. Whitespace is trimmed for the same reason.
#[test]
fn client_provider_name_normalizes_case_and_whitespace() {
    assert_eq!(client_provider_name("Dropbox"), "dropbox");
    assert_eq!(client_provider_name("  Dropbox  "), "dropbox");
}

/// A derived connection name (the alias rule in the oauth-providers knowhow)
/// stays its own provider, not a variant of the one it borrows endpoints from.
#[test]
fn client_provider_name_keeps_derived_provider_names_distinct() {
    assert_eq!(client_provider_name("ghealth"), "ghealth");
    assert_ne!(
        client_provider_name("ghealth"),
        client_provider_name("google")
    );
}

// ---------------------------------------------------------------------------
// callback_page: the last screen of the flow
// ---------------------------------------------------------------------------

/// The success page must name the provider and state the honest tense. It is
/// written at callback RECEIPT, before the code is exchanged, so it cannot claim
/// the account is connected.
#[test]
fn callback_success_page_names_the_provider_and_what_happens_next() {
    let page = callback_page("dropbox", true);
    assert!(page.contains("dropbox"), "{page}");
    assert!(page.contains("Authorization complete"), "{page}");
    assert!(page.contains("close this tab"), "{page}");
    assert!(
        !page.contains("Authorization successful!"),
        "the bare debug-looking heading is gone: {page}"
    );
}

/// The failure page says nothing was connected and sends the user back for the
/// reason, which lives in the engine.
#[test]
fn callback_failure_page_points_back_at_lucidos() {
    let page = callback_page("dropbox", false);
    assert!(page.contains("Authorization failed"), "{page}");
    assert!(page.contains("Nothing was connected"), "{page}");
    assert!(page.contains("Return to Lucidos"), "{page}");
}

/// The injection guard, pinned. NOTHING the provider sends in the redirect may
/// be rendered: `error_description` is attacker-controllable, and echoing it
/// into HTML we serve on the user's own loopback port would be an injection sink
/// for no benefit. The only interpolated value is the engine-side provider name.
#[test]
fn callback_page_renders_no_provider_supplied_text() {
    let hostile = "err=<script>alert(1)</script>&error_description=<img onerror=x>";
    for ok in [true, false] {
        let page = callback_page("dropbox", ok);
        assert!(!page.contains("<script>"), "{page}");
        assert!(!page.contains(&hostile[..10]), "{page}");
    }
    // And the one value that IS interpolated is escaped, so a richer provider
    // name can never open a tag either.
    let page = callback_page("<script>alert(1)</script>", true);
    assert!(!page.contains("<script>"), "{page}");
    assert!(page.contains("&lt;script&gt;"), "{page}");
}

/// The page reaches for nothing over the network.
///
/// It is served by a one-shot loopback listener that holds no engine URL, so a
/// linked stylesheet would trade a certain render for a conditional one on the
/// last screen of the flow; and a redirect landing page that phones anywhere is
/// a privacy surface. Both are avoided the same way: inline CSS, and the mark
/// as inline SVG rather than an `<img>`.
///
/// The mark's `xmlns` is the one allowed exception, and only in the exact form
/// the SVG spec fixes. It is a namespace IDENTIFIER, never resolved by any user
/// agent, and it comes along with `include_str!`ing the real `favicon.svg`
/// instead of keeping a hand-stripped copy. Everything else URL-shaped is a
/// fetch and is refused.
#[test]
fn callback_page_fetches_nothing() {
    const SVG_NS: &str = "xmlns=\"http://www.w3.org/2000/svg\"";
    for ok in [true, false] {
        let page = callback_page("dropbox", ok);
        for needle in ["<link", "<script", "src=", "@import", "url(http"] {
            assert!(
                !page.contains(needle),
                "the callback page must be self-contained, found {needle}: {page}"
            );
        }
        // Strip the one permitted identifier, then no URL may remain anywhere.
        let rest = page.replace(SVG_NS, "");
        for needle in ["http://", "https://"] {
            assert!(
                !rest.contains(needle),
                "the only URL-shaped text allowed is the SVG namespace, found {needle}: {page}"
            );
        }
    }
}

/// The page wears the Lucidos brand surface, so a user landing on it can tell
/// at a glance that it is ours rather than the provider's. It could not before:
/// the palette was a flat `#16181d` grey belonging to no product, and someone
/// completing a real authorization asked whether the tab was Dropbox's.
///
/// Pinned against `styles/picker.css` `.ws-picker`, the repo's existing
/// standalone-screen treatment, whose values this page copies.
#[test]
fn callback_page_wears_the_brand_surface() {
    let page = callback_page("dropbox", true);
    // The picker's gradient stops, and the mark itself.
    assert!(page.contains("#4a97ee"), "{page}");
    assert!(page.contains("#0c52ad"), "{page}");
    assert!(page.contains("<svg"), "the mark is rendered: {page}");
    assert!(page.contains("lucidosBrand"), "{page}");
    // The palette it replaced, which matched neither the app nor the brand.
    assert!(!page.contains("#16181d"), "{page}");
}

/// A blank provider (a flow whose name never made it this far) must not render
/// an empty `<title>`, a stray label, or a hairline ruling off nothing.
#[test]
fn callback_page_tolerates_a_blank_provider() {
    let page = callback_page("", true);
    assert!(page.contains("<title>Lucidos</title>"), "{page}");
    assert!(page.contains("Authorization complete"), "{page}");
    assert!(!page.contains("<dl>"), "no value, no row: {page}");
    assert!(!page.contains("Authorized with"), "{page}");
}

/// The provider id never appears on its own. It used to be a bare trailing word
/// under the copy (`dropbox`), which a user reading the page could not place at
/// all, so it is now the value of a labelled row and the label says what the
/// authorization did with it.
#[test]
fn callback_page_labels_the_provider_it_names() {
    let ok = callback_page("dropbox", true);
    assert!(
        ok.contains("<dt>Authorized with</dt><dd>dropbox</dd>"),
        "{ok}"
    );
    let failed = callback_page("dropbox", false);
    assert!(
        failed.contains("<dt>Tried to connect</dt><dd>dropbox</dd>"),
        "{failed}"
    );
    // The label wraps the ESCAPED value, so the row can never open a tag either.
    let hostile = callback_page("<script>alert(1)</script>", true);
    assert!(
        hostile.contains("<dd>&lt;script&gt;alert(1)&lt;/script&gt;</dd>"),
        "{hostile}"
    );
}

/// The shell is a top-anchored, left-aligned column, not a block floating dead
/// center in a viewport of blue ("elements good, layout sucks", 2026-08-06). The
/// values are the workspace picker's (`styles/picker.css` `.ws-picker`).
///
/// Asserted against the `body` rule specifically rather than the whole page,
/// because the brand lockup legitimately centers its own two children.
#[test]
fn callback_page_shell_is_top_anchored_and_left_aligned() {
    let page = callback_page("dropbox", true);
    let body = css_rule(&page, "body{");
    assert!(body.contains("align-items:flex-start"), "{body}");
    assert!(!body.contains("align-items:center"), "{body}");
    assert!(body.contains("padding:4rem"), "{body}");
    let main = css_rule(&page, "main{");
    assert!(!main.contains("text-align:center"), "{main}");
    // The lockup reads left to right, the way `.ws-picker-brand` does.
    assert!(page.contains("<div class=\"brand\">"), "{page}");
    assert!(page.contains("<span>Lucidos</span>"), "{page}");
}

/// The declarations of one rule of the callback page's inline stylesheet, so a
/// layout assertion can name the element it is about.
fn css_rule<'a>(page: &'a str, selector: &str) -> &'a str {
    let start = page
        .find(selector)
        .unwrap_or_else(|| panic!("no `{selector}` rule in the page: {page}"));
    let open = start + selector.len();
    let close = page[open..]
        .find('}')
        .unwrap_or_else(|| panic!("unterminated `{selector}` rule: {page}"))
        + open;
    &page[open..close]
}

// ---------------------------------------------------------------------------
// userinfo: method selection and the two display-name shapes
// ---------------------------------------------------------------------------

/// Absent means GET. Every credential written before `userinfo_method` existed
/// omits the key, so this branch is what keeps them working untouched.
#[test]
fn userinfo_method_defaults_to_get() {
    assert_eq!(UserinfoMethod::parse(None), UserinfoMethod::Get);
    assert_eq!(UserinfoMethod::parse(Some("")), UserinfoMethod::Get);
    assert_eq!(UserinfoMethod::parse(Some("   ")), UserinfoMethod::Get);
    assert_eq!(UserinfoMethod::parse(Some("GET")), UserinfoMethod::Get);
}

/// Dropbox's `users/get_current_account` is POST-only, which is why the key
/// exists at all. Case-insensitive because the value is typed by a human in the
/// credential modal as often as it is passed by the agent.
#[test]
fn userinfo_method_reads_post_in_any_case() {
    assert_eq!(UserinfoMethod::parse(Some("POST")), UserinfoMethod::Post);
    assert_eq!(UserinfoMethod::parse(Some("post")), UserinfoMethod::Post);
    assert_eq!(UserinfoMethod::parse(Some(" Post ")), UserinfoMethod::Post);
}

/// An unrecognized value degrades to GET rather than failing. Userinfo is
/// fetched best-effort AFTER the token exchange has already succeeded, so a
/// typo here must cost the account's email, never the connection.
#[test]
fn userinfo_method_degrades_an_unknown_value_to_get() {
    assert_eq!(UserinfoMethod::parse(Some("PATCH")), UserinfoMethod::Get);
    assert_eq!(UserinfoMethod::parse(Some("nonsense")), UserinfoMethod::Get);
}

/// OIDC's flat shape.
#[test]
fn display_name_reads_the_flat_oidc_shape() {
    let body = serde_json::json!({ "email": "me@example.com", "name": "Jane Doe" });
    assert_eq!(userinfo_display_name(&body).as_deref(), Some("Jane Doe"));
}

/// Dropbox's nested shape. Reading only the flat form left this as no name at
/// all, which is half of why a connected Dropbox account read as "unknown".
#[test]
fn display_name_reads_the_nested_dropbox_shape() {
    let body = serde_json::json!({
        "email": "me@example.com",
        "name": { "given_name": "Jane", "display_name": "Jane Doe" },
    });
    assert_eq!(userinfo_display_name(&body).as_deref(), Some("Jane Doe"));
}

/// Neither shape present is None, not a panic and not an empty string.
#[test]
fn display_name_is_none_when_absent_or_unrecognized() {
    assert_eq!(userinfo_display_name(&serde_json::json!({})), None);
    assert_eq!(
        userinfo_display_name(&serde_json::json!({ "name": { "given_name": "Jane" } })),
        None
    );
    assert_eq!(
        userinfo_display_name(&serde_json::json!({ "name": 42 })),
        None
    );
}

/// The method rides into the modal's `defaults` block, so a user editing the
/// credential sees (and can correct) what the agent chose.
#[test]
fn oauth_client_request_carries_the_userinfo_method() {
    let overrides = OAuthClientOverrides {
        userinfo_url: Some("https://api.dropboxapi.com/2/users/get_current_account".to_string()),
        userinfo_method: Some("POST".to_string()),
        ..Default::default()
    };
    let req = oauth_client_request("dropbox", &overrides);
    assert_eq!(req["defaults"]["userinfo_method"], "POST");
}

// ---------------------------------------------------------------------------
// Authorization parameters: per-credential, because "give me a refresh token"
// has no standard spelling
// ---------------------------------------------------------------------------

/// The whole point of the field. Dropbox returns a short-lived token and NO
/// refresh token unless the authorize URL carries its own spelling, so every
/// Dropbox connection made before this was unrefreshable.
#[test]
fn a_provider_can_ask_for_offline_access_in_its_own_spelling() {
    let url = build_authorize_url(
        "https://www.dropbox.com/oauth2/authorize",
        "cid-123",
        "http://127.0.0.1:14981/oauth/callback",
        "files.content.write",
        "the-state",
        &ClientAuth::from_secret(Some("s")),
        &AuthorizeParams::parse(Some("token_access_type=offline")).unwrap(),
    );
    assert_eq!(
        authorize_param(&url, "token_access_type").as_deref(),
        Some("offline")
    );
    // Google's spelling must NOT ride along: an explicit value replaces the
    // default outright, so what the knowhow documents is what gets sent.
    assert_eq!(authorize_param(&url, "access_type"), None);
    assert_eq!(authorize_param(&url, "prompt"), None);
}

/// Absent means unchanged. Every stored credential predates the key, and
/// Google needs both halves to re-issue a refresh token on a repeat consent.
#[test]
fn an_absent_value_sends_exactly_what_it_always_sent() {
    assert_eq!(
        AuthorizeParams::parse(None).unwrap(),
        AuthorizeParams::parse(Some(DEFAULT_AUTHORIZE_PARAMS)).unwrap()
    );
    // Blank is the same as absent: the credential modal omits an empty field
    // from the blob, so a user who never touched it must not lose the default.
    assert_eq!(
        AuthorizeParams::parse(Some("   ")).unwrap(),
        AuthorizeParams::parse(None).unwrap()
    );
}

/// The opt-out, for a provider strict enough to reject a parameter it doesn't
/// know. Without it the default would be unavoidable.
#[test]
fn none_sends_nothing_extra() {
    let url = build_authorize_url(
        "https://accounts.example.com/authorize",
        "cid-123",
        "http://127.0.0.1:14981/oauth/callback",
        "openid",
        "the-state",
        &ClientAuth::from_secret(Some("s")),
        &AuthorizeParams::parse(Some("NONE")).unwrap(),
    );
    assert_eq!(
        url,
        // `none` opts out of the CREDENTIAL's extra parameters. `state` is not
        // one of them: the flow owns it (it is in RESERVED_AUTHORIZE_KEYS) and
        // the listener requires it back, so it is sent regardless.
        "https://accounts.example.com/authorize\
         ?client_id=cid-123\
         &redirect_uri=http%3A%2F%2F127.0.0.1%3A14981%2Foauth%2Fcallback\
         &response_type=code\
         &scope=openid\
         &state=the-state"
    );
}

/// A credential is agent- and user-writable, so it must not be able to rewrite
/// the loopback URI the callback listener is bound to, or narrow the scopes the
/// caller asked for, from a field that reads like provider trivia.
#[test]
fn the_flows_own_parameters_are_refused() {
    for key in [
        "client_id",
        "redirect_uri",
        "response_type",
        "scope",
        "state",
        "code_challenge",
        "code_challenge_method",
    ] {
        let err = AuthorizeParams::parse(Some(&format!("{key}=evil")))
            .expect_err("{key} must be refused");
        assert!(err.contains(key), "the error names the key: {err}");
    }
    // Case is not a way around it.
    assert!(AuthorizeParams::parse(Some("Redirect_URI=evil")).is_err());
    // `state` joined the list when the flow started sending one. A stored value
    // would put two `state` parameters on the URL, and the provider would echo
    // back whichever it liked, so the listener could no longer recognise its own
    // redirect.
    assert!(AuthorizeParams::parse(Some("state=pinned")).is_err());
    // A reserved key anywhere in the list fails the whole value, rather than
    // being dropped: a silently ignored parameter is a swallowed error.
    assert!(AuthorizeParams::parse(Some("token_access_type=offline&scope=evil")).is_err());
}

/// A value carrying `&` or `=` must survive as one value. Percent-decoded on
/// the way in, re-encoded on the way out, so it cannot split into further
/// parameters.
#[test]
fn a_value_with_separators_in_it_round_trips_as_one_value() {
    let params = AuthorizeParams::parse(Some("claims=a%26b%3Dc%20d")).unwrap();
    let url = build_authorize_url(
        "https://accounts.example.com/authorize",
        "cid",
        "http://127.0.0.1:14981/oauth/callback",
        "openid",
        "the-state",
        &ClientAuth::from_secret(Some("s")),
        &params,
    );
    assert_eq!(authorize_param(&url, "claims").as_deref(), Some("a&b=c d"));
    // One extra pair, not three. The four the protocol always sends after the
    // first parameter (redirect_uri, response_type, scope, state) plus this one.
    assert_eq!(url.matches('&').count(), 5, "{url}");
}

#[test]
fn a_malformed_entry_is_an_error_not_a_silent_drop() {
    assert!(AuthorizeParams::parse(Some("just_a_key")).is_err());
    assert!(AuthorizeParams::parse(Some("=novalue")).is_err());
    // A trailing separator is ordinary sloppiness, not a malformed pair.
    assert_eq!(
        AuthorizeParams::parse(Some("token_access_type=offline&")).unwrap(),
        AuthorizeParams::parse(Some("token_access_type=offline")).unwrap()
    );
    // An empty value is legitimate: some providers read presence alone.
    assert!(AuthorizeParams::parse(Some("consent=")).is_ok());
}

/// PKCE is appended after the extra parameters, so a public client's challenge
/// keeps its place at the end of the URL.
#[test]
fn pkce_still_lands_last_for_a_public_client() {
    let auth = ClientAuth::from_secret(None);
    let url = build_authorize_url(
        "https://accounts.example.com/authorize",
        "cid",
        "http://127.0.0.1:14981/oauth/callback",
        "openid",
        "the-state",
        &auth,
        &AuthorizeParams::parse(Some("token_access_type=offline")).unwrap(),
    );
    assert!(
        url.contains("&token_access_type=offline&code_challenge="),
        "{url}"
    );
    assert_eq!(
        authorize_param(&url, "code_challenge_method").as_deref(),
        Some("S256")
    );
}

/// The value rides into the modal's `defaults` block, so a user editing the
/// credential sees (and can correct) what the agent chose.
#[test]
fn oauth_client_request_carries_the_authorize_params() {
    let overrides = OAuthClientOverrides {
        authorize_params: Some("token_access_type=offline".to_string()),
        ..Default::default()
    };
    let req = oauth_client_request("dropbox", &overrides);
    assert_eq!(
        req["defaults"]["authorize_params"],
        "token_access_type=offline"
    );
}

/// A bare `oauth:` has nothing after the colon. Stripping it would yield an
/// empty service name, which is not a credential anyone can address, so the
/// input is kept and the caller's own validation rejects it visibly.
#[test]
fn client_provider_name_does_not_strip_a_prefix_with_nothing_after_it() {
    assert_eq!(client_provider_name("oauth:"), "oauth:");
    assert_eq!(client_provider_name("  OAuth:  "), "oauth:");
}

// ─── The registry bridge and the repair request ────────────────────────────
//
// The registry prefills a credential at WRITE time and never participates in a
// flow. These pin the boundary between "seeded a credential" and "drove an
// authorization", which is what keeps a credential the single description of
// its own flow.

use crate::core::oauth_registry::OAuthProviderRow;

fn row(userinfo_method: Option<&str>) -> OAuthProviderRow {
    OAuthProviderRow {
        id: "acme".to_string(),
        label: "Acme".to_string(),
        base_url: "https://api.acme.test".to_string(),
        auth_url: "https://acme.test/authorize".to_string(),
        token_url: "https://api.acme.test/token".to_string(),
        userinfo_url: Some("https://api.acme.test/me".to_string()),
        userinfo_method: userinfo_method.map(str::to_string),
        authorize_params: None,
        redirect_uri: None,
        client_type: Some("public".to_string()),
        console_label: None,
        console_url: None,
        setup_hint: None,
        permissions_hint: None,
    }
}

#[test]
fn from_registry_carries_every_endpoint_but_never_the_scopes() {
    // Scopes are a property of what the connection is FOR, not of the provider,
    // so the row must not supply them: the caller passing backup scopes and the
    // caller passing a bare sign-in both go through here.
    let overrides = OAuthClientOverrides::from_registry(&row(Some("POST")));
    assert_eq!(
        overrides.auth_url.as_deref(),
        Some("https://acme.test/authorize")
    );
    assert_eq!(
        overrides.token_url.as_deref(),
        Some("https://api.acme.test/token")
    );
    assert_eq!(
        overrides.userinfo_url.as_deref(),
        Some("https://api.acme.test/me")
    );
    assert_eq!(overrides.userinfo_method.as_deref(), Some("POST"));
    assert_eq!(overrides.base_url.as_deref(), Some("https://api.acme.test"));
    assert_eq!(overrides.scopes, None);
}

#[test]
fn a_registry_prefilled_request_asks_only_for_the_client_id() {
    // The whole point of the registry: every endpoint arrives in `defaults`, so
    // the modal's endpoint section is prefilled and collapsed rather than blank
    // and titled "(required)".
    let req = oauth_client_request("acme", &OAuthClientOverrides::from_registry(&row(None)));
    assert_eq!(req["base_url"], "https://api.acme.test");
    assert_eq!(req["defaults"]["auth_url"], "https://acme.test/authorize");
    assert_eq!(req["defaults"]["token_url"], "https://api.acme.test/token");
    // Absent on the row means absent in the request, never present-as-null: the
    // modal reads a missing key as "not prefilled".
    assert!(
        req["defaults"].get("userinfo_method").is_none(),
        "an unset userinfo_method must not reach the modal: {req}"
    );
}

// ─── missing_flow_fields ───────────────────────────────────────────────────

#[test]
fn a_complete_client_is_missing_nothing() {
    let complete =
        r#"{"client_id":"abc","auth_url":"https://a.test/x","token_url":"https://a.test/t"}"#;
    assert!(missing_flow_fields(complete).is_empty());
}

#[test]
fn the_endpointless_client_reports_both_urls() {
    // Exactly the credential the old form let a user save: a client id and
    // nothing else. It reached `prepare_oauth_flow` and died there with
    // "Missing auth_url in OAuth credentials", one screen from the cause.
    assert_eq!(
        missing_flow_fields(r#"{"client_id":"abc"}"#),
        vec!["auth_url", "token_url"]
    );
}

#[test]
fn a_blank_or_whitespace_value_counts_as_missing() {
    // `prepare_oauth_flow` trims and rejects an empty client_id, so a credential
    // holding one cannot drive a flow either. Reporting it as present would send
    // the user back to the same dead end.
    assert_eq!(
        missing_flow_fields(
            r#"{"client_id":"  ","auth_url":"https://a.test/x","token_url":"https://a.test/t"}"#
        ),
        vec!["client_id"]
    );
}

#[test]
fn an_unparseable_secret_counts_as_missing_everything() {
    // There is no recoverable client id inside a blob that is not JSON, so
    // reopening the form prefilled beats a toast about JSON.
    assert_eq!(
        missing_flow_fields("not json at all"),
        vec!["client_id", "auth_url", "token_url"]
    );
}

// ─── oauth_client_repair_request ───────────────────────────────────────────

#[test]
fn a_repair_request_targets_the_existing_row_and_keeps_the_client_id() {
    // Both are what stop a repair from creating a SECOND oauth_client for one
    // provider: the id routes the save to an update, and the retained client id
    // means the user is not asked again for a value they already gave.
    let id = uuid::Uuid::new_v4();
    let req = oauth_client_repair_request(
        "acme",
        &OAuthClientOverrides::from_registry(&row(None)),
        id,
        Some("abc"),
        &["auth_url", "token_url"],
    );
    assert_eq!(req["existing_credential_id"], id.to_string());
    assert_eq!(req["defaults"]["client_id"], "abc");
    assert_eq!(req["defaults"]["auth_url"], "https://acme.test/authorize");
    assert_eq!(req["missing"][0], "auth_url");
    assert_eq!(req["missing"][1], "token_url");
}

#[test]
fn a_repair_prompt_names_what_is_missing() {
    let req = oauth_client_repair_request(
        "acme",
        &OAuthClientOverrides::default(),
        uuid::Uuid::new_v4(),
        None,
        &["auth_url", "token_url"],
    );
    let prompt = req["prompt"].as_str().unwrap();
    assert!(
        prompt.contains("auth_url and token_url"),
        "the prompt must name the fields, got: {prompt}"
    );
}

#[test]
fn a_repair_request_with_no_registry_row_still_carries_its_target() {
    // An unknown provider has no defaults to seed, but the repair must still
    // update rather than duplicate. This is the case where indexing into an
    // absent `defaults` block would be easy to get wrong.
    let id = uuid::Uuid::new_v4();
    let req = oauth_client_repair_request(
        "unknown-thing",
        &OAuthClientOverrides::default(),
        id,
        Some("abc"),
        &["auth_url"],
    );
    assert_eq!(req["existing_credential_id"], id.to_string());
    assert_eq!(req["defaults"]["client_id"], "abc");
}

// ─── desired_scopes: Reconnect must be able to widen ───────────────────────
//
// `scopes` records what the provider GRANTED. Reconnect used to re-request it,
// and `prepare_oauth_flow` merges a request with the existing grant, so the
// merge computed `granted UNION granted`: a no-op. An account a provider had
// narrowed could never recover the difference, which is exactly what the
// engine's Dropbox permission error tells the user to do with that button.

#[tokio::test]
async fn connect_records_what_was_asked_for_beside_what_was_granted() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    crate::test_support::seed_oauth_account_with_desired(
        &pool,
        "acme",
        Some("user@example.com"),
        None,
        "access",
        None,
        None,
        "read",
        "read write",
    )
    .await
    .unwrap();

    let account = OAuthStore::get_by_provider(&pool, "acme")
        .await
        .unwrap()
        .expect("seeded account");
    assert_eq!(account.scopes, "read");
    assert_eq!(account.desired_scopes.as_deref(), Some("read write"));

    pool.close().await;
    crate::test_support::teardown_test_db(&db).await;
}

#[test]
fn a_refused_scope_survives_into_the_next_request() {
    // The accumulation `prepare_oauth_flow` performs, in the shape that matters:
    // asked for two, granted one, and the next flow must still ask for two.
    let granted = "read";
    let desired = "read write";
    let held = merge_scopes(granted, desired);
    assert_eq!(merge_scopes(&held, desired), "read write");
}

#[test]
fn accumulating_never_drops_a_scope_the_caller_did_not_mention() {
    // A reconnect asking only for what one page needs must not narrow an account
    // another page widened. Union, never replace.
    let held = merge_scopes("read", "read write");
    assert_eq!(merge_scopes(&held, "admin"), "read write admin");
}

#[tokio::test]
async fn a_legacy_account_with_no_desired_set_is_never_narrowed() {
    // Every account connected before the column existed reads NULL. The merge
    // has to treat that as "nothing extra", not as an empty set that replaces
    // the grant.
    let (pool, db) = crate::test_support::setup_test_db().await;
    crate::test_support::seed_oauth_account(
        &pool,
        "acme",
        Some("user@example.com"),
        None,
        "access",
        None,
        None,
        "read write",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE oauth_accounts SET desired_scopes = NULL WHERE provider = 'acme'")
        .execute(&pool)
        .await
        .unwrap();

    let account = OAuthStore::get_by_provider(&pool, "acme")
        .await
        .unwrap()
        .expect("seeded account");
    assert_eq!(account.desired_scopes, None);
    let held = merge_scopes(
        &account.scopes,
        account.desired_scopes.as_deref().unwrap_or(""),
    );
    assert_eq!(held, "read write");

    pool.close().await;
    crate::test_support::teardown_test_db(&db).await;
}

// ─── The shortfall: what was asked for and did not arrive ──────────────────
//
// `missing_requested_scopes` is the engine's half of a pair. The other half is
// `missingScopes` in `components/settings/oauthConnectForm.ts`, which draws the
// same shortfall on the account row. These cases are the ones that would let
// the two answers diverge.

#[test]
fn a_full_grant_is_short_of_nothing() {
    assert!(missing_requested_scopes("read write", "read write").is_empty());
}

#[test]
fn a_partial_grant_names_every_refused_scope_in_request_order() {
    assert_eq!(
        missing_requested_scopes(
            "files.content.write files.content.read files.metadata.read account_info.read",
            "account_info.read",
        ),
        vec![
            "files.content.write".to_string(),
            "files.content.read".to_string(),
            "files.metadata.read".to_string(),
        ],
    );
}

#[test]
fn a_provider_that_reports_no_scope_string_is_not_reported_short() {
    // `granted_scopes` falls back to the requested set when the token response
    // carries no `scope` at all, which is what a provider granting exactly what
    // it was asked for typically does. Reading that as a total shortfall would
    // make every such connection look broken.
    let requested = "read write";
    let granted = requested;
    assert!(missing_requested_scopes(requested, granted).is_empty());
}

#[test]
fn an_unrecorded_request_reports_no_shortfall_rather_than_a_false_one() {
    // The engine's mirror of the account row's `desired_scopes` being NULL:
    // nothing was recorded as asked for, so nothing can be missing.
    assert!(missing_requested_scopes("", "read write").is_empty());
    assert!(missing_requested_scopes("", "").is_empty());
}

#[test]
fn everything_is_missing_when_the_provider_granted_nothing() {
    assert_eq!(
        missing_requested_scopes("read write", ""),
        vec!["read".to_string(), "write".to_string()],
    );
}

#[test]
fn the_granted_side_is_a_set_so_order_and_spacing_do_not_matter() {
    assert!(missing_requested_scopes("read write", "write   read").is_empty());
    assert!(missing_requested_scopes("read", "read read").is_empty());
    // Newlines and tabs split like spaces: a provider is free to send either.
    assert!(missing_requested_scopes("read write", "read\twrite\n").is_empty());
}

#[test]
fn containment_never_stands_in_for_a_granted_scope() {
    // The whole reason this is not `core::backup::missing_scopes`. That helper's
    // requirements are substring MATCHERS, so `files.content` would "match"
    // here and hide a genuinely refused scope.
    assert_eq!(
        missing_requested_scopes("files.content.write", "files.content"),
        vec!["files.content.write".to_string()],
    );
}

// ---------------------------------------------------------------------------
// provider_for_url: host classification
//
// The caller (`engine/tools/http.rs`) attaches the user's stored bearer token
// to any URL these tests say `Some` to. A false positive is a credential
// handed to whoever registered the host, so every arm is pinned from both
// sides. See the plan doc named on `provider_for_url`.
// ---------------------------------------------------------------------------

/// Every provider, with the legitimate hosts that must keep working.
fn legitimate_hosts() -> Vec<(&'static str, &'static str)> {
    vec![
        ("google", "https://www.googleapis.com/drive/v3/files"),
        ("google", "https://oauth2.googleapis.com/token"),
        (
            "google",
            "https://gmail.googleapis.com/gmail/v1/users/me/messages",
        ),
        ("google", "https://accounts.google.com/o/oauth2/v2/auth"),
        ("microsoft", "https://graph.microsoft.com/v1.0/me"),
        (
            "microsoft",
            "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        ),
        ("github", "https://api.github.com/user/repos"),
        (
            "dropbox",
            "https://api.dropboxapi.com/2/users/get_space_usage",
        ),
        ("dropbox", "https://content.dropboxapi.com/2/files/download"),
        ("dropbox", "https://www.dropbox.com/oauth2/authorize"),
        ("spotify", "https://api.spotify.com/v1/me/player"),
        ("spotify", "https://accounts.spotify.com/api/token"),
    ]
}

#[test]
fn every_legitimate_provider_host_still_classifies() {
    for (provider, url) in legitimate_hosts() {
        assert_eq!(
            provider_for_url(url),
            Some(provider),
            "legitimate host stopped classifying: {url}"
        );
    }
}

#[test]
fn an_attacker_suffix_host_never_classifies() {
    // The reported exploit: the provider domain is a real prefix of the host,
    // but the registrable domain belongs to the attacker.
    for url in [
        "https://api.github.com.attacker.example/",
        "https://www.googleapis.com.attacker.example/drive/v3/files",
        "https://accounts.google.com.attacker.example/",
        "https://graph.microsoft.com.attacker.example/v1.0/me",
        "https://login.microsoftonline.com.attacker.example/token",
        "https://api.dropboxapi.com.attacker.example/2/users",
        "https://www.dropbox.com.attacker.example/oauth2/authorize",
        "https://api.spotify.com.attacker.example/v1/me",
        "https://accounts.spotify.com.attacker.example/api/token",
    ] {
        assert_eq!(
            provider_for_url(url),
            None,
            "attacker suffix classified: {url}"
        );
    }
}

#[test]
fn a_lookalike_registrable_domain_never_classifies() {
    // No dot before the provider domain, so the label boundary is missing and
    // the whole name is the attacker's to register.
    for url in [
        "https://notgithub.com/user/repos",
        "https://myapi.github.com.evil.test/",
        "https://evil-googleapis.com/drive/v3/files",
        "https://notgoogle.com/",
        "https://fakedropbox.com/oauth2/authorize",
        "https://xdropboxapi.com/2/users",
        "https://notapi.spotify.com.evil.test/",
        "https://evilgraph.microsoft.com.evil.test/",
    ] {
        assert_eq!(provider_for_url(url), None, "lookalike classified: {url}");
    }
}

#[test]
fn the_provider_name_in_a_path_or_query_never_classifies() {
    // The two unanchored arms (`google.com/`, `dropbox.com`) matched anywhere
    // in the string, so a bare redirect parameter was enough to collect a
    // bearer. Every provider is checked, not just those two.
    for url in [
        "https://evil.example/?next=google.com/",
        "https://evil.example/google.com/oauth",
        "https://evil.example/?next=https://www.googleapis.com/drive",
        "https://evil.example/api.github.com/user",
        "https://evil.example/?r=api.github.com",
        "https://evil.example/dropbox.com/files",
        "https://evil.example/?to=api.dropboxapi.com",
        "https://evil.example/graph.microsoft.com/v1.0/me",
        "https://evil.example/?u=api.spotify.com",
        "https://evil.example/#accounts.spotify.com",
    ] {
        assert_eq!(
            provider_for_url(url),
            None,
            "path or query bait classified: {url}"
        );
    }
}

#[test]
fn a_plaintext_http_url_injects_nothing() {
    // A matched token would otherwise cross the wire in the clear.
    for (_, https) in legitimate_hosts() {
        let http = https.replacen("https://", "http://", 1);
        assert_eq!(
            provider_for_url(&http),
            None,
            "cleartext URL classified: {http}"
        );
    }
}

#[test]
fn a_non_https_scheme_injects_nothing() {
    for url in [
        "ftp://api.github.com/",
        "file:///api.github.com/",
        "ws://api.spotify.com/",
        "HTTP://api.github.com/",
    ] {
        assert_eq!(
            provider_for_url(url),
            None,
            "non-https scheme classified: {url}"
        );
    }
}

#[test]
fn classification_fails_closed_when_the_host_is_unknown() {
    // No host to judge means no injection, never a guess.
    for url in [
        "",
        "   ",
        "not a url",
        "//api.github.com/",
        "https:///x",
        "https://",
    ] {
        assert_eq!(
            provider_for_url(url),
            None,
            "unparseable URL classified: {url}"
        );
    }
}

#[test]
fn userinfo_cannot_smuggle_a_provider_host() {
    // `https://api.github.com@evil.example/` has host `evil.example`; the
    // provider domain sits in the userinfo, where a string match would find it.
    for url in [
        "https://api.github.com@evil.example/",
        "https://www.googleapis.com@evil.example/drive",
        "https://user:api.spotify.com@evil.example/",
    ] {
        assert_eq!(
            provider_for_url(url),
            None,
            "userinfo bait classified: {url}"
        );
    }
}

#[test]
fn a_unicode_lookalike_host_never_classifies() {
    // The parser punycodes a non-ASCII host, so a Cyrillic lookalike cannot
    // compare equal to the ASCII domain.
    assert_eq!(provider_for_url("https://\u{0430}pi.github.com/"), None);
    assert_eq!(provider_for_url("https://api.g\u{043e}ogle.com/"), None);
}

#[test]
fn host_matching_is_case_insensitive() {
    assert_eq!(
        provider_for_url("https://API.GitHub.COM/user"),
        Some("github")
    );
    assert_eq!(
        provider_for_url("https://WWW.GoogleAPIs.com/drive"),
        Some("google")
    );
}

#[test]
fn host_is_anchors_on_a_label_boundary() {
    // The single predicate the whole classifier rests on.
    assert!(host_is("api.github.com", "api.github.com"));
    assert!(host_is("sub.api.github.com", "api.github.com"));
    assert!(!host_is("notgithub.com", "github.com"));
    assert!(!host_is(
        "api.github.com.attacker.example",
        "api.github.com"
    ));
    assert!(!host_is("github.com.evil", "github.com"));
    assert!(!host_is("xgithub.com", "github.com"));
}
