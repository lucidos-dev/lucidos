//! The gateway constants other components must agree with: the engine reads
//! them directly, the app through the generated `gateway-constants.ts`
//! (ADR 0368).

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
