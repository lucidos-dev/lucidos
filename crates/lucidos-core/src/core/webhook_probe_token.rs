use std::sync::RwLock;
use std::time::{Duration, Instant};

/// INVARIANT: How long a minted token stays recognisable.
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

pub fn mint() -> std::io::Result<String> {
    let token = crate::core::webhooks::mint_token()?;
    let mut slot = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    *slot = Some((token.clone(), Instant::now() + PROBE_TOKEN_LIFETIME));
    Ok(token)
}

/// INVARIANT: Whether `presented` is the token this engine's probe is currently using.
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

#[cfg(any(test, feature = "test-support"))]
pub fn clear() {
    let mut slot = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    *slot = None;
}

#[cfg(test)]
mod tests {
    use super::*;

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
