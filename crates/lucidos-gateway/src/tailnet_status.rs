//! This machine's tailnet state, for Settings → Access.
//!
//! The gateway owns host concerns, so the gateway answers it, once for every
//! workspace it serves. The page joins `serve_origin` to the workspace's own
//! path itself.

use crate::external_address::{probe, ProbeVerdict};
use axum::extract::State;
use axum::Json;
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// INVARIANT: How long an answer is reused before the probes run again.
///
/// Each miss asks Tailscale, which can mean a bounded `lsof` or CLI run, plus
/// one outbound TLS connection per [`SERVE_PORTS`] entry. Short on purpose:
/// the Access page refetches right after an Expose run, which takes far longer.
const TTL: Duration = Duration::from_secs(5);

/// Every port `tailscale serve` can bind for HTTPS.
const SERVE_PORTS: [u16; 3] = [443, 8443, 10000];

/// INVARIANT: The last answer and when it was produced. In-memory cache,
/// rebuilt by the next miss, so a restart loses nothing.
static CACHE: Mutex<Option<(Instant, TailnetStatus)>> = Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TailnetStatus {
    pub magic_dns_name: Option<String>,
    /// A `tailscale serve` origin that answers as THIS gateway.
    pub serve_origin: Option<String>,
    pub reason: Option<String>,
}

/// GET /~/api/v1/control/tailnet-status
pub async fn tailnet_status(State(_): State<crate::server::GatewayState>) -> Json<TailnetStatus> {
    if let Some(fresh) = cached() {
        return Json(fresh);
    }
    let tailnet = tokio::task::spawn_blocking(lucidos_tailscale::tailnet_state)
        .await
        .unwrap_or(lucidos_tailscale::TailnetState::NotRunning);
    let node = tailnet.node();
    let serve_origin = match node.and_then(|n| n.https_name()) {
        Some(name) => {
            verified_serve_origin(name, |origin| async move { probe(&origin).await }).await
        }
        None => None,
    };
    let answer = TailnetStatus {
        magic_dns_name: node.and_then(|n| n.magic_dns_name.clone()),
        serve_origin,
        reason: tailnet.reason(),
    };
    *lock() = Some((Instant::now(), answer.clone()));
    Json(answer)
}

fn cached() -> Option<TailnetStatus> {
    lock()
        .as_ref()
        .filter(|(at, _)| at.elapsed() < TTL)
        .map(|(_, answer)| answer.clone())
}

fn lock() -> std::sync::MutexGuard<'static, Option<(Instant, TailnetStatus)>> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

fn serve_origin(magic_dns_name: &str, port: u16) -> String {
    match port {
        443 => format!("https://{magic_dns_name}"),
        p => format!("https://{magic_dns_name}:{p}"),
    }
}

/// The first serve port, in [`SERVE_PORTS`] order, that reaches this gateway.
/// A port that answers as another gateway, or not at all, is never published.
async fn verified_serve_origin<F, Fut>(magic_dns_name: &str, probe: F) -> Option<String>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = ProbeVerdict>,
{
    let check = |port| {
        let origin = serve_origin(magic_dns_name, port);
        let verdict = probe(origin.clone());
        async move { (verdict.await == ProbeVerdict::Verified).then_some(origin) }
    };
    let (first, second, third) = tokio::join!(
        check(SERVE_PORTS[0]),
        check(SERVE_PORTS[1]),
        check(SERVE_PORTS[2])
    );
    [first, second, third].into_iter().flatten().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "mymac.tailnet-name.ts.net";

    #[test]
    fn a_serve_route_off_443_carries_its_port() {
        assert_eq!(serve_origin(NAME, 10000), format!("https://{NAME}:10000"));
        assert_eq!(serve_origin(NAME, 8443), format!("https://{NAME}:8443"));
        assert_eq!(serve_origin(NAME, 443), format!("https://{NAME}"));
    }

    async fn verified_where(ours: &'static [u16]) -> Option<String> {
        verified_serve_origin(NAME, |origin| async move {
            let ours = ours.iter().any(|port| serve_origin(NAME, *port) == origin);
            if ours {
                ProbeVerdict::Verified
            } else {
                ProbeVerdict::AnotherGateway
            }
        })
        .await
    }

    #[tokio::test]
    async fn only_a_port_that_answers_as_this_gateway_is_published() {
        assert_eq!(verified_where(&[]).await, None);
        assert_eq!(
            verified_where(&[8443]).await,
            Some(format!("https://{NAME}:8443"))
        );
    }

    #[tokio::test]
    async fn the_lowest_serve_port_wins_when_several_reach_us() {
        assert_eq!(
            verified_where(&[10000, 443]).await,
            Some(format!("https://{NAME}"))
        );
    }

    #[tokio::test]
    async fn an_unreachable_or_unknown_port_is_never_published() {
        let answer = verified_serve_origin(NAME, |origin| async move {
            if origin.ends_with(":8443") {
                ProbeVerdict::Unknown {
                    detail: "old".into(),
                }
            } else {
                ProbeVerdict::Unreachable {
                    detail: "down".into(),
                }
            }
        })
        .await;
        assert_eq!(answer, None);
    }
}
