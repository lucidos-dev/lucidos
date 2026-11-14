//! The engine's half of the gateway stamp (`lucidos-gateway-stamp`).
//!
//! A device id header is a claim any local process can type. The starter of
//! this engine, normally the gateway, signs the device it authenticated with a
//! per-engine stamp key it wrote to our stdin at spawn. [`device_is_stamped`]
//! and [`workspace_call_verdict`] are the questions the rest of the engine
//! asks. With no key, every claim is unattested: a keyless engine fails closed.
//!
//! The same key signs this engine's own cross-workspace calls for its gateway
//! ([`sign_workspace_call`]), since the gateway holds its copy.

use std::sync::OnceLock;

use lucidos_gateway_stamp::{
    Claim, StampKey, ENV_STAMP_KEY_ON_STDIN, ENV_STARTED_BY_GATEWAY, HEADER_CALLER_THREAD,
    HEADER_CALLER_VERDICT, HEADER_CALLER_WORKSPACE, HEADER_GATEWAY_STAMP,
};
use uuid::Uuid;

use crate::engine::thread_events::SpawnVerdict;

/// The key and the workspace it signs for. Installed once at boot, never
/// replaced: a new key means a new engine process.
struct StampAuthority {
    key: StampKey,
    workspace_id: String,
}

static AUTHORITY: OnceLock<StampAuthority> = OnceLock::new();

/// Whether a gateway spawned this engine, read once at boot.
static STARTED_BY_GATEWAY: OnceLock<bool> = OnceLock::new();

/// Did a gateway spawn this engine? Reported in `/api/v1/health`, so a
/// restarted gateway takes it back and re-keys it.
pub(crate) fn started_by_gateway() -> bool {
    STARTED_BY_GATEWAY.get().copied().unwrap_or(false)
}

/// Read and remove [`ENV_STARTED_BY_GATEWAY`]. `true` only for exactly `1`.
fn take_started_by_gateway() -> bool {
    let started = std::env::var(ENV_STARTED_BY_GATEWAY).is_ok_and(|v| v == "1");
    std::env::remove_var(ENV_STARTED_BY_GATEWAY);
    started
}

/// Take the stamp key the starter wrote to stdin, when the environment says it
/// did, and whether the starter was a gateway. Call once, from `main`, before
/// any thread starts: it edits this process's environment.
///
/// Both flags leave the environment either way, so no subprocess inherits
/// them.
pub fn take_stamp_key_from_starter() {
    if STARTED_BY_GATEWAY.set(take_started_by_gateway()).is_err() {
        crate::log!("[Stamp] The starter was read twice; keeping the first answer");
    }
    let flagged = std::env::var(ENV_STAMP_KEY_ON_STDIN).is_ok_and(|v| v == "1");
    std::env::remove_var(ENV_STAMP_KEY_ON_STDIN);
    if !flagged {
        crate::log!("[Stamp] No stamp key from the starter: every device claim is unattested");
        return;
    }
    let Some(workspace_id) = stamped_workspace_id(
        std::env::var("LUCIDOS_WORKSPACE_ID").ok(),
        std::env::var("LUCIDOS_WORKSPACE").ok(),
    ) else {
        crate::log!(
            "[Stamp] A stamp key was offered with no workspace to bind it to: every device \
             claim is unattested"
        );
        return;
    };
    match StampKey::read_handover(std::io::stdin().lock()) {
        Some(key) => install(key, workspace_id),
        None => crate::log!(
            "[Stamp] The starter's stamp key did not arrive intact: every device claim is \
             unattested"
        ),
    }
}

/// The workspace a stamp names: the gateway's slug for this engine, else the
/// workspace directory's name. A harness that starts its own engine has no
/// slug, and signs with the directory's name (`scripts/lib/e2e_stamp.sh`).
fn stamped_workspace_id(slug: Option<String>, workspace_dir: Option<String>) -> Option<String> {
    let non_blank = |s: String| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    slug.and_then(non_blank).or_else(|| {
        let dir = workspace_dir.and_then(non_blank)?;
        std::path::Path::new(&dir)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    })
}

fn install(key: StampKey, workspace_id: String) {
    if AUTHORITY.set(StampAuthority { key, workspace_id }).is_err() {
        crate::log!("[Stamp] A second stamp key was offered and ignored");
    }
}

tokio::task_local! {
    static ARRIVED_AT: u64;
}

/// Note when a request arrived, so its stamp's age is judged then. A body that
/// takes longer than the stamp window to upload still comes from its device.
pub(crate) async fn note_arrival(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    ARRIVED_AT
        .scope(lucidos_gateway_stamp::now_unix(), next.run(req))
        .await
}

/// The time a stamp is checked at: the request's arrival, else now.
fn checked_at() -> u64 {
    ARRIVED_AT
        .try_with(|at| *at)
        .unwrap_or_else(|_| lucidos_gateway_stamp::now_unix())
}

/// Did this engine's starter sign `device_id` on this request?
pub(crate) fn device_is_stamped(headers: &axum::http::HeaderMap, device_id: &str) -> bool {
    stamped_by(AUTHORITY.get(), headers, device_id)
}

fn stamped_by(
    authority: Option<&StampAuthority>,
    headers: &axum::http::HeaderMap,
    device_id: &str,
) -> bool {
    authority.is_some_and(|authority| authority.device_is_stamped(headers, device_id, checked_at()))
}

/// The calling workspace's verdict on a cross-workspace call, when this
/// engine's starter stamped it for `caller_workspace` and `caller_thread_id`.
/// `None` for an unstamped, forged or mismatched call: an unattested caller.
pub(crate) fn workspace_call_verdict(
    headers: &axum::http::HeaderMap,
    caller_workspace: &str,
    caller_thread_id: Option<Uuid>,
) -> Option<SpawnVerdict> {
    AUTHORITY.get()?.workspace_call_verdict(
        headers,
        caller_workspace,
        caller_thread_id,
        checked_at(),
    )
}

/// A cross-workspace call this engine signed for its gateway: the claim's
/// headers, ready to send.
pub(crate) struct SignedWorkspaceCall {
    /// This workspace's own id, the name the gateway knows it by.
    pub(crate) caller_workspace: String,
    pub(crate) headers: Vec<(&'static str, String)>,
}

/// Sign a call from `caller_thread_id` into `target_workspace`, carrying
/// `verdict`. `None` on an engine with no stamp key: no gateway would accept
/// its claim.
pub(crate) fn sign_workspace_call(
    target_workspace: &str,
    caller_thread_id: Option<Uuid>,
    verdict: &SpawnVerdict,
) -> Option<SignedWorkspaceCall> {
    let authority = AUTHORITY.get()?;
    let thread = caller_thread_id.map(|t| t.to_string()).unwrap_or_default();
    let verdict = verdict.to_header();
    let stamp = authority.key.sign_now(Claim::WorkspaceCall {
        target_workspace,
        caller_workspace: &authority.workspace_id,
        caller_thread_id: &thread,
        verdict: &verdict,
    });
    Some(SignedWorkspaceCall {
        caller_workspace: authority.workspace_id.clone(),
        headers: vec![
            (HEADER_CALLER_WORKSPACE, authority.workspace_id.clone()),
            (HEADER_CALLER_THREAD, thread),
            (HEADER_CALLER_VERDICT, verdict),
            (HEADER_GATEWAY_STAMP, stamp),
        ],
    })
}

fn header<'h>(headers: &'h axum::http::HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

impl StampAuthority {
    fn device_is_stamped(
        &self,
        headers: &axum::http::HeaderMap,
        device_id: &str,
        now_unix: u64,
    ) -> bool {
        let Some(stamp) = header(headers, HEADER_GATEWAY_STAMP) else {
            return false;
        };
        self.key.verify(
            stamp,
            Claim::Device {
                workspace_id: &self.workspace_id,
                device_id,
            },
            now_unix,
        )
    }

    fn workspace_call_verdict(
        &self,
        headers: &axum::http::HeaderMap,
        caller_workspace: &str,
        caller_thread_id: Option<Uuid>,
        now_unix: u64,
    ) -> Option<SpawnVerdict> {
        let stamp = header(headers, HEADER_GATEWAY_STAMP)?;
        let verdict = header(headers, HEADER_CALLER_VERDICT)?;
        let thread = caller_thread_id.map(|t| t.to_string()).unwrap_or_default();
        let claim = Claim::WorkspaceCall {
            target_workspace: &self.workspace_id,
            caller_workspace,
            caller_thread_id: &thread,
            verdict,
        };
        if !self.key.verify(stamp, claim, now_unix) {
            return None;
        }
        SpawnVerdict::from_header(verdict)
    }
}

/// Stamped requests for tests, under one fixed test key.
///
/// The authority is process-global and first-writer-wins, so every test shares
/// this one. A test of a keyless engine asks [`StampAuthority`] directly.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    const TEST_WORKSPACE: &str = "stamp-test-workspace";

    fn test_key() -> StampKey {
        StampKey::read_handover("ab".repeat(32).as_bytes()).expect("a fixed test key")
    }

    fn authority() -> &'static StampAuthority {
        AUTHORITY.get_or_init(|| StampAuthority {
            key: test_key(),
            workspace_id: TEST_WORKSPACE.to_string(),
        })
    }

    /// The stamp header value the gateway would send for `device_id`.
    pub(crate) fn stamp_for(device_id: &str) -> String {
        let authority = authority();
        authority.key.sign_now(Claim::Device {
            workspace_id: &authority.workspace_id,
            device_id,
        })
    }

    /// Add `device_id` and its stamp, as the gateway forwards a paired device.
    pub(crate) fn insert_stamped_device(headers: &mut axum::http::HeaderMap, device_id: &str) {
        headers.insert(
            crate::api::actor::HEADER_DEVICE_ID,
            device_id.parse().expect("a header-safe device id"),
        );
        headers.insert(
            HEADER_GATEWAY_STAMP,
            stamp_for(device_id).parse().expect("a header-safe stamp"),
        );
    }

    /// Headers holding only a stamped device.
    pub(crate) fn stamped_device_headers(device_id: &str) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        insert_stamped_device(&mut headers, device_id);
        headers
    }

    /// The claim headers the gateway forwards for a call into this engine
    /// from `caller_workspace`'s thread.
    pub(crate) fn stamped_workspace_call_headers(
        caller_workspace: &str,
        caller_thread_id: Option<Uuid>,
        verdict: &SpawnVerdict,
    ) -> axum::http::HeaderMap {
        let authority = authority();
        let thread = caller_thread_id.map(|t| t.to_string()).unwrap_or_default();
        let verdict = verdict.to_header();
        let stamp = authority.key.sign_now(Claim::WorkspaceCall {
            target_workspace: &authority.workspace_id,
            caller_workspace,
            caller_thread_id: &thread,
            verdict: &verdict,
        });
        let mut headers = axum::http::HeaderMap::new();
        for (name, value) in [
            (HEADER_CALLER_VERDICT, verdict),
            (HEADER_GATEWAY_STAMP, stamp),
        ] {
            headers.insert(name, value.parse().expect("a header-safe value"));
        }
        headers
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn headers_with_stamp(stamp: &str) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(HEADER_GATEWAY_STAMP, stamp.parse().unwrap());
        headers
    }

    #[test]
    fn a_stamp_binds_to_the_slug_else_the_workspace_directory() {
        let some = |s: &str| Some(s.to_string());
        assert_eq!(
            stamped_workspace_id(some("dev"), some("/home/u/workspaces/e2e-test")).as_deref(),
            Some("dev")
        );
        assert_eq!(
            stamped_workspace_id(some("  "), some("/home/u/workspaces/e2e-test/")).as_deref(),
            Some("e2e-test")
        );
        assert_eq!(stamped_workspace_id(None, some("/")), None);
        assert_eq!(stamped_workspace_id(None, None), None);
    }

    #[test]
    fn a_stamped_device_verifies() {
        assert!(device_is_stamped(&stamped_device_headers("d1"), "d1"));
    }

    #[test]
    fn a_stamp_for_another_device_does_not_verify() {
        assert!(!device_is_stamped(
            &headers_with_stamp(&stamp_for("d1")),
            "d2"
        ));
    }

    #[test]
    fn a_typed_device_id_with_no_stamp_does_not_verify() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(crate::api::actor::HEADER_DEVICE_ID, "d1".parse().unwrap());
        assert!(!device_is_stamped(&headers, "d1"));
    }

    #[test]
    fn a_forged_stamp_does_not_verify() {
        assert!(!device_is_stamped(
            &headers_with_stamp(&format!(
                "v1.{}.{}",
                lucidos_gateway_stamp::now_unix(),
                "00".repeat(32)
            )),
            "d1"
        ));
    }

    #[test]
    fn another_engines_stamp_does_not_verify() {
        let other = StampKey::generate().unwrap();
        let stamp = other.sign_now(Claim::Device {
            workspace_id: "stamp-test-workspace",
            device_id: "d1",
        });
        assert!(!device_is_stamped(&headers_with_stamp(&stamp), "d1"));
    }

    #[test]
    fn an_expired_stamp_does_not_verify() {
        let _ = stamp_for("d1");
        let authority = AUTHORITY.get().unwrap();
        let old = authority.key.sign(
            Claim::Device {
                workspace_id: &authority.workspace_id,
                device_id: "d1",
            },
            lucidos_gateway_stamp::now_unix() - lucidos_gateway_stamp::WINDOW_SECS - 5,
        );
        assert!(!device_is_stamped(&headers_with_stamp(&old), "d1"));
    }

    /// A slow upload ends past the window, yet the stamp was fresh when the
    /// request arrived, so it still verifies.
    #[tokio::test]
    async fn a_stamp_is_judged_when_its_request_arrived() {
        let _ = stamp_for("d1");
        let authority = AUTHORITY.get().unwrap();
        let arrived = lucidos_gateway_stamp::now_unix() - lucidos_gateway_stamp::WINDOW_SECS - 5;
        let stamp = authority.key.sign(
            Claim::Device {
                workspace_id: &authority.workspace_id,
                device_id: "d1",
            },
            arrived,
        );
        let headers = headers_with_stamp(&stamp);
        assert!(
            ARRIVED_AT
                .scope(arrived, async { device_is_stamped(&headers, "d1") })
                .await
        );
        assert!(!device_is_stamped(&headers, "d1"));
    }

    /// Only an exact `1` counts, and the flag leaves the environment, so a
    /// subprocess's own engine never claims a gateway started it.
    #[test]
    fn the_gateway_flag_is_read_once_and_leaves_the_environment() {
        std::env::set_var(ENV_STARTED_BY_GATEWAY, "1");
        assert!(take_started_by_gateway());
        assert!(std::env::var_os(ENV_STARTED_BY_GATEWAY).is_none());
        assert!(!take_started_by_gateway());
        std::env::set_var(ENV_STARTED_BY_GATEWAY, "yes");
        assert!(!take_started_by_gateway());
        assert!(std::env::var_os(ENV_STARTED_BY_GATEWAY).is_none());
    }

    #[test]
    fn a_keyless_engine_reads_a_stamp_as_unattested() {
        let headers = stamped_device_headers("d1");
        assert!(!stamped_by(None, &headers, "d1"));
    }

    #[test]
    fn a_stamp_for_another_workspace_does_not_verify() {
        let authority = StampAuthority {
            key: StampKey::generate().unwrap(),
            workspace_id: "dev".into(),
        };
        let stamp = authority.key.sign(
            Claim::Device {
                workspace_id: "work2",
                device_id: "d1",
            },
            1_000,
        );
        assert!(!authority.device_is_stamped(&headers_with_stamp(&stamp), "d1", 1_000));
    }

    fn refused() -> SpawnVerdict {
        SpawnVerdict::Refused {
            reason: "The trigger \u{201c}Nightly\u{201d} has no grant.".into(),
        }
    }

    #[test]
    fn a_stamped_workspace_call_yields_its_verdict() {
        let thread = Uuid::new_v4();
        for verdict in [SpawnVerdict::Allowed, SpawnVerdict::Unattested, refused()] {
            let headers = stamped_workspace_call_headers("dev", Some(thread), &verdict);
            assert_eq!(
                workspace_call_verdict(&headers, "dev", Some(thread)),
                Some(verdict)
            );
        }
    }

    #[test]
    fn a_workspace_call_for_another_caller_or_thread_is_unattested() {
        let thread = Uuid::new_v4();
        let headers = stamped_workspace_call_headers("dev", Some(thread), &SpawnVerdict::Allowed);
        assert_eq!(
            workspace_call_verdict(&headers, "work2", Some(thread)),
            None
        );
        assert_eq!(
            workspace_call_verdict(&headers, "dev", Some(Uuid::new_v4())),
            None
        );
        assert_eq!(workspace_call_verdict(&headers, "dev", None), None);
    }

    /// A caller cannot upgrade its verdict: the stamp signs the header.
    #[test]
    fn an_edited_verdict_is_unattested() {
        let mut headers = stamped_workspace_call_headers("dev", None, &refused());
        headers.insert(
            HEADER_CALLER_VERDICT,
            SpawnVerdict::Allowed.to_header().parse().unwrap(),
        );
        assert_eq!(workspace_call_verdict(&headers, "dev", None), None);
    }

    #[test]
    fn a_device_stamp_never_vouches_for_a_workspace_call() {
        let mut headers = stamped_device_headers("dev");
        headers.insert(
            HEADER_CALLER_VERDICT,
            SpawnVerdict::Allowed.to_header().parse().unwrap(),
        );
        assert_eq!(workspace_call_verdict(&headers, "dev", None), None);
    }

    #[test]
    fn an_unstamped_workspace_call_is_unattested() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            HEADER_CALLER_VERDICT,
            SpawnVerdict::Allowed.to_header().parse().unwrap(),
        );
        assert_eq!(workspace_call_verdict(&headers, "dev", None), None);
    }

    /// What this engine signs for its gateway is what a gateway holding the
    /// same key verifies.
    #[test]
    fn an_outbound_call_signs_its_own_workspace_as_the_caller() {
        let _ = stamp_for("d1");
        let thread = Uuid::new_v4();
        let signed = sign_workspace_call("work2", Some(thread), &refused()).unwrap();
        assert_eq!(signed.caller_workspace, "stamp-test-workspace");
        let get = |name: &str| {
            signed
                .headers
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(get(HEADER_CALLER_WORKSPACE), "stamp-test-workspace");
        assert_eq!(get(HEADER_CALLER_THREAD), thread.to_string());
        let verdict = get(HEADER_CALLER_VERDICT);
        assert_eq!(SpawnVerdict::from_header(&verdict), Some(refused()));
        let authority = AUTHORITY.get().unwrap();
        assert!(authority.key.verify_now(
            &get(HEADER_GATEWAY_STAMP),
            Claim::WorkspaceCall {
                target_workspace: "work2",
                caller_workspace: "stamp-test-workspace",
                caller_thread_id: &thread.to_string(),
                verdict: &verdict,
            }
        ));
    }
}
