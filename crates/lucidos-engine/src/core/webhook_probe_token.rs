//! The bearer the ingress probe presents, so a delivery can recognise its own.
//!
//! ADR 0143 explains why this exists. The probe POSTs to a real hook with a
//! credential that cannot verify, which is a refusal like any other. Left
//! alone, it would stamp `last_refused_at` every 15 minutes and bury the one
//! signal those columns exist to expose.
//!
//! So the probe mints a token here before each cycle and sends it as the
//! bearer. `api/webhooks.rs::deliver` asks [`is_probe_token`] before stamping,
//! refuses the delivery exactly as it refuses anything else, and skips only
//! the timestamp. Nothing changes on the wire.
//!
//! The token lives in memory alone, which the statelessness rule allows: an
//! engine restart mid-cycle costs one stray refusal stamp, and the next cycle
//! mints again.

use std::sync::RwLock;
use std::time::{Duration, Instant};

/// How long a minted token stays recognisable.
///
/// A cycle probes its addresses one at a time. The window has to cover every
/// request, plus whatever the funnel still forwards after the last one timed
/// out. Five minutes covers that and stays inside the 15 minute interval, so no
/// two cycles are live at once.
///
/// It expires on purpose. A token nobody is using must stop matching, or one
/// leak silences the refusal stamp for as long as the engine runs.
const PROBE_TOKEN_LIFETIME: Duration = Duration::from_secs(300);

static CURRENT: RwLock<Option<(String, Instant)>> = RwLock::new(None);

/// Mint the token for one probe cycle, replacing any earlier one.
pub fn mint() -> std::io::Result<String> {
    let token = crate::core::webhooks::mint_token()?;
    let mut slot = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    *slot = Some((token.clone(), Instant::now() + PROBE_TOKEN_LIFETIME));
    Ok(token)
}

/// Whether `presented` is the token this engine's probe is currently using.
///
/// Compared in constant time, and only against a token that has not expired.
pub fn is_probe_token(presented: &str) -> bool {
    let slot = CURRENT.read().unwrap_or_else(|e| e.into_inner());
    let Some((token, expires_at)) = slot.as_ref() else {
        return false;
    };
    if Instant::now() >= *expires_at {
        return false;
    }
    crate::core::webhooks::ct_eq(presented, token)
}

/// Forget the current token.
///
/// Tests only. A cycle leaves its token to expire on its own. A probe that
/// timed out here may still be in flight through the funnel. Clearing early is
/// what makes that late arrival stamp a refusal.
#[cfg(test)]
pub fn clear() {
    let mut slot = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    *slot = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The static is process-wide, so these run under one lock rather than as
    /// separate `#[test]` functions racing each other.
    #[test]
    fn a_minted_token_matches_itself_and_nothing_else() {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

        clear();
        assert!(!is_probe_token("anything"), "nothing matches before a mint");

        let token = mint().expect("mint");
        assert!(is_probe_token(&token));
        assert!(!is_probe_token("some other bearer"));
        assert!(!is_probe_token(""));

        let second = mint().expect("mint again");
        assert_ne!(token, second, "each cycle gets fresh entropy");
        assert!(
            !is_probe_token(&token),
            "a superseded token stops matching, so one leak cannot last"
        );
        assert!(is_probe_token(&second));

        clear();
        assert!(!is_probe_token(&second), "a cleared token stops matching");
    }
}
