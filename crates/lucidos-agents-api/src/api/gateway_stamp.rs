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

use lucidos_gateway_stamp::remote_start::RemoteStart;
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
    operation_nonces: std::sync::Mutex<std::collections::HashMap<(String, String, String), u64>>,
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
        crate::log_legacy!("[Stamp] The starter was read twice; keeping the first answer");
    }
    let flagged = std::env::var(ENV_STAMP_KEY_ON_STDIN).is_ok_and(|v| v == "1");
    std::env::remove_var(ENV_STAMP_KEY_ON_STDIN);
    if !flagged {
        crate::log_legacy!(
            "[Stamp] No stamp key from the starter: every device claim is unattested"
        );
        return;
    }
    let Some(workspace_id) = stamped_workspace_id(
        std::env::var("LUCIDOS_WORKSPACE_ID").ok(),
        std::env::var("LUCIDOS_WORKSPACE").ok(),
    ) else {
        crate::log_legacy!(
            "[Stamp] A stamp key was offered with no workspace to bind it to: every device \
             claim is unattested"
        );
        return;
    };
    match StampKey::read_handover(std::io::stdin().lock()) {
        Some(key) => install(key, workspace_id),
        None => crate::log_legacy!(
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
    if AUTHORITY
        .set(StampAuthority {
            key,
            workspace_id,
            operation_nonces: Default::default(),
        })
        .is_err()
    {
        crate::log_legacy!("[Stamp] A second stamp key was offered and ignored");
    }
}

tokio::task_local! {
    static ARRIVED_AT: u64;
}

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

/// Verify the receiver's own starter stamp before trusting delivery identity.
pub(crate) fn diagnostic_delivery(
    headers: &axum::http::HeaderMap,
) -> Option<lucidos_gateway_stamp::DiagnosticDelivery> {
    let authority = AUTHORITY.get()?;
    let delivery = lucidos_gateway_stamp::DiagnosticDelivery::new(
        header(headers, lucidos_gateway_stamp::HEADER_DIAGNOSTIC_ORIGIN)?,
        header(headers, lucidos_gateway_stamp::HEADER_DIAGNOSTIC_DELIVERY)?,
        header(headers, lucidos_gateway_stamp::HEADER_DIAGNOSTIC_DIGEST)?,
    )?;
    authority
        .key
        .verify_now(
            header(headers, HEADER_GATEWAY_STAMP)?,
            delivery.claim(&authority.workspace_id),
        )
        .then_some(delivery)
}

/// A cross-workspace call this engine signed for its gateway: the claim's
/// headers, ready to send.
pub(crate) struct SignedWorkspaceCall {
    /// This workspace's own id, the name the gateway knows it by.
    pub(crate) caller_workspace: String,
    pub(crate) headers: Vec<(&'static str, String)>,
}

pub(crate) fn sign_operation(
    request: lucidos_gateway_stamp::workspace_operation::RoutingRequest,
    thread: Uuid,
    verdict: String,
) -> Option<lucidos_gateway_stamp::workspace_operation::SignedOperation> {
    use lucidos_gateway_stamp::workspace_operation::{OperationEnvelope, SignedOperation};
    let authority = AUTHORITY.get()?;
    let envelope = OperationEnvelope {
        request,
        caller_gateway: String::new(),
        caller_workspace: authority.workspace_id.clone(),
        caller_thread_id: thread.to_string(),
        verdict,
        nonce: Uuid::new_v4().to_string(),
    };
    let digest = envelope.digest().ok()?;
    let stamp = authority
        .key
        .sign_now(Claim::WorkspaceOperation(envelope.claim(&digest)));
    Some(SignedOperation { envelope, stamp })
}

/// What a verified operation hands the existing handlers: the spawn identity
/// headers, and for a spawn the remote start its gateway signed.
pub(crate) type VerifiedOperation = (axum::http::HeaderMap, Option<RemoteStart>);

/// Verify the complete operation before translating its spawn identity for
/// the existing chat handler. A read cannot produce a legacy spawn stamp. A
/// spawn needs a valid `remote_start` header value from the gateway.
pub(crate) fn verify_operation(
    signed: &lucidos_gateway_stamp::workspace_operation::SignedOperation,
    remote_start: Option<&str>,
) -> Option<VerifiedOperation> {
    verify_operation_by(AUTHORITY.get(), signed, remote_start)
}

fn verify_operation_by(
    authority: Option<&StampAuthority>,
    signed: &lucidos_gateway_stamp::workspace_operation::SignedOperation,
    remote_start: Option<&str>,
) -> Option<VerifiedOperation> {
    use lucidos_gateway_stamp::workspace_operation::WorkspaceOperation;
    let authority = authority?;
    let envelope = &signed.envelope;
    if envelope.request.to.as_deref() != Some(&authority.workspace_id)
        || envelope.caller_gateway.is_empty()
        || envelope.request.via.is_none()
    {
        return None;
    }
    let digest = envelope.digest().ok()?;
    if !authority.key.verify(
        &signed.stamp,
        Claim::WorkspaceOperation(envelope.claim(&digest)),
        checked_at(),
    ) {
        return None;
    }
    let mut headers = axum::http::HeaderMap::new();
    let mut start = None;
    if let WorkspaceOperation::Spawn { .. } = &envelope.request.operation {
        SpawnVerdict::from_header(&envelope.verdict)?;
        start = Some(RemoteStart::from_header(
            remote_start?,
            &authority.key,
            &authority.workspace_id,
            &envelope.nonce,
            checked_at(),
        )?);
        let stamp = authority.key.sign_now(Claim::WorkspaceCall {
            target_workspace: &authority.workspace_id,
            caller_workspace: &envelope.caller_workspace,
            caller_thread_id: &envelope.caller_thread_id,
            verdict: &envelope.verdict,
        });
        for (name, value) in [
            (HEADER_GATEWAY_STAMP, stamp.as_str()),
            (HEADER_CALLER_WORKSPACE, envelope.caller_workspace.as_str()),
            (HEADER_CALLER_THREAD, envelope.caller_thread_id.as_str()),
            (HEADER_CALLER_VERDICT, envelope.verdict.as_str()),
        ] {
            headers.insert(name, value.parse().ok()?);
        }
    } else if !envelope.verdict.is_empty() {
        return None;
    }
    let now = checked_at();
    let mut seen = authority.operation_nonces.lock().ok()?;
    seen.retain(|_, expires| *expires > now);
    let nonce = (
        envelope.caller_gateway.clone(),
        envelope.caller_workspace.clone(),
        envelope.nonce.clone(),
    );
    if seen.len() >= 1024 || seen.contains_key(&nonce) {
        return None;
    }
    seen.insert(
        nonce,
        now.saturating_add(2 * lucidos_gateway_stamp::WINDOW_SECS + 1),
    );
    Some((headers, start))
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
        if let Some(session) = header(headers, lucidos_gateway_stamp::HEADER_DEVICE_SESSION) {
            if self
                .key
                .verify_session(session, &self.workspace_id, device_id, now_unix)
            {
                return true;
            }
        }
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
            operation_nonces: Default::default(),
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

    pub(crate) fn diagnostic_headers(
        origin: &str,
        delivery_key: &str,
        digest: &str,
    ) -> axum::http::HeaderMap {
        let authority = authority();
        let delivery =
            lucidos_gateway_stamp::DiagnosticDelivery::new(origin, delivery_key, digest).unwrap();
        let mut headers = axum::http::HeaderMap::new();
        for (name, value) in delivery.headers() {
            headers.insert(name, value.parse().unwrap());
        }
        headers.insert(
            HEADER_GATEWAY_STAMP,
            authority
                .key
                .sign_now(delivery.claim(&authority.workspace_id))
                .parse()
                .unwrap(),
        );
        headers
    }

    /// Headers for a direct-port device session under the shared test authority.
    pub(crate) fn session_device_headers(device_id: &str) -> axum::http::HeaderMap {
        let authority = authority();
        let session = authority
            .key
            .sign_session(
                &authority.workspace_id,
                device_id,
                lucidos_gateway_stamp::now_unix(),
                lucidos_gateway_stamp::DEFAULT_SESSION_SECS,
            )
            .unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            crate::core::subprocess_origin::HEADER_DEVICE_ID,
            device_id.parse().unwrap(),
        );
        headers.insert(
            lucidos_gateway_stamp::HEADER_DEVICE_SESSION,
            session.parse().unwrap(),
        );
        headers
    }

    /// Add `device_id` and its stamp, as the gateway forwards a paired device.
    pub(crate) fn insert_stamped_device(headers: &mut axum::http::HeaderMap, device_id: &str) {
        headers.insert(
            crate::core::subprocess_origin::HEADER_DEVICE_ID,
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

    #[test]
    fn typed_operations_are_target_bound_and_single_use() {
        use lucidos_gateway_stamp::workspace_operation::{
            OperationEnvelope, ReadKind, ReadRequest, RoutingRequest, SignedOperation,
            WorkspaceOperation,
        };
        let authority = StampAuthority {
            key: StampKey::generate().unwrap(),
            workspace_id: "target".into(),
            operation_nonces: Default::default(),
        };
        let envelope = OperationEnvelope {
            request: RoutingRequest {
                via: Some("routing-gateway".into()),
                to: Some("target".into()),
                operation: WorkspaceOperation::Read {
                    request: ReadRequest {
                        kind: ReadKind::ThreadMessages,
                        thread_id: Some("thread-1".into()),
                        query: Default::default(),
                    },
                },
            },
            caller_gateway: "source-gateway".into(),
            caller_workspace: "source".into(),
            caller_thread_id: Uuid::new_v4().to_string(),
            verdict: String::new(),
            nonce: Uuid::new_v4().to_string(),
        };
        let digest = envelope.digest().unwrap();
        let stamp = authority
            .key
            .sign_now(Claim::WorkspaceOperation(envelope.claim(&digest)));
        let signed = SignedOperation { envelope, stamp };
        let mut changed = signed.clone();
        changed.envelope.request.to = Some("another-target".into());
        assert!(verify_operation_by(Some(&authority), &changed, None).is_none());
        assert!(verify_operation_by(None, &signed, None).is_none());
        let (headers, start) = verify_operation_by(Some(&authority), &signed, None).unwrap();
        assert!(
            !headers.contains_key(HEADER_CALLER_VERDICT),
            "a read cannot produce a spawn stamp"
        );
        assert_eq!(start, None, "a read carries no remote start");
        assert!(verify_operation_by(Some(&authority), &signed, None).is_none());
    }

    #[test]
    fn a_spawn_needs_the_remote_start_its_gateway_signed() {
        use lucidos_gateway_stamp::remote_start::{RemotePairing, RemoteStartSetting};
        use lucidos_gateway_stamp::workspace_operation::{
            OperationEnvelope, RoutingRequest, SignedOperation, WorkspaceOperation,
        };
        let authority = StampAuthority {
            key: StampKey::generate().unwrap(),
            workspace_id: "target".into(),
            operation_nonces: Default::default(),
        };
        let spawn = || {
            let envelope = OperationEnvelope {
                request: RoutingRequest {
                    via: Some("routing-gateway".into()),
                    to: Some("target".into()),
                    operation: WorkspaceOperation::Spawn {
                        body: serde_json::json!({"message": "Start"})
                            .as_object()
                            .unwrap()
                            .clone(),
                    },
                },
                caller_gateway: "source-gateway".into(),
                caller_workspace: "source".into(),
                caller_thread_id: Uuid::new_v4().to_string(),
                verdict: SpawnVerdict::Allowed.to_header(),
                nonce: Uuid::new_v4().to_string(),
            };
            let digest = envelope.digest().unwrap();
            let stamp = authority
                .key
                .sign_now(Claim::WorkspaceOperation(envelope.claim(&digest)));
            SignedOperation { envelope, stamp }
        };
        let ask = RemoteStart {
            setting: RemoteStartSetting::Ask,
            pairing: Some(RemotePairing {
                id: "pairing-1".into(),
                label: "Laptop".into(),
            }),
        };
        let signed = spawn();
        assert!(
            verify_operation_by(Some(&authority), &signed, None).is_none(),
            "a spawn without the header is refused"
        );
        let other_nonce = ask.header_value(&authority.key, "target", "another-nonce");
        assert!(verify_operation_by(Some(&authority), &signed, Some(&other_nonce)).is_none());
        let header = ask.header_value(&authority.key, "target", &signed.envelope.nonce);
        let (_, start) = verify_operation_by(Some(&authority), &signed, Some(&header)).unwrap();
        assert_eq!(start, Some(ask));
    }

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
    fn a_device_session_attests_only_its_device() {
        let headers = session_device_headers("d1");
        assert!(device_is_stamped(&headers, "d1"));
        assert!(!device_is_stamped(&headers, "d2"));
        assert!(!stamped_by(None, &headers, "d1"));
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
        headers.insert(
            crate::core::subprocess_origin::HEADER_DEVICE_ID,
            "d1".parse().unwrap(),
        );
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

    #[tokio::test]
    async fn the_arrival_middleware_pins_the_check_time_to_the_request_start() {
        use tower::ServiceExt as _;
        let slow_handler = tower::service_fn(|_req: axum::extract::Request| async {
            tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
            Ok::<_, std::convert::Infallible>(axum::response::IntoResponse::into_response(
                checked_at().to_string(),
            ))
        });
        let service = tower::ServiceBuilder::new()
            .layer(axum::middleware::from_fn(note_arrival))
            .service(slow_handler);
        let started = lucidos_gateway_stamp::now_unix();
        let response = service
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let checked: u64 = String::from_utf8(bytes.to_vec()).unwrap().parse().unwrap();
        assert!(
            checked <= started + 1,
            "checked at {checked}, started {started}"
        );
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
            operation_nonces: Default::default(),
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
