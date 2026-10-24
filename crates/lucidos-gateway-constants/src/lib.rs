//! The gateway constants the app reads from the generated
//! `gateway-constants.ts` (ADR 0368).

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
