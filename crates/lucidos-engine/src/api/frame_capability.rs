//! Minting the URL pass an app frame carries to its own workspace files.
//!
//! The policy, the token format and the verification live in the crate the
//! gateway shares with us, `lucidos-frame-capability` (ADR 0238). This module
//! is the engine's half: where the key comes from, and when a mint happens.
//!
//! # Only behind a gateway, and only for a frame
//!
//! A direct-to-engine hit has no device gate to pass, so it needs no pass and
//! gets none. Nothing is stamped into the document and the bytes are what they
//! were. That is the ADR 0014 §4 shape every other rewrite here follows.
//!
//! A standalone app tab is a top-level document on a real origin, so the
//! browser sends it the device cookie with every subresource. A pass there
//! would sit in a page the user can read. It would also break the tab's links
//! an hour later, where nothing was broken before.

use axum::http::HeaderMap;
use lucidos_frame_capability as capability;

use super::base_path;
use super::local_auth;

/// The signing key, derived from the machine-local token.
///
/// Derived per call rather than cached. It is one HMAC of a short constant, and
/// only an app-document serve or a renewal pays it. Caching would mean deciding
/// what happens when the first caller runs before the token is published, which
/// is a race worth not having.
///
/// `None` on a machine with no token on disk, which is a machine with no
/// gateway. There is nothing to prove to there.
fn key() -> Option<capability::Key> {
    local_auth::machine_local_token().map(capability::derive_key)
}

/// The workspace slug this request arrived under, if it came through a gateway.
///
/// `X-Forwarded-Prefix` is forgeable on a direct hit to the engine's own port.
/// The value reaches a URL and an HTML attribute, so its shape is checked
/// first. A crafted one mints nothing, which is cheaper than escaping it.
/// Exactly `/<segment>/`, never trimmed to one. `//evil.com/` trims to a
/// perfectly slug-shaped `evil.com`, and the caller stamps the prefix VERBATIM.
/// Trimming would therefore put a protocol-relative
/// `<base href="//evil.com/…">` in the app's head, sending every relative ref
/// to another host.
fn slug_from_prefix(prefix: &str) -> Option<&str> {
    let slug = prefix.strip_prefix('/')?.strip_suffix('/')?;
    capability::is_url_safe_segment(slug).then_some(slug)
}

/// Mint a pass for an app frame, or `None` when this request needs none.
///
/// `prefix` is [`base_path::forwarded_prefix`]'s answer, so `/` means no
/// gateway.
pub(super) fn mint(prefix: &str, app_id: &str) -> Option<String> {
    let slug = slug_from_prefix(prefix)?;
    capability::mint(
        &key()?,
        slug,
        app_id,
        chrono::Utc::now().timestamp(),
        capability::TTL_SECS,
    )
}

/// Mint for a document the engine is about to serve, if it is a framed one.
///
/// The framed test is `Sec-Fetch-Dest`, which a browser sets and page script
/// cannot forge. A client sending no fetch metadata gets nothing, and loads
/// exactly as it does today.
pub(super) fn mint_for_document(headers: &HeaderMap, app_id: &str) -> Option<String> {
    let dest = headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok());
    if !capability::is_nested_frame_dest(dest) {
        return None;
    }
    mint(&base_path::forwarded_prefix(headers), app_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(prefix: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("sec-fetch-dest", "iframe".parse().unwrap());
        headers.insert("x-forwarded-prefix", prefix.parse().unwrap());
        headers
    }

    #[test]
    fn a_framed_document_behind_a_gateway_gets_a_pass() {
        let token = local_auth::publish_test_local_token();
        let minted = mint_for_document(&framed("/dev/"), "site-publisher")
            .expect("a framed document behind a gateway mints");
        let key = capability::derive_key(token);
        assert_eq!(
            capability::verify(&key, &minted, "dev", chrono::Utc::now().timestamp()).as_deref(),
            Some("site-publisher"),
            "the gateway must read back what we minted"
        );
    }

    #[test]
    fn nothing_is_minted_where_nothing_needs_proving() {
        local_auth::publish_test_local_token();

        // No gateway: the engine's own port asks for no credential.
        let mut direct = framed("/dev/");
        direct.remove("x-forwarded-prefix");
        assert_eq!(mint_for_document(&direct, "site-publisher"), None);

        // A standalone app tab. Real origin, so the device cookie still flows.
        let mut tab = framed("/dev/");
        tab.insert("sec-fetch-dest", "document".parse().unwrap());
        assert_eq!(mint_for_document(&tab, "site-publisher"), None);

        // A client with no fetch metadata is not a browser.
        let mut bare = framed("/dev/");
        bare.remove("sec-fetch-dest");
        assert_eq!(mint_for_document(&bare, "site-publisher"), None);
    }

    #[test]
    fn a_crafted_forwarded_prefix_mints_nothing() {
        // The header is the gateway's on a proxied request, and forgeable on a
        // direct one. A shape that is not a slug is refused whole.
        local_auth::publish_test_local_token();
        for prefix in [
            "/",
            "//",
            // The sharp one. Trimmed rather than matched, this reads as the
            // slug `evil.com`, and the base it stamps is protocol-relative.
            "//evil.com/",
            "///evil.com/",
            "/a\"onload=x/",
            "/deep/nested/",
            "/with space/",
            "/../",
        ] {
            assert_eq!(
                mint_for_document(&framed(prefix), "site-publisher"),
                None,
                "{prefix}"
            );
        }
    }

    #[test]
    fn an_app_id_that_is_not_a_slug_mints_nothing() {
        local_auth::publish_test_local_token();
        assert_eq!(mint_for_document(&framed("/dev/"), "a b"), None);
        assert_eq!(mint_for_document(&framed("/dev/"), ""), None);
    }
}
