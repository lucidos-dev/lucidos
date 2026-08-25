//! Verification is the whole security surface of a public endpoint. It is
//! tested here as pure functions, against the three real senders it claims to
//! cover.

use super::*;

fn hook_with(token: Option<&str>, hmac: Option<HmacConfig>) -> Webhook {
    Webhook {
        id: Uuid::nil(),
        name: "build finished".into(),
        event_type: "BuildFinished".into(),
        token_hash: token.map(digest),
        hmac,
        dedupe: None,
        headers: Vec::new(),
        enabled: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

fn github_config() -> HmacConfig {
    HmacConfig {
        credential: "example-repo-webhook".into(),
        signature_header: "X-Hub-Signature-256".into(),
        algorithm: HmacAlgorithm::Sha256,
        encoding: DigestEncoding::Hex,
        prefix: Some("sha256=".into()),
        signature_key: None,
        timestamp_header: None,
        timestamp_key: None,
        template: "{body}".into(),
        tolerance_secs: None,
    }
}

fn slack_config() -> HmacConfig {
    HmacConfig {
        credential: "slack-signing".into(),
        signature_header: "X-Slack-Signature".into(),
        algorithm: HmacAlgorithm::Sha256,
        encoding: DigestEncoding::Hex,
        prefix: Some("v0=".into()),
        signature_key: None,
        timestamp_header: Some("X-Slack-Request-Timestamp".into()),
        timestamp_key: None,
        template: "v0:{timestamp}:{body}".into(),
        tolerance_secs: Some(300),
    }
}

fn stripe_config() -> HmacConfig {
    HmacConfig {
        credential: "stripe-webhook".into(),
        signature_header: "Stripe-Signature".into(),
        algorithm: HmacAlgorithm::Sha256,
        encoding: DigestEncoding::Hex,
        prefix: None,
        signature_key: Some("v1".into()),
        timestamp_header: None,
        timestamp_key: Some("t".into()),
        template: "{timestamp}.{body}".into(),
        tolerance_secs: Some(300),
    }
}

// ── The bearer token ─────────────────────────────────────────────────────

#[test]
fn a_bearer_token_is_read_out_of_the_authorization_header() {
    assert_eq!(presented_bearer(Some("Bearer abc123")), Some("abc123"));
    assert_eq!(presented_bearer(Some("bearer abc123")), Some("abc123"));
    assert_eq!(
        presented_bearer(Some("  Bearer   abc123  ")),
        Some("abc123")
    );
    assert_eq!(presented_bearer(Some("Basic abc123")), None);
    assert_eq!(presented_bearer(Some("abc123")), None);
    assert_eq!(presented_bearer(Some("Bearer ")), None);
    assert_eq!(presented_bearer(None), None);
}

#[test]
fn the_right_token_passes_and_every_other_shape_is_refused() {
    let hook = hook_with(Some("s3cret"), None);
    let presented = |auth: Option<&'static str>| PresentedDelivery {
        authorization: auth,
        signature_header: None,
        timestamp_header: None,
        body: "{}",
        now_unix: 1_700_000_000,
    };
    assert_eq!(
        verify(&hook, &presented(Some("Bearer s3cret")), None),
        Ok(())
    );
    for wrong in [None, Some("Bearer wrong"), Some("Bearer "), Some("s3cret")] {
        assert_eq!(
            verify(&hook, &presented(wrong), None),
            Err(DeliveryRefusal::Token),
            "auth: {wrong:?}"
        );
    }
}

#[test]
fn the_stored_token_is_a_digest_and_never_the_token() {
    let hook = hook_with(Some("s3cret"), None);
    let stored = hook.token_hash.unwrap();
    assert_ne!(stored, "s3cret");
    assert_eq!(stored.len(), 64);
    assert_eq!(stored, digest("s3cret"));
}

// ── The three senders the config claims to cover ─────────────────────────

#[test]
fn a_github_delivery_verifies() {
    let cfg = github_config();
    let secret = "It's a Secret to Everybody";
    let body = r#"{"action":"opened"}"#;
    let expected = sign(&cfg, secret, body);
    let hook = hook_with(None, Some(cfg));
    let header = format!("sha256={expected}");
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: Some(&header),
        timestamp_header: None,
        body,
        now_unix: 1_700_000_000,
    };
    assert_eq!(verify(&hook, &presented, Some(secret)), Ok(()));
}

#[test]
fn a_slack_delivery_signs_the_timestamp_with_the_body() {
    let cfg = slack_config();
    let secret = "slack-signing-secret";
    let body = "token=x&team_id=T1";
    let now = 1_700_000_000;
    let expected = sign(&cfg, secret, &format!("v0:{now}:{body}"));
    let hook = hook_with(None, Some(cfg));
    let header = format!("v0={expected}");
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: Some(&header),
        timestamp_header: Some("1700000000"),
        body,
        now_unix: now,
    };
    assert_eq!(verify(&hook, &presented, Some(secret)), Ok(()));
}

#[test]
fn a_stripe_delivery_reads_both_fields_out_of_one_header() {
    let cfg = stripe_config();
    let secret = "whsec_test";
    let body = r#"{"id":"evt_1"}"#;
    let now = 1_700_000_000;
    let expected = sign(&cfg, secret, &format!("{now}.{body}"));
    let hook = hook_with(None, Some(cfg));
    let header = format!("t={now},v1={expected},v0=ignored");
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: Some(&header),
        timestamp_header: None,
        body,
        now_unix: now,
    };
    assert_eq!(verify(&hook, &presented, Some(secret)), Ok(()));
}

// ── What must be refused ─────────────────────────────────────────────────

#[test]
fn a_wrong_signature_is_refused() {
    let hook = hook_with(None, Some(github_config()));
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: Some("sha256=deadbeef"),
        timestamp_header: None,
        body: "{}",
        now_unix: 1_700_000_000,
    };
    assert_eq!(
        verify(&hook, &presented, Some("secret")),
        Err(DeliveryRefusal::SignatureMismatch)
    );
}

#[test]
fn a_body_edited_after_signing_is_refused() {
    // The reason the body reaches the engine byte-for-byte. One character of
    // whitespace changes the digest.
    let cfg = github_config();
    let secret = "secret";
    let expected = sign(&cfg, secret, r#"{"a":1}"#);
    let hook = hook_with(None, Some(cfg));
    let header = format!("sha256={expected}");
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: Some(&header),
        timestamp_header: None,
        body: r#"{ "a": 1 }"#,
        now_unix: 1_700_000_000,
    };
    assert_eq!(
        verify(&hook, &presented, Some(secret)),
        Err(DeliveryRefusal::SignatureMismatch)
    );
}

#[test]
fn a_missing_signature_header_is_refused() {
    let hook = hook_with(None, Some(github_config()));
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: None,
        timestamp_header: None,
        body: "{}",
        now_unix: 1_700_000_000,
    };
    assert_eq!(
        verify(&hook, &presented, Some("secret")),
        Err(DeliveryRefusal::SignatureMissing)
    );
}

#[test]
fn a_bare_digest_does_not_satisfy_a_prefixed_scheme() {
    // Stripping a prefix that is not there would accept a digest computed for
    // some other scheme entirely.
    let cfg = github_config();
    let secret = "secret";
    let body = "{}";
    let expected = sign(&cfg, secret, body);
    let hook = hook_with(None, Some(cfg));
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: Some(&expected),
        timestamp_header: None,
        body,
        now_unix: 1_700_000_000,
    };
    assert_eq!(
        verify(&hook, &presented, Some(secret)),
        Err(DeliveryRefusal::SignatureMissing)
    );
}

#[test]
fn a_replayed_delivery_is_refused_once_it_is_stale() {
    let cfg = slack_config();
    let secret = "slack-signing-secret";
    let body = "token=x";
    let signed_at = 1_700_000_000;
    let expected = sign(&cfg, secret, &format!("v0:{signed_at}:{body}"));
    let hook = hook_with(None, Some(cfg));
    let header = format!("v0={expected}");
    let replay = PresentedDelivery {
        authorization: None,
        signature_header: Some(&header),
        timestamp_header: Some("1700000000"),
        body,
        // Half an hour later, well past the five-minute tolerance.
        now_unix: signed_at + 1800,
    };
    assert_eq!(
        verify(&hook, &replay, Some(secret)),
        Err(DeliveryRefusal::TimestampOutsideTolerance)
    );
}

#[test]
fn a_tolerance_with_no_timestamp_refuses_rather_than_skipping_the_check() {
    assert!(timestamp_within_tolerance(None, None, 0));
    assert!(timestamp_within_tolerance(Some(300), Some("100"), 200));
    assert!(timestamp_within_tolerance(Some(300), Some("300"), 100));
    assert!(!timestamp_within_tolerance(Some(300), None, 100));
    assert!(!timestamp_within_tolerance(
        Some(300),
        Some("nonsense"),
        100
    ));
    assert!(!timestamp_within_tolerance(Some(300), Some("100"), 500));
}

#[test]
fn an_extreme_timestamp_is_refused_rather_than_overflowing() {
    // The header is written by a public caller. A plain `(now - parsed).abs()`
    // panics on these in a debug build and wraps in a release one, so the
    // caller would be choosing which. Both ends refuse instead.
    let now = 1_700_000_000;
    for extreme in [i64::MIN, i64::MIN + 1, i64::MAX, -1, 0] {
        assert!(
            !timestamp_within_tolerance(Some(300), Some(&extreme.to_string()), now),
            "timestamp {extreme} must be refused"
        );
    }
    // The same arithmetic from the other side, with `now` itself extreme.
    assert!(!timestamp_within_tolerance(Some(300), Some("0"), i64::MAX));
    assert!(!timestamp_within_tolerance(Some(300), Some("0"), i64::MIN));
    // A nonsensical tolerance admits nothing rather than wrapping.
    assert!(!timestamp_within_tolerance(
        Some(-1),
        Some("1700000000"),
        now
    ));
}

#[test]
fn a_missing_credential_refuses_rather_than_verifying_nothing() {
    let hook = hook_with(None, Some(github_config()));
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: Some("sha256=whatever"),
        timestamp_header: None,
        body: "{}",
        now_unix: 1_700_000_000,
    };
    assert_eq!(
        verify(&hook, &presented, None),
        Err(DeliveryRefusal::CredentialMissing)
    );
}

#[test]
fn both_verifiers_must_pass_when_both_are_configured() {
    let cfg = github_config();
    let secret = "secret";
    let body = "{}";
    let expected = sign(&cfg, secret, body);
    let hook = hook_with(Some("s3cret"), Some(cfg));
    let signature = format!("sha256={expected}");

    let good = PresentedDelivery {
        authorization: Some("Bearer s3cret"),
        signature_header: Some(&signature),
        timestamp_header: None,
        body,
        now_unix: 1_700_000_000,
    };
    assert_eq!(verify(&hook, &good, Some(secret)), Ok(()));

    let no_token = PresentedDelivery {
        authorization: None,
        ..good
    };
    assert_eq!(
        verify(&hook, &no_token, Some(secret)),
        Err(DeliveryRefusal::Token)
    );

    let bad_signature = PresentedDelivery {
        signature_header: Some("sha256=deadbeef"),
        ..good
    };
    assert_eq!(
        verify(&hook, &bad_signature, Some(secret)),
        Err(DeliveryRefusal::SignatureMismatch)
    );
}

// ── Field extraction ─────────────────────────────────────────────────────

#[test]
fn a_key_is_matched_whole_inside_a_pair_list() {
    let cfg = stripe_config();
    let header = "t=1700000000,v1=abc,v10=wrong";
    assert_eq!(extract_signature(&cfg, header), Some("abc"));
    assert_eq!(extract_timestamp(&cfg, header, None), Some("1700000000"));
    // A key that is merely a suffix of another must not resolve.
    assert_eq!(extract_signature(&cfg, "xv1=abc"), None);
}

#[test]
fn the_canonical_string_substitutes_both_placeholders() {
    assert_eq!(canonical_string("{body}", None, "hello"), "hello");
    assert_eq!(
        canonical_string("v0:{timestamp}:{body}", Some("42"), "hello"),
        "v0:42:hello"
    );
    assert_eq!(
        canonical_string("{timestamp}.{body}", Some("42"), "x"),
        "42.x"
    );
}

#[test]
fn base64_and_sha1_are_both_expressible() {
    let mut cfg = github_config();
    cfg.encoding = DigestEncoding::Base64;
    let b64 = sign(&cfg, "secret", "body");
    assert_eq!(b64.len(), 44, "{b64}");

    cfg.encoding = DigestEncoding::Hex;
    cfg.algorithm = HmacAlgorithm::Sha1;
    assert_eq!(sign(&cfg, "secret", "body").len(), 40);
}

#[test]
fn a_constant_time_compare_still_compares() {
    assert!(ct_eq("abc", "abc"));
    assert!(!ct_eq("abc", "abd"));
    assert!(!ct_eq("abc", "abcd"));
    assert!(ct_eq("", ""));
}

// ── The store, against a real database ───────────────────────────────────

#[tokio::test]
async fn every_mutation_announces_and_the_row_holds_no_token() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
            .bind(event_type)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    let (hook, token) = WebhookStore::create(
        &pool,
        &bus,
        "deploys",
        "DeployFinished",
        WebhookConfig::default(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(emitted(&pool, "WebhookCreated").await, 1);
    let token = token.expect("an unsigned hook authenticates by token, so it gets one");
    assert_eq!(token.len(), 64);

    // The token is unrecoverable: only its digest was written, and the whole
    // row read back as text does not contain it.
    let row: String = sqlx::query_scalar("SELECT webhooks::text FROM webhooks WHERE id = $1")
        .bind(hook.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!row.contains(&token), "the row leaked the token");
    assert!(row.contains(&digest(&token)));

    // A delivery presenting that token verifies against what was stored.
    let stored = WebhookStore::get(&pool, hook.id).await.unwrap().unwrap();
    let authorization = format!("Bearer {token}");
    let presented = PresentedDelivery {
        authorization: Some(&authorization),
        signature_header: None,
        timestamp_header: None,
        body: "{}",
        now_unix: 1_700_000_000,
    };
    assert_eq!(verify(&stored, &presented, None), Ok(()));

    WebhookStore::update(
        &pool,
        &bus,
        hook.id,
        WebhookPatch {
            enabled: Some(false),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
    .expect("the hook exists");
    assert_eq!(emitted(&pool, "WebhookUpdated").await, 1);
    assert!(
        !WebhookStore::get(&pool, hook.id)
            .await
            .unwrap()
            .unwrap()
            .enabled
    );

    // An update that matched no row announces nothing.
    assert!(WebhookStore::update(
        &pool,
        &bus,
        Uuid::new_v4(),
        WebhookPatch {
            name: Some("ghost".into()),
            ..Default::default()
        },
        None
    )
    .await
    .unwrap()
    .is_none());
    assert_eq!(emitted(&pool, "WebhookUpdated").await, 1);

    assert!(WebhookStore::delete(&pool, &bus, hook.id, None)
        .await
        .unwrap());
    assert_eq!(emitted(&pool, "WebhookDeleted").await, 1);
    assert!(!WebhookStore::delete(&pool, &bus, hook.id, None)
        .await
        .unwrap());
    assert_eq!(
        emitted(&pool, "WebhookDeleted").await,
        1,
        "a second delete removes nothing and announces nothing"
    );

    crate::test_support::teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_signed_hook_gets_no_token_so_a_real_sender_can_reach_it() {
    // The bug this pins: `create` used to mint a token unconditionally, and
    // `verify` requires every verifier the row carries. A GitHub hook was
    // therefore born refusing every delivery GitHub could send, since GitHub
    // attaches no bearer token.
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let cfg = github_config();
    let secret = "It's a Secret to Everybody";
    let body = r#"{"action":"opened"}"#;
    let expected = sign(&cfg, secret, body);

    let (hook, token) = WebhookStore::create(
        &pool,
        &bus,
        "github",
        "PullRequestOpened",
        WebhookConfig {
            hmac: Some(cfg),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    assert!(token.is_none(), "a signed hook must not pin a bearer token");

    let stored = WebhookStore::get(&pool, hook.id).await.unwrap().unwrap();
    assert!(stored.token_hash.is_none());

    // Exactly what GitHub sends: a signature, and no Authorization header.
    let header = format!("sha256={expected}");
    let delivery = PresentedDelivery {
        authorization: None,
        signature_header: Some(&header),
        timestamp_header: None,
        body,
        now_unix: 1_700_000_000,
    };
    assert_eq!(verify(&stored, &delivery, Some(secret)), Ok(()));

    crate::test_support::teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_webhook_with_no_verifier_at_all_cannot_be_stored() {
    // The floor under "a webhook needs at least one verifier". The HTTP layer
    // refuses it first. This is the row that must be impossible anyway, since
    // it would be reachable from the public internet with nothing to check.
    let (pool, db) = crate::test_support::setup_test_db().await;
    let result = sqlx::query(
        "INSERT INTO webhooks (id, name, event_type, token_hash, hmac) \
         VALUES ($1, 'open', 'Whatever', NULL, NULL)",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await;
    assert!(result.is_err(), "a verifier-less webhook must be refused");

    crate::test_support::teardown_test_db(&db).await;
}
