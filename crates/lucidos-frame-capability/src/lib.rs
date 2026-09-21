//! The frame capability: a short-lived, read-only URL pass to an app frame's
//! own workspace files.
//!
//! Full design: [ADR 0238](../../../docs/adr/0238-app-frame-carries-a-capability-to-its-own-files.md).
//!
//! # The problem it solves
//!
//! An app frame runs at an opaque origin (ADR 0227), so its site-for-cookies is
//! null. The browser therefore sends no `SameSite=Lax` device credential with
//! any subresource of the frame's document. Behind a gateway that means the
//! app's own `style.css`, its images and everything `lucidos.data.url(path)`
//! builds are refused. The `/api/v1` assets were exempted by name. Workspace
//! content cannot be: an exemption by name would hand an unpaired device the
//! user's artifacts.
//!
//! So the browser carries proof instead, in the URL, as one path segment.

//! # Why a path segment and not a query
//!
//! Relative resolution carries a path prefix down every hop for free. The
//! nested case is the one that matters. An app previews an artifact in an
//! iframe, and that artifact is an HTML document. Its own `./next/index.html`
//! resolves against its URL, with no query on it. A path prefix survives that
//! and a query does not. An app also concatenates onto the result
//! (`lucidos.data.url(src) + '?t=' + Date.now()`), which a query-borne pass
//! turns into a URL with two `?`.

//! # What it is not
//!
//! It is not a second copy of the device credential. It reads two sub-trees of
//! one workspace, with GET and HEAD, for an hour. It reaches no `/api/v1`
//! route, no control plane and nothing under the picker's namespace. The
//! caller composing it enforces that last part with [`admits`].
//!
//! Unrelated to the *capability parity manifest*, which is a different thing
//! with a similar word in it.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// The path segment that introduces a capability: `/<slug>/~cap/<token>/…`.
///
/// A tilde because the gateway already reserves that shape for its own
/// namespace (`/~/`), so no workspace route can collide with it.
pub const SEGMENT: &str = "~cap";

/// How long a freshly minted capability lasts.
///
/// An hour bounds a leaked URL to one working session. It costs nothing to the
/// frame itself: the host re-mints at half-life and pushes the new value, so a
/// frame that stays open stays live.
pub const TTL_SECS: i64 = 3600;

/// Re-mint this long after minting, which is half the life.
pub const RENEW_AFTER_SECS: i64 = TTL_SECS / 2;

/// The domain separator, so the signing key is derived from the machine-local
/// token rather than being it. A leaked capability then yields no token.
const KEY_CONTEXT: &[u8] = b"lucidos/frame-capability/v1";

/// How many bytes of the HMAC ride in the URL. 128 bits is far past what an
/// hour-long, read-only, loopback-reachable pass needs, and it keeps the
/// segment short enough to read in a log.
const SIG_BYTES: usize = 16;

type HmacSha256 = Hmac<Sha256>;

/// The signing key, derived from the machine-local token.
#[derive(Clone)]
pub struct Key([u8; 32]);

impl std::fmt::Debug for Key {
    /// Never print the bytes. This lands in a `#[derive(Debug)]` struct sooner
    /// or later, and from there in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(<redacted>)")
    }
}

/// Derive the signing key from the machine-local token.
///
/// Both processes read the same mode 0600 file (`lucidos-local-token`), which
/// the gateway mints before it spawns any engine. So the two agree with no
/// handshake, no shared state and nothing to lose across a restart of either.
pub fn derive_key(local_token: &str) -> Key {
    Key(hmac(local_token.as_bytes(), KEY_CONTEXT))
}

/// Mint a capability for one app frame of one workspace.
///
/// `None` when the app id is not URL-safe. The id reaches a URL path segment,
/// where it is compared as raw bytes. Refusing an exotic one is simpler than
/// encoding it, and it keeps the gateway free of a percent-decoder. Such an app
/// loads exactly as it does today, which is the status quo rather than a new
/// break.
pub fn mint(key: &Key, slug: &str, app_id: &str, now_unix: i64, ttl_secs: i64) -> Option<String> {
    if !is_url_safe_segment(app_id) {
        return None;
    }
    let expires = now_unix.checked_add(ttl_secs)?;
    let signature = sign(key, slug, app_id, expires);
    Some(format!("{expires:x}~{app_id}~{signature}"))
}

/// Read a capability back, answering which app it was minted for.
///
/// `None` for a token this key did not sign, one for another workspace, one
/// that has expired, and one that is malformed in any way. The caller then has
/// nothing to branch on but the app id, which is the point: every refusal looks
/// the same from outside.
pub fn verify(key: &Key, token: &str, slug: &str, now_unix: i64) -> Option<String> {
    let mut parts = token.split('~');
    let expires = i64::from_str_radix(parts.next()?, 16).ok()?;
    let app_id = parts.next()?;
    let presented = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    // Re-checked on the way in, not just on the way out. A future mint that
    // widened the charset must not be readable by a verifier that did not.
    if !is_url_safe_segment(app_id) || now_unix >= expires {
        return None;
    }
    if !lucidos_local_token::ct_eq(presented, &sign(key, slug, app_id, expires)) {
        return None;
    }
    Some(app_id.to_string())
}

/// Split `/~cap/<token>/<rest>` into the token and `/<rest>`.
///
/// `None` when the path carries no capability, and also when it carries one
/// with nothing after it. A bare `/~cap/<token>` addresses no file, so there is
/// nothing to admit.
pub fn split(path: &str) -> Option<(&str, &str)> {
    let after = path
        .strip_prefix('/')?
        .strip_prefix(SEGMENT)?
        .strip_prefix('/')?;
    let (token, rest) = after.split_once('/')?;
    if token.is_empty() || rest.is_empty() {
        return None;
    }
    Some((token, &after[token.len()..]))
}

/// May a capability minted for `app_id` reach this path?
///
/// `rest` is what [`split`] returned, so it begins with `/`. Two sub-trees and
/// no others:
///
///  * `/data/…`, the workspace's own files. An app frame already reads that
///    whole tree through `lucidos.data.read`, over the host bridge (ADR 0231).
///    So this grants no reach the app lacked. It grants it as a URL.
///  * `/app/<app_id>/…`, the app's own bundle, and only its own.
///
/// Everything else is refused, `/api/v1` first among them. Widening `/api/v1`
/// would put the whole engine API behind a pass the frame hands to any document
/// it embeds.
pub fn admits(rest: &str, app_id: &str) -> bool {
    if rest.starts_with("/data/") {
        return true;
    }
    let Some(tail) = rest.strip_prefix("/app/") else {
        return false;
    };
    tail.split('/').next().unwrap_or("") == app_id
}

/// May a capability carry this method? A read is the whole grant.
///
/// `HEAD` rides along because it is a `GET` that answers no body, and a browser
/// issues one for a range probe without asking anybody.
pub fn method_is_read_only(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD")
}

/// A name that survives a URL path segment untouched.
///
/// Asked of an app id before minting, and of a workspace slug by the engine
/// before it builds a URL out of one.
///
/// `apps::is_valid_id` is wider: it bans `/`, `\`, `..` and a leading dot, and
/// allows the rest. So it admits a space, which a browser percent-encodes, and
/// a `~`, which is this token's own separator. Both would make the raw
/// comparison in [`admits`] answer the wrong question.
pub fn is_url_safe_segment(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !name.starts_with('.')
}

/// Is this `Sec-Fetch-Dest` a nested browsing context?
///
/// Both processes ask it, about the same request, for reasons that must not
/// drift. The engine mints only for a FRAMED document, since a standalone app
/// tab has a real origin and keeps the device cookie. The gateway renders a
/// refused frame a short page rather than the pairing shell.
///
/// `None` means a client sending no fetch metadata, which is not a browser.
pub fn is_nested_frame_dest(sec_fetch_dest: Option<&str>) -> bool {
    matches!(sec_fetch_dest, Some("iframe") | Some("frame"))
}

/// The signature, as lowercase hex.
///
/// Every field is length-framed, so no two different triples can build the same
/// message. Without that, slug `a` with app `bc` and slug `ab` with app `c`
/// would sign identically, and a capability for one workspace would open
/// another.
fn sign(key: &Key, slug: &str, app_id: &str, expires: i64) -> String {
    let mut message = Vec::with_capacity(slug.len() + app_id.len() + 40);
    message.extend_from_slice(KEY_CONTEXT);
    for field in [slug.as_bytes(), app_id.as_bytes()] {
        message.extend_from_slice(&(field.len() as u32).to_be_bytes());
        message.extend_from_slice(field);
    }
    message.extend_from_slice(&expires.to_be_bytes());
    hex(&hmac(&key.0, &message)[..SIG_BYTES])
}

fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts a key of any length");
    mac.update(message);
    mac.finalize().into_bytes().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    fn key() -> Key {
        derive_key("a-machine-local-token")
    }

    fn fresh(app_id: &str) -> String {
        mint(&key(), "dev", app_id, NOW, TTL_SECS).expect("a slug-shaped app id mints")
    }

    #[test]
    fn a_minted_capability_reads_back_as_the_app_it_names() {
        let token = fresh("site-publisher");
        assert_eq!(
            verify(&key(), &token, "dev", NOW).as_deref(),
            Some("site-publisher")
        );
    }

    #[test]
    fn the_signing_key_is_derived_rather_than_the_token_itself() {
        // A capability is handed to every document the frame embeds, so it
        // leaks the way a URL leaks. It must not be a step towards the
        // machine-local token, which authorizes the whole engine API.
        let token = fresh("habit-tracker");
        assert!(
            !token.contains("a-machine-local-token"),
            "the token must not carry the secret: {token}"
        );
        assert_ne!(
            verify(&derive_key("another-machine"), &token, "dev", NOW),
            Some("habit-tracker".to_string()),
            "another machine's key must not read it"
        );
    }

    #[test]
    fn another_workspace_cannot_use_it() {
        // The signing key is machine-wide. So the slug in the signed message is
        // the only thing keeping one workspace's frame out of the next one's
        // artifacts.
        let token = fresh("habit-tracker");
        assert_eq!(verify(&key(), &token, "work", NOW), None);
    }

    #[test]
    fn a_capability_expires() {
        let token = fresh("habit-tracker");
        assert!(verify(&key(), &token, "dev", NOW + TTL_SECS - 1).is_some());
        assert_eq!(verify(&key(), &token, "dev", NOW + TTL_SECS), None);
        assert_eq!(verify(&key(), &token, "dev", NOW + TTL_SECS + 86_400), None);
    }

    #[test]
    fn a_tampered_capability_is_refused() {
        let token = fresh("habit-tracker");
        let (expires, rest) = token.split_once('~').unwrap();
        let (_, signature) = rest.split_once('~').unwrap();

        // A later expiry the signature does not cover.
        let stretched = format!("{:x}~habit-tracker~{signature}", NOW + 86_400);
        assert_eq!(verify(&key(), &stretched, "dev", NOW), None);

        // Another app id under the same signature. This is the one that would
        // read a sibling app's source.
        let swapped = format!("{expires}~other-app~{signature}");
        assert_eq!(verify(&key(), &swapped, "dev", NOW), None);

        // One flipped hex digit in the signature.
        let mut bent: Vec<char> = token.chars().collect();
        let last = bent.len() - 1;
        bent[last] = if bent[last] == 'a' { 'b' } else { 'a' };
        assert_eq!(
            verify(&key(), &bent.into_iter().collect::<String>(), "dev", NOW),
            None
        );
    }

    #[test]
    fn a_malformed_capability_is_refused_rather_than_guessed() {
        for token in [
            "",
            "~",
            "~~",
            "notahexnumber~app~0011223344556677",
            "6a0b~habit-tracker",
            "6a0b~habit-tracker~short",
            "6a0b~habit-tracker~0011223344556677~extra",
            "-1~habit-tracker~00112233445566778899aabbccddeeff",
        ] {
            assert_eq!(verify(&key(), token, "dev", NOW), None, "{token}");
        }
    }

    #[test]
    fn an_app_id_that_does_not_survive_a_url_segment_mints_nothing() {
        // Refused whole rather than encoded, so the gateway needs no decoder
        // and the comparison in `admits` stays a byte comparison.
        for app_id in ["", "with space", "a~tilde", "a%2fslash", ".dotfile", "a/b"] {
            assert_eq!(mint(&key(), "dev", app_id, NOW, TTL_SECS), None, "{app_id}");
        }
        for app_id in ["site-publisher", "habit_tracker", "app.v2", "a"] {
            assert!(
                mint(&key(), "dev", app_id, NOW, TTL_SECS).is_some(),
                "{app_id}"
            );
        }
    }

    #[test]
    fn the_capability_segment_splits_off_the_path_it_guards() {
        assert_eq!(
            split("/~cap/6a0b~app~00/data/artifacts/x.png"),
            Some(("6a0b~app~00", "/data/artifacts/x.png"))
        );
        assert_eq!(
            split("/~cap/tok/app/habit-tracker/"),
            Some(("tok", "/app/habit-tracker/"))
        );
    }

    #[test]
    fn a_path_with_no_capability_splits_to_nothing() {
        for path in [
            "/data/artifacts/x.png",
            "/~cap",
            "/~cap/",
            "/~cap/tok",
            "/~cap/tok/",
            "/~cap//data/x",
            "/~capybara/tok/data/x",
            "/api/v1/~cap/tok/data/x",
        ] {
            assert_eq!(split(path), None, "{path}");
        }
    }

    #[test]
    fn a_capability_admits_the_workspace_tree_and_its_own_app() {
        for rest in [
            "/data/artifacts/x.png",
            "/data/artifacts/web/lucidos-me/index.html",
            // One hop down from the artifact above: the relative click inside a
            // previewed document, which is the case this design exists for.
            "/data/artifacts/web/lucidos-me/five-ai-words/index.html",
            "/app/site-publisher",
            "/app/site-publisher/",
            "/app/site-publisher/style.css",
            "/app/site-publisher/artifacts/chart.png",
        ] {
            assert!(admits(rest, "site-publisher"), "{rest}");
        }
    }

    #[test]
    fn a_capability_admits_nothing_else() {
        for rest in [
            // The control plane and the whole engine API.
            "/api/v1/threads/list",
            "/api/v1/credentials",
            "/api/v1/data/config/apis.json",
            "/control/workspaces",
            // Another app's bundle, including one whose name merely starts the
            // same way.
            "/app/habit-tracker/index.html",
            "/app/site-publisher-two/style.css",
            "/app/",
            "/app",
            // The frontend bundle and the service worker.
            "/assets/index.js",
            "/sw.js",
            "/favicon.svg",
            // `/data` without a tree under it is the API's list route, not the
            // static mount.
            "/data",
            "/database/x",
            "/",
        ] {
            assert!(!admits(rest, "site-publisher"), "{rest}");
        }
    }

    #[test]
    fn a_nested_frame_is_told_apart_from_everything_else() {
        assert!(is_nested_frame_dest(Some("iframe")));
        assert!(is_nested_frame_dest(Some("frame")));
        for dest in ["document", "script", "style", "image", "font", "empty", ""] {
            assert!(!is_nested_frame_dest(Some(dest)), "{dest}");
        }
        assert!(!is_nested_frame_dest(None));
    }

    #[test]
    fn only_a_read_rides_a_capability() {
        assert!(method_is_read_only("GET"));
        assert!(method_is_read_only("head"));
        for method in ["POST", "PUT", "DELETE", "PATCH", "OPTIONS"] {
            assert!(!method_is_read_only(method), "{method}");
        }
    }
}
