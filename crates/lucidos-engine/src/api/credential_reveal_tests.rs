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

/// Regression: an app iframe cannot read a credential.
///
/// The route returned the plaintext to any caller. Apps are same-origin, so
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
fn a_token_reveals_its_own_credential_exactly_once() {
    let store = RevealTokens::new();
    let id = uuid::Uuid::new_v4();
    let token = store.mint(id).expect("mint");

    assert!(store.redeem(&token, id), "the first spend succeeds");
    assert!(
        !store.redeem(&token, id),
        "a token is one-shot; a replay must be refused"
    );
}

/// A token is bound to the row it was minted for. Without this, one click on
/// one credential would open every other one for 30 seconds.
#[test]
fn a_token_does_not_reveal_a_different_credential() {
    let store = RevealTokens::new();
    let id = uuid::Uuid::new_v4();
    let other = uuid::Uuid::new_v4();
    let token = store.mint(id).expect("mint");

    assert!(!store.redeem(&token, other), "wrong id must be refused");
    assert!(
        !store.redeem(&token, id),
        "and the mishandled token is spent, not re-offerable"
    );
}

#[test]
fn an_unknown_token_is_refused() {
    let store = RevealTokens::new();
    assert!(!store.redeem("nope", uuid::Uuid::new_v4()));
}

/// Two mints never collide, and each spends independently.
#[test]
fn two_tokens_are_distinct() {
    let store = RevealTokens::new();
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let first = store.mint(a).expect("mint");
    let second = store.mint(b).expect("mint");

    assert_ne!(first, second, "tokens must not repeat");
    assert!(store.redeem(&first, a));
    assert!(store.redeem(&second, b));
}
