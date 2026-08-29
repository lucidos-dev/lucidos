use super::*;
use axum::http::HeaderName;

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (name, value) in pairs {
        h.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    h
}

/// Both rules, so a test names which one it is asserting.
const BOTH_RULES: [(&str, RefererRule); 2] = [
    ("Required", RefererRule::Required),
    ("WhenPresent", RefererRule::WhenPresent),
];

/// Regression: an app iframe cannot read a credential, and cannot read the
/// backup key either.
///
/// Both routes returned their secret to any caller. Apps are same-origin, so
/// `Sec-Fetch-Site` reads `same-origin` for them exactly as for the Settings
/// page, and the `Referer` path is the only thing that differs.
#[test]
fn an_app_document_is_refused_under_either_rule() {
    for (label, rule) in BOTH_RULES {
        for referer in [
            "https://localhost:5251/app/habit-tracker/",
            "https://localhost:5251/app/habit-tracker/index.html?x=1",
            "https://localhost:5251/dev/app/habit-tracker/",
            "/app/habit-tracker/",
        ] {
            assert!(
                !reveal_request_allowed(
                    &headers(&[("sec-fetch-site", "same-origin"), ("referer", referer)]),
                    rule,
                ),
                "an app document must be refused under {label}: {referer}"
            );
        }
    }
}

/// The Settings page keeps working, direct and behind the gateway.
#[test]
fn the_workspace_shell_is_allowed_under_either_rule() {
    for (label, rule) in BOTH_RULES {
        for referer in [
            "https://localhost:5251/",
            "https://localhost:5251/index.html",
            "https://localhost:5251/dev/",
            "https://localhost:5251/dev/index.html",
            // A workspace whose slug is literally `app` is a shell, not an app:
            // no app id follows.
            "https://localhost:5251/app/",
        ] {
            assert!(
                reveal_request_allowed(
                    &headers(&[("sec-fetch-site", "same-origin"), ("referer", referer)]),
                    rule,
                ),
                "the workspace shell must be allowed under {label}: {referer}"
            );
        }
    }
}

/// The mint is stricter than the gateway's control plane, deliberately.
///
/// A browser that suppressed its `Referer` removed the only thing telling it
/// apart from an app. So `referrerPolicy: 'no-referrer'` cannot mint.
#[test]
fn a_browser_that_hides_its_referer_cannot_mint() {
    for pairs in [
        &[("sec-fetch-site", "same-origin")][..],
        &[("origin", "https://localhost:5251")][..],
    ] {
        assert!(!reveal_request_allowed(
            &headers(pairs),
            RefererRule::Required
        ));
    }
}

/// The redeem lets that same request through, and the token is what gates it.
///
/// The service worker re-issues a `GET` on iOS. A re-issue is meant to carry
/// the original referrer, and a browser that dropped it would take the Copy
/// button down in the installed PWA. Nothing is lost: a token exists only
/// because a mint passed the strict rule above.
#[test]
fn a_browser_that_hides_its_referer_can_still_redeem() {
    assert!(reveal_request_allowed(
        &headers(&[("sec-fetch-site", "same-origin")]),
        RefererRule::WhenPresent
    ));
}

/// A non-browser client sends no fetch metadata at all. The loopback bind is
/// its boundary, and this is the CLI and the API e2e suite.
#[test]
fn a_non_browser_client_is_allowed_under_either_rule() {
    for (label, rule) in BOTH_RULES {
        assert!(
            reveal_request_allowed(&HeaderMap::new(), rule),
            "a bare request must pass {label}"
        );
        assert!(
            reveal_request_allowed(&headers(&[("user-agent", "lucidos-cli/1")]), rule),
            "the CLI must pass {label}"
        );
    }
}

#[test]
fn a_token_reveals_its_own_subject_exactly_once() {
    let store = RevealTokens::new();
    for subject in [
        RevealSubject::Credential(uuid::Uuid::new_v4()),
        RevealSubject::BackupKey,
    ] {
        let token = store.mint(subject).expect("mint");
        assert!(store.redeem(&token, subject), "the first spend succeeds");
        assert!(
            !store.redeem(&token, subject),
            "a token is one-shot; a replay must be refused"
        );
    }
}

/// A token is bound to the row it was minted for. Without this, one click on
/// one credential would open every other one for 30 seconds.
#[test]
fn a_token_does_not_reveal_a_different_credential() {
    let store = RevealTokens::new();
    let id = RevealSubject::Credential(uuid::Uuid::new_v4());
    let other = RevealSubject::Credential(uuid::Uuid::new_v4());
    let token = store.mint(id).expect("mint");

    assert!(!store.redeem(&token, other), "wrong id must be refused");
    assert!(
        !store.redeem(&token, id),
        "and the mishandled token is spent, not re-offerable"
    );
}

/// The two secrets share one store, so a token must not cross between them.
/// A credential reveal is an ordinary Settings click; the backup key encrypts
/// every archive the workspace ever uploaded.
#[test]
fn a_credential_token_does_not_open_the_backup_key() {
    let store = RevealTokens::new();
    let credential = store
        .mint(RevealSubject::Credential(uuid::Uuid::new_v4()))
        .expect("mint");
    assert!(!store.redeem(&credential, RevealSubject::BackupKey));

    let backup = store.mint(RevealSubject::BackupKey).expect("mint");
    assert!(!store.redeem(&backup, RevealSubject::Credential(uuid::Uuid::new_v4())));
}

#[test]
fn an_unknown_token_is_refused() {
    let store = RevealTokens::new();
    assert!(!store.redeem("nope", RevealSubject::BackupKey));
    assert!(!store.redeem("nope", RevealSubject::Credential(uuid::Uuid::new_v4())));
}

/// Two mints never collide, and each spends independently.
#[test]
fn two_tokens_are_distinct() {
    let store = RevealTokens::new();
    let a = RevealSubject::Credential(uuid::Uuid::new_v4());
    let b = RevealSubject::BackupKey;
    let first = store.mint(a).expect("mint");
    let second = store.mint(b).expect("mint");

    assert_ne!(first, second, "tokens must not repeat");
    assert!(store.redeem(&first, a));
    assert!(store.redeem(&second, b));
}

/// The mint response tells the caller how long it has, so a client can decide
/// whether to re-mint rather than guessing at the window.
#[test]
fn the_mint_response_reports_the_window() {
    let body = RevealTokenResponse::new("abc".to_string());
    assert_eq!(body.token, "abc");
    assert_eq!(body.expires_in_secs, TOKEN_TTL.as_secs());
}

/// Both refusals name what happened. A silent 403 in front of a Copy button is
/// undebuggable from the page.
#[test]
fn a_refusal_says_which_wall_it_hit() {
    let (status, body) = forbidden("the backup key");
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("the backup key"), "{body}");

    let (status, body) = token_required("/api/v1/backup/key/reveal-token");
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("/api/v1/backup/key/reveal-token"), "{body}");
}
