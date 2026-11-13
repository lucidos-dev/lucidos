//! The gateway stamp: a short-lived signature over a claim the starter of an
//! engine vouches for.
//!
//! An engine cannot tell its gateway's hop from any other local process: both
//! hold the machine-local token, and both can type any header. So a header
//! naming a device is a claim, not proof. The stamp makes it proof. The
//! gateway holds a fresh *stamp key* per engine it starts, hands it over the
//! engine's stdin pipe, and signs each claim it forwards. Nothing else holds
//! the key, because it never touches a file or an environment variable.
//!
//! The plan: `docs/plans/2026-11-10-unattested-callers-and-gateway-stamps.md`.

use std::io::BufRead;

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// The request header carrying the stamp. The gateway owns it: a client's copy
/// is stripped at the hop, the same as the device id it vouches for.
pub const HEADER_GATEWAY_STAMP: &str = "x-lucidos-gateway-stamp";

/// Set to `1` in an engine's environment when its starter writes the stamp key
/// to the engine's stdin. Only the flag travels this way, never the key.
///
/// The engine removes it from its own environment once read. A subprocess that
/// inherited it and then started an engine would leave that engine waiting
/// on a stdin nobody writes.
pub const ENV_STAMP_KEY_ON_STDIN: &str = "LUCIDOS_STAMP_KEY_ON_STDIN";

/// Set to `1` in an engine's environment by the gateway that spawned it. The
/// engine reports it in its health body as `started_by_gateway`. A gateway
/// that restarts while the engine lives on then takes it back and re-keys it.
/// Without it, the engine is adopted keyless, as another starter's.
///
/// The engine removes it from its own environment, like
/// [`ENV_STAMP_KEY_ON_STDIN`], so an engine a subprocess starts never claims it.
pub const ENV_STARTED_BY_GATEWAY: &str = "LUCIDOS_STARTED_BY_GATEWAY";

/// How far a stamp's timestamp may sit from the verifier's clock. Signer and
/// verifier share one machine, so this bounds replay, not clock skew.
pub const WINDOW_SECS: u64 = 60;

/// The bytes of key material.
const KEY_BYTES: usize = 32;

/// Names this format, so a stamp can never verify as any other HMAC under the
/// same key.
const DOMAIN: &str = "lucidos/gateway-stamp/v1";

/// The version prefix of a stamp's wire form.
const WIRE_VERSION: &str = "v1";

type HmacSha256 = Hmac<Sha256>;

/// A per-engine stamp key.
///
/// Implements neither `Clone`, `Serialize` nor a `Debug` that prints its bytes.
/// The one way out of the process is [`StampKey::handover_line`], which a
/// starter writes into a pipe.
pub struct StampKey([u8; KEY_BYTES]);

impl std::fmt::Debug for StampKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StampKey(<redacted>)")
    }
}

/// The calling workspace of a [`Claim::WorkspaceCall`]. Gateway-owned, like
/// [`HEADER_GATEWAY_STAMP`].
pub const HEADER_CALLER_WORKSPACE: &str = "x-lucidos-caller-workspace";

/// The calling thread of a [`Claim::WorkspaceCall`], empty for a call with no
/// thread (a scheduled script).
pub const HEADER_CALLER_THREAD: &str = "x-lucidos-caller-thread";

/// The calling workspace's spawn-gate verdict on its thread. Opaque here: the
/// engine encodes and decodes it, and the gateway carries it.
pub const HEADER_CALLER_VERDICT: &str = "x-lucidos-caller-verdict";

/// The `reason` an engine answers on its workspace-call route when it has no
/// gateway to sign for it. The caller then posts to the target directly.
pub const REASON_NO_GATEWAY: &str = "no_gateway";

/// The `reason` a gateway answers when the target workspace is not one of its
/// own. The caller then posts to the target directly.
pub const REASON_WORKSPACE_NOT_ON_THIS_GATEWAY: &str = "workspace_not_on_this_gateway";

/// Where cross-workspace calls live, below an API root: the engine's
/// `/api/v1`, the gateway's `/~/api/v1`, or the gateway-to-gateway
/// `/~/api/gateway/v1`.
pub const WORKSPACE_CALLS_PATH: &str = "/workspace-calls";

/// The route of a cross-workspace spawn into `target`, below an API root.
///
/// `None` for a name that is not a plain path segment, so no name can reach
/// another route.
pub fn workspace_call_path(target: &str) -> Option<String> {
    is_plain_target(target).then(|| format!("{WORKSPACE_CALLS_PATH}/{target}/chat/stream"))
}

/// Is `target` a plain path segment: letters, digits, `-`, `_` and `.`, and
/// never `.` or `..` alone?
pub fn is_plain_target(target: &str) -> bool {
    !target.is_empty()
        && target != "."
        && target != ".."
        && target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// What a stamp vouches for. Each variant signs under its own kind name, so a
/// stamp for one claim never verifies as another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim<'a> {
    /// A paired device, authenticated by the gateway, reaches this workspace.
    Device {
        workspace_id: &'a str,
        device_id: &'a str,
    },
    /// A thread of `caller_workspace` spawns into `target_workspace`, and the
    /// caller's spawn gate gave `verdict`. The calling engine signs it for its
    /// gateway, and the gateway signs it again for the target.
    WorkspaceCall {
        target_workspace: &'a str,
        caller_workspace: &'a str,
        caller_thread_id: &'a str,
        verdict: &'a str,
    },
}

impl Claim<'_> {
    fn signed_fields(&self) -> (&'static str, Vec<&str>) {
        match self {
            Claim::Device {
                workspace_id,
                device_id,
            } => ("device", vec![workspace_id, device_id]),
            Claim::WorkspaceCall {
                target_workspace,
                caller_workspace,
                caller_thread_id,
                verdict,
            } => (
                "workspace-call",
                vec![
                    target_workspace,
                    caller_workspace,
                    caller_thread_id,
                    verdict,
                ],
            ),
        }
    }
}

impl StampKey {
    /// A fresh key from the OS's entropy source.
    pub fn generate() -> std::io::Result<Self> {
        let hex = lucidos_local_token::mint_hex(KEY_BYTES)?;
        Self::from_hex(&hex).ok_or_else(|| std::io::Error::other("minted key is not hex"))
    }

    /// The key as one line for a pipe: lowercase hex and a newline.
    pub fn handover_line(&self) -> String {
        let mut line = hex(&self.0);
        line.push('\n');
        line
    }

    /// Read one handover line. `None` for anything but a well-formed key, so a
    /// broken pipe leaves the engine keyless rather than keyed with junk.
    pub fn read_handover(mut reader: impl BufRead) -> Option<Self> {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        Self::from_hex(line.trim_end_matches(['\n', '\r']))
    }

    fn from_hex(text: &str) -> Option<Self> {
        if text.len() != KEY_BYTES * 2 || !text.is_ascii() {
            return None;
        }
        let mut key = [0u8; KEY_BYTES];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self(key))
    }

    /// Sign `claim` at `now_unix`, giving the header value.
    pub fn sign(&self, claim: Claim<'_>, now_unix: u64) -> String {
        format!(
            "{WIRE_VERSION}.{now_unix}.{}",
            hex(&self.mac(claim, now_unix))
        )
    }

    /// [`Self::sign`] at the current time.
    pub fn sign_now(&self, claim: Claim<'_>) -> String {
        self.sign(claim, now_unix())
    }

    /// Does `stamp` vouch for exactly `claim`, within [`WINDOW_SECS`] of
    /// `now_unix`? Every failure answers `false` alike.
    pub fn verify(&self, stamp: &str, claim: Claim<'_>, now_unix: u64) -> bool {
        let mut parts = stamp.split('.');
        let (Some(WIRE_VERSION), Some(at), Some(presented), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        let Ok(at) = at.parse::<u64>() else {
            return false;
        };
        if at.abs_diff(now_unix) > WINDOW_SECS {
            return false;
        }
        lucidos_local_token::ct_eq(presented, &hex(&self.mac(claim, at)))
    }

    /// [`Self::verify`] at the current time.
    pub fn verify_now(&self, stamp: &str, claim: Claim<'_>) -> bool {
        self.verify(stamp, claim, now_unix())
    }

    fn mac(&self, claim: Claim<'_>, at: u64) -> [u8; 32] {
        let (kind, fields) = claim.signed_fields();
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("HMAC takes any key length");
        // Each field is length-prefixed, so no value can shift a boundary.
        for field in [DOMAIN, kind, &at.to_string()].into_iter().chain(fields) {
            mac.update(field.len().to_string().as_bytes());
            mac.update(b":");
            mac.update(field.as_bytes());
        }
        mac.finalize().into_bytes().into()
    }
}

/// Seconds since the Unix epoch. A clock before 1970 reads as 0, which no
/// live stamp is near.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: Claim<'static> = Claim::Device {
        workspace_id: "dev",
        device_id: "d1",
    };

    fn key() -> StampKey {
        StampKey::generate().expect("entropy")
    }

    #[test]
    fn a_stamp_verifies_for_its_own_claim() {
        let key = key();
        let stamp = key.sign(DEVICE, 1_000);
        assert!(key.verify(&stamp, DEVICE, 1_000));
        assert!(key.verify(&stamp, DEVICE, 1_000 + WINDOW_SECS));
    }

    #[test]
    fn a_stamp_does_not_verify_for_another_device_or_workspace() {
        let key = key();
        let stamp = key.sign(DEVICE, 1_000);
        let other_device = Claim::Device {
            workspace_id: "dev",
            device_id: "d2",
        };
        let other_workspace = Claim::Device {
            workspace_id: "work2",
            device_id: "d1",
        };
        assert!(!key.verify(&stamp, other_device, 1_000));
        assert!(!key.verify(&stamp, other_workspace, 1_000));
    }

    #[test]
    fn a_field_boundary_cannot_shift() {
        let key = key();
        let stamp = key.sign(
            Claim::Device {
                workspace_id: "ab",
                device_id: "c",
            },
            1_000,
        );
        let shifted = Claim::Device {
            workspace_id: "a",
            device_id: "bc",
        };
        assert!(!key.verify(&stamp, shifted, 1_000));
    }

    #[test]
    fn another_engines_key_does_not_verify() {
        let stamp = key().sign(DEVICE, 1_000);
        assert!(!key().verify(&stamp, DEVICE, 1_000));
    }

    #[test]
    fn an_expired_or_future_stamp_does_not_verify() {
        let key = key();
        let stamp = key.sign(DEVICE, 1_000);
        assert!(!key.verify(&stamp, DEVICE, 1_000 + WINDOW_SECS + 1));
        assert!(!key.verify(&stamp, DEVICE, 1_000 - WINDOW_SECS - 1));
    }

    #[test]
    fn an_edited_timestamp_does_not_verify() {
        let key = key();
        let stamp = key.sign(DEVICE, 1_000);
        let edited = stamp.replacen(".1000.", ".1001.", 1);
        assert_ne!(stamp, edited);
        assert!(!key.verify(&edited, DEVICE, 1_000));
    }

    #[test]
    fn malformed_stamps_do_not_verify() {
        let key = key();
        let good = key.sign(DEVICE, 1_000);
        let extra_part = format!("{good}.extra");
        let other_version = good.replacen("v1.", "v2.", 1);
        for bad in [
            "",
            "v1",
            "v1.1000",
            "v2.1000.00",
            extra_part.as_str(),
            other_version.as_str(),
            "v1.notanumber.00",
        ] {
            assert!(!key.verify(bad, DEVICE, 1_000), "{bad:?}");
        }
    }

    #[test]
    fn the_handover_line_round_trips() {
        let key = key();
        let line = key.handover_line();
        assert!(line.ends_with('\n'));
        let read = StampKey::read_handover(line.as_bytes()).expect("a well-formed line");
        let stamp = key.sign(DEVICE, 1_000);
        assert!(read.verify(&stamp, DEVICE, 1_000));
    }

    #[test]
    fn a_broken_handover_leaves_no_key() {
        let short = "a".repeat(63);
        let not_hex = "g".repeat(64);
        for bad in ["", "\n", "zz\n", short.as_str(), not_hex.as_str()] {
            assert!(StampKey::read_handover(bad.as_bytes()).is_none(), "{bad:?}");
        }
    }

    const CALL: Claim<'static> = Claim::WorkspaceCall {
        target_workspace: "work2",
        caller_workspace: "dev",
        caller_thread_id: "t1",
        verdict: "allowed",
    };

    #[test]
    fn a_workspace_call_verifies_only_for_its_own_fields() {
        let key = key();
        let stamp = key.sign(CALL, 1_000);
        assert!(key.verify(&stamp, CALL, 1_000));
        let Claim::WorkspaceCall {
            target_workspace,
            caller_workspace,
            caller_thread_id,
            verdict,
        } = CALL
        else {
            unreachable!()
        };
        for other in [
            Claim::WorkspaceCall {
                target_workspace: "dev",
                caller_workspace,
                caller_thread_id,
                verdict,
            },
            Claim::WorkspaceCall {
                target_workspace,
                caller_workspace: "work3",
                caller_thread_id,
                verdict,
            },
            Claim::WorkspaceCall {
                target_workspace,
                caller_workspace,
                caller_thread_id: "t2",
                verdict,
            },
            Claim::WorkspaceCall {
                target_workspace,
                caller_workspace,
                caller_thread_id,
                verdict: "unattested",
            },
        ] {
            assert!(!key.verify(&stamp, other, 1_000), "{other:?}");
        }
    }

    #[test]
    fn a_device_stamp_never_verifies_as_a_workspace_call() {
        let key = key();
        let device = Claim::Device {
            workspace_id: "work2",
            device_id: "dev",
        };
        assert!(!key.verify(&key.sign(device, 1_000), CALL, 1_000));
        assert!(!key.verify(&key.sign(CALL, 1_000), device, 1_000));
    }

    #[test]
    fn only_a_plain_name_gets_a_workspace_call_path() {
        assert_eq!(
            workspace_call_path("work-2_a.b").as_deref(),
            Some("/workspace-calls/work-2_a.b/chat/stream")
        );
        for bad in ["", ".", "..", "a/b", "a?b", "a b", "a%2f"] {
            assert_eq!(workspace_call_path(bad), None, "{bad:?}");
        }
    }

    /// Pins the wire format. The browser e2e harness signs in TypeScript, and
    /// `crates/lucidos-app/e2e/harnessStamp.test.ts` pins the same vector.
    #[test]
    fn a_fixed_key_signs_the_pinned_vector() {
        let key = StampKey::from_hex(&"ab".repeat(32)).expect("a fixed test key");
        assert_eq!(
            key.sign(DEVICE, 1_000),
            "v1.1000.fa40792259c9d99bff467c50b8b976de3da76e0b92c9a69360d5917c49bfa3c4"
        );
    }

    #[test]
    fn debug_never_prints_the_key() {
        let key = key();
        let printed = format!("{key:?}");
        assert_eq!(printed, "StampKey(<redacted>)");
        assert!(!printed.contains(key.handover_line().trim()));
    }
}
