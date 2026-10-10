//! The row registry, without a pool.

use super::*;

/// A stored value round-trips through its id, so Settings and the engine
/// agree on every row.
#[test]
fn every_row_reads_back_from_its_id() {
    for endpoint in SystemOneEndpoint::ALL {
        assert_eq!(SystemOneEndpoint::from_id(endpoint.id()), Some(endpoint));
    }
}

/// `chat` is a site's default, never a System One row.
#[test]
fn chat_is_not_a_row() {
    assert_eq!(SystemOneEndpoint::from_id("chat"), None);
}

#[test]
fn clef_and_clef_flash_share_cloudflares_switch_and_token() {
    assert_eq!(
        SystemOneEndpoint::Clef.switch_key().key(),
        SystemOneEndpoint::ClefFlash.switch_key().key()
    );
    assert_eq!(
        SystemOneEndpoint::Clef.credential_service(),
        SystemOneEndpoint::ClefFlash.credential_service()
    );
    assert_ne!(
        SystemOneEndpoint::Clef.switch_key().key(),
        SystemOneEndpoint::Jev.switch_key().key()
    );
}

#[test]
fn a_workers_ai_scope_yields_the_model_url() {
    assert_eq!(
        workers_ai_url(
            "https://api.cloudflare.com/client/v4/accounts/abc123/ai",
            "clef"
        )
        .as_deref(),
        Some("https://api.cloudflare.com/client/v4/accounts/abc123/ai/run/@cf/cloudflare/clef")
    );
    assert_eq!(
        workers_ai_url(
            " https://api.cloudflare.com/client/v4/accounts/abc123/ai/ ",
            "clef-flash"
        )
        .as_deref(),
        Some(
            "https://api.cloudflare.com/client/v4/accounts/abc123/ai/run/@cf/cloudflare/clef-flash"
        ),
        "a trailing slash and padding are forgiven"
    );
}

/// The token is sent to whatever URL this returns, so anything but the
/// Cloudflare API with a plain account id is refused.
#[test]
fn a_scope_naming_no_account_yields_nothing() {
    for scope in [
        "",
        "https://api.cloudflare.com/client/v4/accounts//ai",
        "https://api.cloudflare.com/client/v4/accounts/abc123",
        "https://api.cloudflare.com/client/v4/accounts/abc/../x/ai",
        "https://evil.example/client/v4/accounts/abc123/ai",
        "http://api.cloudflare.com/client/v4/accounts/abc123/ai",
    ] {
        assert_eq!(workers_ai_url(scope, "clef"), None, "{scope:?}");
    }
}
