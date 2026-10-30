//! The gateway constants other components must agree with: the engine reads
//! them directly, the app through the generated `gateway-constants.ts`
//! (ADR 0368).

/// Digits in a pairing code. Short enough to type on a phone. Its safety comes
/// from the TTL and single use, not from the digits alone.
pub const PAIRING_CODE_DIGITS: u32 = 8;

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
