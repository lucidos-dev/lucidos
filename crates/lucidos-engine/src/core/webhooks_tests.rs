//! Verification is the whole security surface of a public endpoint. It is
//! tested here as pure functions, against the three real senders it claims to
//! cover.

use super::*;
use crate::core::webhook_refusal::RefusalCause;

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
        last_accepted_at: None,
        last_refused_at: None,
        last_refusal_reason: None,
        refusal_run: RefusalRun::default(),
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

// ── The digests the senders themselves publish ───────────────────────────
//
// The three tests above round-trip: they build the expected signature with our
// own `sign`, so they prove the pieces agree with each other and nothing more.
// A pinned digest is the other half. It fails if `sign`, `canonical_string` or
// either extractor changes what it computes, however self-consistently.

/// GitHub's own documented example, secret and payload both.
#[test]
fn the_github_vector_from_their_docs_verifies() {
    let cfg = github_config();
    let secret = "It's a Secret to Everybody";
    let body = "Hello, World!";
    let published = "757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
    assert_eq!(sign(&cfg, secret, body), published);

    let hook = hook_with(None, Some(cfg));
    let header = format!("sha256={published}");
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: Some(&header),
        timestamp_header: None,
        body,
        now_unix: 1_700_000_000,
    };
    assert_eq!(verify(&hook, &presented, Some(secret)), Ok(()));
}

/// Slack's own documented example. The timestamp is signed with the body, so
/// this pins the template as well as the digest.
#[test]
fn the_slack_vector_from_their_docs_verifies() {
    let cfg = slack_config();
    let secret = "8f742231b10e8888abcd99yyyzzz85a5";
    let body = "token=xyzz0WbapA4vBCDEFasx0q6G&team_id=T1DC2JH3J&team_domain=testteamnow\
                &channel_id=G8PSS9T3V&channel_name=foobar&user_id=U2CERLKJA\
                &user_name=roadrunner&command=%2Fwebhook-collect&text=\
                &response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT1DC2JH3J\
                %2F397700885554%2F96rGlfmibIGlgcZRskXaIFfN\
                &trigger_id=398738663015.47445629121.803a0bc887a14d10d2c447fce8b6703c";
    let signed_at = 1_531_420_618;
    let published = "a2114d57b48eac39b9ad189dd8316235a7b4a8d21a10bd27519666489c69b503";
    assert_eq!(
        sign(
            &cfg,
            secret,
            &canonical_string(&cfg.template, Some("1531420618"), body)
        ),
        published
    );

    let hook = hook_with(None, Some(cfg));
    let header = format!("v0={published}");
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: Some(&header),
        timestamp_header: Some("1531420618"),
        body,
        // Slack's example is from 2018, so a real clock would replay-refuse it.
        now_unix: signed_at,
    };
    assert_eq!(verify(&hook, &presented, Some(secret)), Ok(()));
}

/// Stripe publishes no vector, so this one is frozen rather than quoted. It
/// still fails on a change to what the engine computes, which is the point.
#[test]
fn the_frozen_stripe_vector_verifies() {
    let cfg = stripe_config();
    let secret = "whsec_frozen_test_vector";
    let body = r#"{"id":"evt_1"}"#;
    let signed_at = 1_700_000_000;
    let frozen = "266ac802ab1ec1286a6fd80dc96feee4f4d5291d5e3e6bec12d1aa2e366ba366";
    assert_eq!(sign(&cfg, secret, &format!("{signed_at}.{body}")), frozen);

    let hook = hook_with(None, Some(cfg));
    let header = format!("t={signed_at},v1={frozen}");
    let presented = PresentedDelivery {
        authorization: None,
        signature_header: Some(&header),
        timestamp_header: None,
        body,
        now_unix: signed_at,
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

    let (_, no_token) = WebhookStore::update(
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
    assert!(
        no_token.is_none(),
        "an update that left the verifier alone mints nothing"
    );
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

/// The whole reason `hmac` became editable: a hook keeps its delivery URL
/// across a change of verifier, so the sender it was given to keeps working.
///
/// Each transition also moves the OTHER verifier, because `verify` requires
/// every one a row carries. A hook holding both would refuse every real signed
/// delivery, and a hook holding neither cannot be stored at all.
#[tokio::test]
async fn changing_the_verifier_swaps_it_whole_and_keeps_the_url() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let secret = "It's a Secret to Everybody";
    let body = r#"{"action":"opened"}"#;

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
    assert!(token.is_some(), "an unsigned hook authenticates by token");

    // Unsigned to signed. The token has to go, or GitHub could never reach it.
    let (signed, minted) = WebhookStore::update(
        &pool,
        &bus,
        hook.id,
        WebhookPatch {
            hmac: HmacChange::Set(github_config()),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
    .expect("the hook exists");
    assert_eq!(signed.id, hook.id, "the delivery URL is the id");
    assert!(minted.is_none(), "setting a signature mints nothing");
    assert!(signed.token_hash.is_none(), "the old token is gone");
    let header = format!("sha256={}", sign(&github_config(), secret, body));
    let delivery = PresentedDelivery {
        authorization: None,
        signature_header: Some(&header),
        timestamp_header: None,
        body,
        now_unix: 1_700_000_000,
    };
    assert_eq!(verify(&signed, &delivery, Some(secret)), Ok(()));

    // A rotation: same hook, same URL, a different credential named.
    let mut rotated_cfg = github_config();
    rotated_cfg.credential = "example-repo-webhook-2026".into();
    let (rotated, _) = WebhookStore::update(
        &pool,
        &bus,
        hook.id,
        WebhookPatch {
            hmac: HmacChange::Set(rotated_cfg),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
    .expect("the hook exists");
    assert_eq!(rotated.id, hook.id);
    assert_eq!(
        rotated.hmac.as_ref().unwrap().credential,
        "example-repo-webhook-2026"
    );

    // Signed back to unsigned. A token is minted, or the row would carry no
    // verifier and the table's CHECK would refuse it.
    let (unsigned, fresh) = WebhookStore::update(
        &pool,
        &bus,
        hook.id,
        WebhookPatch {
            hmac: HmacChange::Clear,
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
    .expect("the hook exists");
    assert_eq!(unsigned.id, hook.id);
    assert!(unsigned.hmac.is_none());
    let fresh = fresh.expect("clearing a signature mints a token");
    assert_eq!(fresh.len(), 64);
    assert_ne!(
        Some(&fresh),
        token.as_ref(),
        "a fresh token, not the one the hook was born with"
    );
    assert_eq!(
        unsigned.token_hash.as_deref(),
        Some(digest(&fresh).as_str())
    );

    // The row still holds no readable secret, after all of that.
    let stored: String = sqlx::query_scalar("SELECT webhooks::text FROM webhooks WHERE id = $1")
        .bind(hook.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!stored.contains(&fresh));
    assert!(!stored.contains(secret));

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

/// Every arm carries both its words and its key, and the two never swap.
///
/// The key is stored JSON and reaches the wire, so it is frozen. The reason is
/// prose on a page and may be reworded. A test that let them share one string
/// would make the next rewording a silent schema change.
#[test]
fn every_refusal_has_a_stable_key_and_its_own_words() {
    let mut keys = std::collections::HashSet::new();
    let mut words = std::collections::HashSet::new();
    for refusal in DeliveryRefusal::ALL {
        assert!(keys.insert(refusal.key()), "duplicate key {:?}", refusal);
        assert!(
            words.insert(refusal.reason()),
            "duplicate words {:?}",
            refusal
        );
        assert!(
            !refusal.key().contains(' '),
            "a key is a wire value, not prose: {:?}",
            refusal
        );
    }

    // Exactly one arm examined nothing, and that is what earns it its own
    // words in `core::webhook_refusal`.
    let unexamined: Vec<&str> = DeliveryRefusal::ALL
        .into_iter()
        .filter(|r| !r.examined_the_delivery())
        .map(|r| r.key())
        .collect();
    assert_eq!(unexamined, vec!["disabled"]);
}

/// The evidence a real outage leaves must survive a diagnostic probe.
///
/// `last_refusal_reason` holds only the last one, so a hand-run `curl` while
/// investigating overwrites exactly the field you came to read. The run is
/// what makes that harmless.
#[tokio::test]
async fn a_refusal_run_accumulates_and_one_probe_cannot_erase_it() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let (hook, _) = WebhookStore::create(
        &pool,
        &bus,
        "github",
        "GithubWorkflowRunStateChanged",
        WebhookConfig::default(),
        None,
    )
    .await
    .unwrap();

    async fn read(pool: &PgPool, id: Uuid) -> Webhook {
        WebhookStore::get(pool, id).await.unwrap().unwrap()
    }

    let fresh = read(&pool, hook.id).await.refusal_run;
    assert!(!fresh.is_running(), "a fresh hook is not refusing anything");
    assert_eq!(fresh.cause, None);
    assert!(fresh.since.is_none());

    for _ in 0..40 {
        WebhookStore::record_refused(&pool, hook.id, DeliveryRefusal::SignatureMismatch)
            .await
            .unwrap();
    }
    let run = read(&pool, hook.id).await.refusal_run;
    assert_eq!(run.refusals, 40);
    assert_eq!(run.reasons.get("signature-mismatch"), Some(&40));
    assert_eq!(run.cause, Some(RefusalCause::Verification));
    let started = run.since.expect("a run knows when it started");
    assert!(
        run.run_secs.is_some_and(|secs| secs >= 0),
        "the database measures the age, so it is never absent on a live run"
    );

    // The investigator's own unsigned probe, against the same live hook. It
    // lands as its own reason, and the forty behind it are untouched.
    WebhookStore::record_refused(&pool, hook.id, DeliveryRefusal::SignatureMissing)
        .await
        .unwrap();
    let after = read(&pool, hook.id).await;
    assert_eq!(after.refusal_run.refusals, 41);
    assert_eq!(
        after.refusal_run.reasons.get("signature-mismatch"),
        Some(&40)
    );
    assert_eq!(after.refusal_run.reasons.get("signature-missing"), Some(&1));
    assert_eq!(
        after.refusal_run.since,
        Some(started),
        "the run keeps its start, or it could never age past the window"
    );
    assert_eq!(
        after.last_refusal_reason.as_deref(),
        Some(DeliveryRefusal::SignatureMissing.reason()),
        "the old column still names the last one, which is what it is for"
    );

    // A refusal of the OTHER cause restarts the run, so the count and the
    // tally never describe two faults at once. Reachable only by the user
    // switching the hook off, since a live hook cannot answer `disabled`.
    WebhookStore::record_refused(&pool, hook.id, DeliveryRefusal::Disabled)
        .await
        .unwrap();
    let switched = read(&pool, hook.id).await;
    assert_eq!(switched.refusal_run.refusals, 1);
    assert_eq!(switched.refusal_run.cause, Some(RefusalCause::Disabled));
    assert_eq!(
        switched.refusal_run.reasons,
        std::collections::BTreeMap::from([("disabled".to_string(), 1)]),
        "the verification tally would otherwise be read as thrown away unread"
    );
    assert_ne!(switched.refusal_run.since, Some(started));

    // One delivery that verified ends the run whole. That is the positive
    // evidence a recovery rests on.
    WebhookStore::record_accepted(&pool, hook.id).await.unwrap();
    let accepted = read(&pool, hook.id).await;
    assert!(!accepted.refusal_run.is_running());
    assert_eq!(accepted.refusal_run.refusals, 0);
    assert_eq!(accepted.refusal_run.since, None);
    assert_eq!(accepted.refusal_run.cause, None);
    assert!(accepted.refusal_run.reasons.is_empty());
    assert!(accepted.last_accepted_at.is_some());
    assert!(
        accepted.last_refused_at.is_some(),
        "the stamps are a history, so an acceptance does not erase the refusal"
    );

    crate::test_support::teardown_test_db(&db).await;
}

/// Moving the enabled flag ends the run, because the flag IS one of the causes.
///
/// A run left standing across a switch describes a fault that is over. Its
/// count and tally then get re-reported under the other cause's words. That is
/// how a hook switched off after an hour of signature failures comes to read
/// "every one was refused before it was read".
#[tokio::test]
async fn moving_the_enabled_flag_ends_the_refusal_run() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let (hook, _) = WebhookStore::create(
        &pool,
        &bus,
        "github",
        "GithubWorkflowRunStateChanged",
        WebhookConfig::default(),
        None,
    )
    .await
    .unwrap();

    async fn read(pool: &PgPool, id: Uuid) -> Webhook {
        WebhookStore::get(pool, id).await.unwrap().unwrap()
    }

    fn patch(enabled: Option<bool>) -> WebhookPatch {
        WebhookPatch {
            enabled,
            ..WebhookPatch::default()
        }
    }

    for _ in 0..42 {
        WebhookStore::record_refused(&pool, hook.id, DeliveryRefusal::SignatureMismatch)
            .await
            .unwrap();
    }
    assert_eq!(read(&pool, hook.id).await.refusal_run.refusals, 42);

    // A PUT that touches something else leaves the run alone. So does one
    // resending the flag the row already carries.
    WebhookStore::update(&pool, &bus, hook.id, patch(None), None)
        .await
        .unwrap();
    WebhookStore::update(&pool, &bus, hook.id, patch(Some(true)), None)
        .await
        .unwrap();
    let untouched = read(&pool, hook.id).await.refusal_run;
    assert_eq!(
        untouched.refusals, 42,
        "an unrelated write must not erase a live outage's evidence"
    );
    assert_eq!(untouched.cause, Some(RefusalCause::Verification));

    // Switching it off ends the run whole, exactly as an acceptance does.
    WebhookStore::update(&pool, &bus, hook.id, patch(Some(false)), None)
        .await
        .unwrap();
    let off = read(&pool, hook.id).await;
    assert!(!off.enabled);
    assert!(!off.refusal_run.is_running());
    assert_eq!(off.refusal_run.refusals, 0);
    assert_eq!(off.refusal_run.since, None);
    assert_eq!(off.refusal_run.cause, None);
    assert!(off.refusal_run.reasons.is_empty());
    assert!(
        off.last_refused_at.is_some(),
        "the stamps are a history, so ending the run does not erase them"
    );

    // The next delivery starts an honest one, in the words the flag earns.
    WebhookStore::record_refused(&pool, hook.id, DeliveryRefusal::Disabled)
        .await
        .unwrap();
    let fresh = read(&pool, hook.id).await.refusal_run;
    assert_eq!(fresh.refusals, 1);
    assert_eq!(fresh.cause, Some(RefusalCause::Disabled));

    crate::test_support::teardown_test_db(&db).await;
}
