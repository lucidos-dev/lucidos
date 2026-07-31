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
        redirect_uri: Some("http://localhost:14981/oauth/callback".to_string()),
    };
    let req = oauth_client_request("ghealth", &overrides);
    assert_eq!(req["service"], "oauth:ghealth");
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
                &auth,
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
        &auth,
    );
    assert_eq!(
        url,
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?client_id=cid-123\
         &redirect_uri=http%3A%2F%2F127.0.0.1%3A14981%2Foauth%2Fcallback\
         &response_type=code\
         &scope=openid%20email\
         &access_type=offline&prompt=consent",
        "the confidential authorize URL must be exactly what it has always been"
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
            &auth,
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
    let tail = format!("{tail}&session_state=abc HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");

    send_callback_request(addr, head, Some(tail)).await;

    let got = wait_for_oauth_callback(listener).await.unwrap();
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
        "GET /oauth/callback?error=access_denied&error_description=The+user+denied+consent \
         HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
            .to_string(),
        None,
    )
    .await;

    let err = wait_for_oauth_callback(listener)
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
            "GET /oauth/callback?code=family-code HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string(),
            None,
        )
        .await;
        let got = wait_for_oauth_callback(listener).await.unwrap();
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
        "GET /oauth/callback?code=the-real-code HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_string(),
        None,
    )
    .await;

    let got = wait_for_oauth_callback(listener).await.unwrap();
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
        &auth,
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
        real.write_all(b"GET /oauth/callback?code=survived HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        real.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    });

    let got = wait_for_oauth_callback(listener).await.unwrap();
    assert_eq!(got, "survived");
}
