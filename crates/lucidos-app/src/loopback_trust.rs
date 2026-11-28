//! The Trust step before the window opens on https (ADR 0745).
//!
//! The gateway serves loopback with a leaf from the per-user loopback CA, and
//! WKWebView trusts only the login keychain. So `desktop::launch` holds the
//! window on the boot splash until the CA is trusted there. The splash shows
//! the Trust screen through `startup_status`, and each attempt is the user's
//! own press: nothing here retries.

use std::sync::{Condvar, Mutex};

use lucidos_loopback_ca::trust::{self, TrustStatus};
use lucidos_loopback_ca::{CaDir, LoopbackCa};

use crate::desktop::StartupReport;

/// Where the Trust step stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustGate {
    /// Nothing has asked yet, or the check is running.
    Checking,
    /// The window waits for the user's press. `detail` says why the last
    /// check or attempt fell short, as a sentence.
    NeedsTrust {
        detail: Option<String>,
    },
    /// The press is being served: macOS shows its password prompt.
    Trusting,
    Trusted,
}

impl TrustGate {
    /// The gate after a status check. `Unavailable` needs the user too: its
    /// message names the cure, and the press runs the check again.
    pub fn from_status(status: TrustStatus) -> TrustGate {
        match status {
            TrustStatus::Trusted => TrustGate::Trusted,
            TrustStatus::Untrusted(_) => TrustGate::NeedsTrust { detail: None },
            TrustStatus::Unavailable(why) => TrustGate::NeedsTrust {
                detail: Some(sentence(why)),
            },
        }
    }

    /// The gate after a press: the attempt's own failure wins, since it names
    /// what went wrong. A store that still disagrees after a clean attempt is
    /// reported in its own words.
    pub fn after_attempt(attempt: Result<(), String>, status: TrustStatus) -> TrustGate {
        match (attempt, status) {
            (Err(why), _) => TrustGate::NeedsTrust {
                detail: Some(sentence(why)),
            },
            (Ok(()), TrustStatus::Trusted) => TrustGate::Trusted,
            (Ok(()), TrustStatus::Untrusted(why) | TrustStatus::Unavailable(why)) => {
                TrustGate::NeedsTrust {
                    detail: Some(sentence(why)),
                }
            }
        }
    }
}

/// End `text` as a sentence, so the screen can follow it with another.
fn sentence(text: String) -> String {
    let text = text.trim_end();
    if text.ends_with(['.', '!', '?']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

const TRUST_LABEL: &str = "Lucidos keeps its connection on this Mac secure with its own \
     certificate.\nTrust the certificate once to continue. macOS asks for your password.";
const TRUST_BUTTON: &str = "Trust certificate";
const RETRY_BUTTON: &str = "Try again";
const TRUSTING_LABEL: &str = "Waiting for macOS to trust the local certificate…";

/// The splash's Trust screen for `gate`, or `None` when it shows none.
pub fn trust_prompt(gate: &TrustGate) -> Option<StartupReport> {
    match gate {
        TrustGate::Checking | TrustGate::Trusted => None,
        TrustGate::NeedsTrust { detail: None } => Some(StartupReport {
            label: TRUST_LABEL.to_string(),
            trust_button: Some(TRUST_BUTTON),
        }),
        TrustGate::NeedsTrust {
            detail: Some(detail),
        } => Some(StartupReport {
            label: format!("{detail}\nLucidos opens once its local certificate is trusted."),
            trust_button: Some(RETRY_BUTTON),
        }),
        TrustGate::Trusting => Some(StartupReport {
            label: TRUSTING_LABEL.to_string(),
            trust_button: None,
        }),
    }
}

/// The gate, shared by the launch thread that waits on it and the command the
/// press calls. Managed as Tauri state.
#[derive(Default)]
pub struct LoopbackTrust {
    gate: Mutex<Option<TrustGate>>,
    changed: Condvar,
}

impl LoopbackTrust {
    fn set(&self, gate: TrustGate) {
        *self.gate.lock().unwrap_or_else(|e| e.into_inner()) = Some(gate);
        self.changed.notify_all();
    }

    /// Check once, then block until the CA is trusted. Returns at once when it
    /// already is. Waiting is all this does: only a press moves the gate.
    pub fn wait_until_trusted(&self, check: impl FnOnce() -> TrustStatus) {
        self.set(TrustGate::Checking);
        self.set(TrustGate::from_status(check()));
        let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        while gate.as_ref() != Some(&TrustGate::Trusted) {
            gate = self.changed.wait(gate).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Claim the press. False unless the gate waits for one, so a second press
    /// during the password prompt does nothing.
    pub fn begin_attempt(&self) -> bool {
        let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*gate, Some(TrustGate::NeedsTrust { .. })) {
            return false;
        }
        *gate = Some(TrustGate::Trusting);
        true
    }

    pub fn finish_attempt(&self, attempt: Result<(), String>, status: TrustStatus) {
        self.set(TrustGate::after_attempt(attempt, status));
    }

    /// The Trust screen, while the gate shows one.
    pub fn prompt(&self) -> Option<StartupReport> {
        let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        gate.as_ref().and_then(trust_prompt)
    }
}

/// The CA directory, created with its CA on first use. The gateway creates it
/// too, and the CA's own lock lets the first creator win.
fn ensure_ca() -> Result<CaDir, String> {
    let dir = CaDir::resolve().ok_or_else(|| {
        "Lucidos cannot find its private folder because HOME is not set. Set HOME and \
         open Lucidos again"
            .to_string()
    })?;
    LoopbackCa::ensure(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Whether this user's keychain trusts the loopback CA, creating the CA first.
pub fn check_loopback_trust() -> TrustStatus {
    match ensure_ca() {
        Ok(dir) => trust::status(&dir),
        Err(why) => TrustStatus::Unavailable(why),
    }
}

/// The Trust button: trust the CA in the login keychain, then ask the keychain
/// again. Blocks while macOS shows its password prompt.
#[tauri::command]
pub async fn trust_loopback_ca(trust_gate: tauri::State<'_, LoopbackTrust>) -> Result<(), String> {
    if !trust_gate.begin_attempt() {
        return Ok(());
    }
    let (attempt, status) = tauri::async_runtime::spawn_blocking(|| {
        let attempt = ensure_ca().and_then(|dir| trust::trust(&dir).map_err(|e| e.to_string()));
        (attempt, check_loopback_trust())
    })
    .await
    // A panicked attempt still ends, so the splash offers the button again.
    .unwrap_or_else(|e| {
        let why = format!("the trust step stopped unexpectedly ({e})");
        (Err(why.clone()), TrustStatus::Unavailable(why))
    });
    trust_gate.finish_attempt(attempt.clone(), status);
    attempt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn untrusted() -> TrustStatus {
        TrustStatus::Untrusted("the login keychain does not trust it yet".into())
    }

    #[test]
    fn a_trusted_ca_shows_no_screen() {
        assert_eq!(
            TrustGate::from_status(TrustStatus::Trusted),
            TrustGate::Trusted
        );
        assert_eq!(trust_prompt(&TrustGate::Trusted), None);
        assert_eq!(trust_prompt(&TrustGate::Checking), None);
    }

    #[test]
    fn an_untrusted_ca_asks_for_the_press() {
        let gate = TrustGate::from_status(untrusted());
        assert_eq!(gate, TrustGate::NeedsTrust { detail: None });
        let prompt = trust_prompt(&gate).unwrap();
        assert_eq!(prompt.trust_button, Some(TRUST_BUTTON));
        assert!(prompt.label.contains("password"), "{}", prompt.label);
    }

    #[test]
    fn an_unavailable_store_names_its_cure_beside_the_retry() {
        let gate = TrustGate::from_status(TrustStatus::Unavailable("cannot run `security`".into()));
        let prompt = trust_prompt(&gate).unwrap();
        assert_eq!(prompt.trust_button, Some(RETRY_BUTTON));
        assert!(
            prompt.label.starts_with("cannot run `security`.\n"),
            "{}",
            prompt.label
        );
    }

    #[test]
    fn a_failed_attempt_reports_its_own_words() {
        let gate = TrustGate::after_attempt(
            Err("`security add-trusted-cert` failed: User canceled. Run `lucidos tls trust` to try again.".into()),
            untrusted(),
        );
        let prompt = trust_prompt(&gate).unwrap();
        assert!(prompt
            .label
            .starts_with("`security add-trusted-cert` failed: User canceled."));
        assert_eq!(prompt.trust_button, Some(RETRY_BUTTON));
    }

    #[test]
    fn a_clean_attempt_the_store_still_refuses_says_so() {
        let gate = TrustGate::after_attempt(Ok(()), untrusted());
        assert_eq!(
            gate,
            TrustGate::NeedsTrust {
                detail: Some("the login keychain does not trust it yet.".into())
            }
        );
        assert_eq!(
            TrustGate::after_attempt(Ok(()), TrustStatus::Trusted),
            TrustGate::Trusted
        );
    }

    #[test]
    fn the_password_prompt_shows_no_button() {
        let prompt = trust_prompt(&TrustGate::Trusting).unwrap();
        assert_eq!(prompt.trust_button, None);
    }

    #[test]
    fn only_a_waiting_gate_takes_a_press() {
        let trust_gate = LoopbackTrust::default();
        assert!(!trust_gate.begin_attempt(), "no press before the check");
        trust_gate.set(TrustGate::NeedsTrust { detail: None });
        assert!(trust_gate.begin_attempt());
        assert!(
            !trust_gate.begin_attempt(),
            "a second press during the prompt"
        );
        assert_eq!(trust_gate.prompt().unwrap().trust_button, None);
    }

    #[test]
    fn a_trusted_ca_does_not_wait() {
        let trust_gate = LoopbackTrust::default();
        trust_gate.wait_until_trusted(|| TrustStatus::Trusted);
        assert_eq!(trust_gate.prompt(), None);
    }

    #[test]
    fn the_launch_waits_until_a_press_trusts_the_ca() {
        let trust_gate = Arc::new(LoopbackTrust::default());
        let waiter = {
            let trust_gate = Arc::clone(&trust_gate);
            std::thread::spawn(move || trust_gate.wait_until_trusted(untrusted))
        };
        // The check lands, then the gate holds until the press.
        while trust_gate.prompt().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!waiter.is_finished(), "the launch went on without trust");

        assert!(trust_gate.begin_attempt());
        trust_gate.finish_attempt(Err("User canceled".into()), untrusted());
        std::thread::sleep(Duration::from_millis(20));
        assert!(!waiter.is_finished(), "a failed press let the launch go on");

        assert!(trust_gate.begin_attempt());
        trust_gate.finish_attempt(Ok(()), TrustStatus::Trusted);
        waiter.join().unwrap();
        assert_eq!(trust_gate.prompt(), None);
    }
}
