use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const SEGMENT: &str = "~cap";

pub const ARTIFACT_PREVIEW_SUBJECT: &str = "artifact..preview";

pub const TTL_SECS: i64 = 3600;

pub const RENEW_AFTER_SECS: i64 = TTL_SECS / 2;

const KEY_CONTEXT: &[u8] = b"lucidos/frame-capability/v1";

const SIG_BYTES: usize = 16;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct Key([u8; 32]);

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(<redacted>)")
    }
}

pub fn derive_key(local_token: &str) -> Key {
    Key(hmac(local_token.as_bytes(), KEY_CONTEXT))
}

pub fn mint(key: &Key, slug: &str, app_id: &str, now_unix: i64, ttl_secs: i64) -> Option<String> {
    if !is_url_safe_segment(app_id) {
        return None;
    }
    let expires = now_unix.checked_add(ttl_secs)?;
    let signature = sign(key, slug, app_id, expires);
    Some(format!("{expires:x}~{app_id}~{signature}"))
}

pub fn verify(key: &Key, token: &str, slug: &str, now_unix: i64) -> Option<String> {
    let mut parts = token.split('~');
    let expires = i64::from_str_radix(parts.next()?, 16).ok()?;
    let app_id = parts.next()?;
    let presented = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if !is_url_safe_segment(app_id) || now_unix >= expires {
        return None;
    }
    if !lucidos_local_token::ct_eq(presented, &sign(key, slug, app_id, expires)) {
        return None;
    }
    Some(app_id.to_string())
}

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

pub fn admits(rest: &str, app_id: &str) -> bool {
    if app_id == ARTIFACT_PREVIEW_SUBJECT {
        return rest.starts_with("/data/artifacts/");
    }
    if rest.starts_with("/data/") {
        return true;
    }
    let Some(tail) = rest.strip_prefix("/app/") else {
        return false;
    };
    tail.split('/').next().unwrap_or("") == app_id
}

pub fn method_is_read_only(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD")
}

pub fn is_url_safe_segment(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !name.starts_with('.')
}

pub fn is_nested_frame_dest(sec_fetch_dest: Option<&str>) -> bool {
    matches!(sec_fetch_dest, Some("iframe") | Some("frame"))
}

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

        let stretched = format!("{:x}~habit-tracker~{signature}", NOW + 86_400);
        assert_eq!(verify(&key(), &stretched, "dev", NOW), None);

        let swapped = format!("{expires}~other-app~{signature}");
        assert_eq!(verify(&key(), &swapped, "dev", NOW), None);

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
            "/api/v1/threads/list",
            "/api/v1/credentials",
            "/api/v1/data/config/apis.json",
            "/control/workspaces",
            "/app/habit-tracker/index.html",
            "/app/site-publisher-two/style.css",
            "/app/",
            "/app",
            "/assets/index.js",
            "/sw.js",
            "/favicon.svg",
            "/data",
            "/database/x",
            "/",
        ] {
            assert!(!admits(rest, "site-publisher"), "{rest}");
        }
    }

    #[test]
    fn the_preview_pass_admits_the_artifacts_tree_and_nothing_else() {
        assert!(is_url_safe_segment(ARTIFACT_PREVIEW_SUBJECT));
        for rest in [
            "/data/artifacts/x.png",
            "/data/artifacts/reports/img/chart.png",
            "/data/artifacts/web/site/style.css",
        ] {
            assert!(admits(rest, ARTIFACT_PREVIEW_SUBJECT), "{rest}");
        }
        for rest in [
            "/data/config/apis.json",
            "/data/scripts/handshake.js",
            "/data/auth-modules/signer.wasm",
            "/data/knowhow/notes.md",
            "/data/apps/habit-tracker/index.html",
            "/data/artifacts",
            "/app/artifact..preview/index.html",
            "/api/v1/threads/list",
            "/",
        ] {
            assert!(!admits(rest, ARTIFACT_PREVIEW_SUBJECT), "{rest}");
        }
    }

    #[test]
    fn the_preview_pass_round_trips_like_an_app_pass() {
        let token = mint(&key(), "dev", ARTIFACT_PREVIEW_SUBJECT, NOW, TTL_SECS)
            .expect("the preview subject mints");
        assert_eq!(
            verify(&key(), &token, "dev", NOW).as_deref(),
            Some(ARTIFACT_PREVIEW_SUBJECT)
        );
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
