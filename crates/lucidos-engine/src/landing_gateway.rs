use crate::engine::git_ops::LandingOrder;
use std::sync::OnceLock;
use std::time::Duration;

const MAIN_LANDED_PATH: &str = "/~/api/v1/control/main-landed";
const GATEWAY_PAIRINGS_PATH: &str = "/~/api/v1/control/gateway-pairings";

const GATEWAY_TIMEOUT: Duration = Duration::from_secs(5);

static GATEWAY_PORT: OnceLock<String> = OnceLock::new();

pub fn enable() {
    if let Ok(port) = std::env::var("LUCIDOS_GATEWAY_PORT") {
        let _ = GATEWAY_PORT.set(port);
    }
}

/// INVARIANT: Origin-first only while the gateway holds a pairing. An engine with no
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
    let url = format!(
        "{}{GATEWAY_PAIRINGS_PATH}",
        lucidos_local_token::client::origin(port)
    );
    let response = client.get(&url).send().await.map_err(|e| e.to_string())?;
    match response.status() {
        reqwest::StatusCode::NOT_FOUND => Ok(false),
        status if status.is_success() => {
            let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
            body["paired"]
                .as_bool()
                .ok_or_else(|| "the answer had no paired field".to_string())
        }
        status => Err(format!("HTTP {status}")),
    }
}

pub(crate) fn local_label() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes within the supplied buffer.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|&byte| byte == 0).unwrap_or(buf.len());
    let hostname = String::from_utf8_lossy(&buf[..end]);
    let hostname = hostname.trim();
    (!hostname.is_empty()).then(|| format!("Lucidos on {hostname}"))
}

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
                log_legacy!("[LandingGateway] {}", e);
                return;
            }
        };
        let url = format!(
            "{}{MAIN_LANDED_PATH}",
            lucidos_local_token::client::origin(&port)
        );
        let refusal = match client.post(&url).send().await {
            Ok(response) if response.status().is_success() => return,
            Ok(response) => format!("HTTP {}", response.status()),
            Err(e) => e.to_string(),
        };
        log_legacy!(
            "[LandingGateway] the gateway did not take the landing report: {}",
            refusal
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
