//! What landing asks and tells the gateway that spawned this engine (ADR 0589).
//!
//! - [`landing_order`]: does `main` land on `origin` first? Only while the
//!   gateway holds a pairing, in either direction.
//! - [`report_main_landed`]: a push moved `origin`'s `main`. The gateway then
//!   nudges every remote gateway it is paired with, so their engines follow at
//!   once instead of at their next polling round.
//!
//! The report is best-effort like `boot_report`: detached, short timeout,
//! failures logged only. A lost report costs a follower one polling interval.
//!
//! Both calls reach the gateway only once [`enable`] has run. `main` calls it
//! for a gateway-spawned engine. A test inherits the gateway's environment
//! from the session that runs it, and must never ask or nudge real gateways.

use crate::engine::git_ops::LandingOrder;
use std::sync::OnceLock;
use std::time::Duration;

/// The control routes, under the gateway's `/~/api/v1/control` mount.
const MAIN_LANDED_PATH: &str = "/~/api/v1/control/main-landed";
const GATEWAY_PAIRINGS_PATH: &str = "/~/api/v1/control/gateway-pairings";

const GATEWAY_TIMEOUT: Duration = Duration::from_secs(5);

static GATEWAY_PORT: OnceLock<String> = OnceLock::new();

/// Talk to the gateway that spawned this engine. No-op without one.
pub fn enable() {
    if let Ok(port) = std::env::var("LUCIDOS_GATEWAY_PORT") {
        let _ = GATEWAY_PORT.set(port);
    }
}

/// Origin-first only while the gateway holds a pairing. An engine with no
/// gateway is unpaired. A gateway that predates the question has no pairings.
///
/// A packaged install follows releases, not `origin`, so nothing would ever
/// retry a landing that waited for `origin`. It always lands local-first.
pub(crate) async fn landing_order() -> LandingOrder {
    order_for(
        crate::runtime::is_packaged(),
        GATEWAY_PORT.get().map(String::as_str),
    )
    .await
}

async fn order_for(packaged: bool, gateway_port: Option<&str>) -> LandingOrder {
    let Some(port) = gateway_port.filter(|_| !packaged) else {
        return LandingOrder::LocalFirst;
    };
    match gateway_paired(port).await {
        Ok(true) => LandingOrder::OriginFirst,
        Ok(false) => LandingOrder::LocalFirst,
        Err(e) => LandingOrder::Unknown(format!(
            "the gateway did not say whether it holds a pairing: {e}"
        )),
    }
}

async fn gateway_paired(port: &str) -> Result<bool, String> {
    let client = gateway_client()?;
    let mut last_error = String::new();
    for scheme in crate::net_config::peer_scheme_order() {
        let url = format!("{scheme}://127.0.0.1:{port}{GATEWAY_PAIRINGS_PATH}");
        match client.get(&url).send().await {
            Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => {
                return Ok(false)
            }
            Ok(response) if response.status().is_success() => {
                let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
                return body["paired"]
                    .as_bool()
                    .ok_or_else(|| "the answer had no paired field".to_string());
            }
            Ok(response) => last_error = format!("HTTP {}", response.status()),
            Err(e) => last_error = e.to_string(),
        }
    }
    Err(last_error)
}

/// A push moved `origin`'s `main`. Returns at once.
pub fn report_main_landed() {
    let Some(port) = GATEWAY_PORT.get().cloned() else {
        return;
    };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        let client = match gateway_client() {
            Ok(c) => c,
            Err(e) => {
                log!("[LandingGateway] {}", e);
                return;
            }
        };
        let mut last_error = String::new();
        for scheme in crate::net_config::peer_scheme_order() {
            let url = format!("{scheme}://127.0.0.1:{port}{MAIN_LANDED_PATH}");
            match client.post(&url).send().await {
                Ok(response) if response.status().is_success() => return,
                Ok(response) => last_error = format!("HTTP {}", response.status()),
                Err(e) => last_error = e.to_string(),
            }
        }
        log!(
            "[LandingGateway] the gateway did not take the landing report: {}",
            last_error
        );
    });
}

fn gateway_client() -> Result<reqwest::Client, String> {
    crate::gateway_auth::client_builder()
        .timeout(GATEWAY_TIMEOUT)
        .build()
        .map_err(|e| format!("could not build a gateway client: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Neither case asks the gateway: the port names nothing that listens.
    #[tokio::test]
    async fn packaged_or_gatewayless_engines_land_local_first() {
        assert_eq!(order_for(true, Some("1")).await, LandingOrder::LocalFirst);
        assert_eq!(order_for(false, None).await, LandingOrder::LocalFirst);
    }

    #[tokio::test]
    async fn a_gateway_that_does_not_answer_makes_the_order_unknown() {
        let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = unused.local_addr().unwrap().port().to_string();
        drop(unused);
        assert!(matches!(
            order_for(false, Some(&port)).await,
            LandingOrder::Unknown(_)
        ));
    }
}
