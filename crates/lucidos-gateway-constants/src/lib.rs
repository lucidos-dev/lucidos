//! The gateway constants other components must agree with: the engine reads
//! them directly, the app through the generated `gateway-constants.ts`
//! (ADR 0368).

/// The `chrono` format that starts every engine and gateway log line: RFC 3339
/// with the local UTC offset, such as `2026-11-11T21:25:19+01:00`. A line in
/// the host log then names its day and zone on any install.
pub const LOG_STAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z";

/// Digits in a pairing code. Short enough to type on a phone. Its safety comes
/// from the TTL, single use and a local-only channel, not from the digits.
pub const PAIRING_CODE_DIGITS: u32 = 8;

/// The prefix of a device pairing secret, the long secret a QR or link carries.
pub const DEVICE_PAIRING_SECRET_PREFIX: &str = "ldp_";

/// The prefix of a gateway pairing secret, which one gateway pastes into
/// another.
pub const GATEWAY_PAIRING_SECRET_PREFIX: &str = "lgp_";

/// Lowercase base32 characters after a pairing secret's prefix. 26 of them
/// carry 130 random bits, which no flood can guess.
pub const PAIRING_SECRET_CHARS: usize = 26;

/// The URL parameter carrying the code to the pairing screen.
///
/// `crates/lucidos-app/src/utils/pairingCodeSeed.ts` reads it, then strips it
/// from the address bar.
pub const PAIR_PARAM: &str = "pair";

/// The reserved sigil that prefixes all gateway-owned paths (`/~/…`). A
/// workspace slug can never start with it.
pub const SIGIL: char = '~';

/// The gateway's own health. Public, so a probe presents no credential. Its
/// `instance_id` is what proves which gateway answered at an address.
pub const GATEWAY_HEALTH_PATH: &str = "/~/api/v1/health";

/// Joins a workspace slug to a remote gateway's short name in a route:
/// `/<slug>@<name>/` (ADR 0591). A slug never contains it.
pub const REMOTE_ROUTE_SEPARATOR: char = '@';

/// The longest short name a remote gateway may take, since it becomes part of
/// every route to its workspaces.
pub const REMOTE_GATEWAY_NAME_MAX_LEN: usize = 32;

/// The loopback port an OAuth provider redirects to. Fixed, because the
/// redirect URI is registered with the provider ahead of time. The gateway
/// owns it, and an engine binds it only with no gateway.
pub const OAUTH_CALLBACK_PORT: u16 = 14981;

/// The path the loopback OAuth callback answers on.
pub const OAUTH_CALLBACK_PATH: &str = "/oauth/callback";

/// Separates the routing prefix from the nonce in a gateway-mode OAuth
/// `state` (`/dev/~<nonce>`). A nonce is base64url and never holds it, so the
/// LAST one splits the value.
pub const OAUTH_STATE_PREFIX_SEPARATOR: char = '~';

/// The engine route a gateway delivers an OAuth callback to, relative to the
/// engine's `/api/v1` mount.
pub const OAUTH_DELIVERY_ROUTE: &str = "/internal/oauth/callback";

/// The path a *web redirect URI* names on a gateway's own origin:
/// `https://<origin>/~/oauth/callback`. Public, since a provider sends the
/// browser there with no credential.
pub const OAUTH_WEB_CALLBACK_PATH: &str = "/~/oauth/callback";

/// Stamped by the gateway facing the browser on a request that reached it
/// straight from this machine. The engine then offers the loopback redirect.
/// The gateway strips a client's copy on every hop.
pub const HEADER_LOCAL_BROWSER: &str = "x-lucidos-local-browser";

/// Prefix for databases provisioned by the gateway's shared Postgres cluster.
pub const WORKSPACE_DATABASE_PREFIX: &str = "lucidos_";

/// Registry routing keys, including legacy ids up to 64 bytes.
pub fn is_valid_workspace_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// Do two display names name the same workspace? Trimmed and case-insensitive:
/// "Work" and "work" are no more tellable apart in a list than two equal ones.
pub fn display_names_match(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Derive a stable, filesystem- and URL-safe slug from a display name:
/// lowercase, non-alphanumerics collapsed to single `-`, leading/trailing `-`
/// trimmed. Empty input (or all-punctuation) falls back to `workspace`. The
/// output is always `[a-z0-9-]`, so it can never start with the gateway sigil.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "workspace".to_string()
    } else {
        out
    }
}

/// The database authority shared by gateway provisioning and offline restore.
pub fn workspace_database_name(
    id: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if !is_valid_workspace_id(id) {
        return Err(format!("invalid workspace id for database name: '{id}'").into());
    }
    Ok(format!("{WORKSPACE_DATABASE_PREFIX}{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_slug_contract_and_database_mapping_are_shared() {
        for slug in ["demo", "e2e-test", "a1", &"a".repeat(64)] {
            assert!(is_valid_workspace_id(slug));
            assert_eq!(
                workspace_database_name(slug).unwrap(),
                format!("lucidos_{slug}")
            );
        }
        for slug in [
            "",
            "../demo",
            "Demo",
            "demo@remote",
            "-demo",
            "demo-",
            &"a".repeat(65),
        ] {
            assert!(!is_valid_workspace_id(slug));
            assert!(workspace_database_name(slug).is_err());
        }
    }
}
