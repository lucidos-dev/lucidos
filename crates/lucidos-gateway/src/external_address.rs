use crate::net_config::BindChoice;
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum ProbeVerdict {
    Verified,
    Unreachable { detail: String },
    Unknown { detail: String },
    AnotherGateway,
}

impl ProbeVerdict {
    pub fn refuses(&self) -> bool {
        matches!(self, ProbeVerdict::AnotherGateway)
    }
}

#[derive(Debug)]
pub enum HealthAnswer {
    Failed(String),
    Status(u16),
    NotJson,
    Body(serde_json::Value),
}

pub fn classify(answer: HealthAnswer, ours: &str) -> ProbeVerdict {
    let body = match answer {
        HealthAnswer::Failed(detail) => return ProbeVerdict::Unreachable { detail },
        HealthAnswer::Status(code) => {
            return ProbeVerdict::Unknown {
                detail: format!("it answered with HTTP {code}, not with Lucidos's health"),
            }
        }
        HealthAnswer::NotJson => {
            return ProbeVerdict::Unknown {
                detail: "it answered, but not with Lucidos's health".to_string(),
            }
        }
        HealthAnswer::Body(body) => body,
    };
    let is_gateway =
        body.get("role").and_then(serde_json::Value::as_str) == Some(crate::peers::GATEWAY_ROLE);
    match body.get("instance_id").and_then(serde_json::Value::as_str) {
        Some(id) if id == ours => ProbeVerdict::Verified,
        Some(_) if is_gateway => ProbeVerdict::AnotherGateway,
        _ => ProbeVerdict::Unknown {
            detail: "a Lucidos answered, but too old to say which gateway it is".to_string(),
        },
    }
}

pub async fn probe(origin: &str) -> ProbeVerdict {
    classify(fetch_health(origin).await, crate::server::instance_id())
}

async fn fetch_health(origin: &str) -> HealthAnswer {
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .timeout(PROBE_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(e) => return HealthAnswer::Failed(format!("the probe could not start: {e}")),
    };
    let response = match client
        .get(format!("{origin}{}", crate::peers::GATEWAY_HEALTH_PATH))
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => return HealthAnswer::Failed(error_chain(&e)),
    };
    if !response.status().is_success() {
        return HealthAnswer::Status(response.status().as_u16());
    }
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => return HealthAnswer::Failed(error_chain(&e)),
    };
    match serde_json::from_slice(&bytes) {
        Ok(body) => HealthAnswer::Body(body),
        Err(_) => HealthAnswer::NotJson,
    }
}

pub(crate) fn error_chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub interface: String,
    pub origin: String,
}

pub fn suggestions(scheme: &str, port: u16, bind: BindChoice) -> Vec<Suggestion> {
    let mut found: Vec<Suggestion> = interface_addresses()
        .into_iter()
        .filter(|(_, ip)| suggestible(*ip) && listens_on(bind, *ip))
        .map(|(interface, ip)| Suggestion {
            interface,
            origin: format!("{scheme}://{ip}:{port}"),
        })
        .collect();
    found.dedup();
    found
}

fn suggestible(ip: Ipv4Addr) -> bool {
    !(ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_multicast())
}

fn listens_on(bind: BindChoice, ip: Ipv4Addr) -> bool {
    match bind {
        BindChoice::Loopback => ip.is_loopback(),
        BindChoice::All => true,
        BindChoice::Address(addr) => addr == IpAddr::V4(ip),
    }
}

fn interface_addresses() -> Vec<(String, Ipv4Addr)> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we own until freeifaddrs.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return out;
    }
    let up = (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_uint;
    let mut cursor = head;
    while !cursor.is_null() {
        // SAFETY: `cursor` walks the list getifaddrs returned, which is live.
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        if entry.ifa_addr.is_null() || entry.ifa_flags & up != up {
            continue;
        }
        // SAFETY: `ifa_addr` is non-null and points at a sockaddr.
        if i32::from(unsafe { (*entry.ifa_addr).sa_family }) != libc::AF_INET {
            continue;
        }
        // SAFETY: an AF_INET sockaddr is a sockaddr_in.
        let addr = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
        let ip = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
        // SAFETY: `ifa_name` is a NUL-terminated C string for a live entry.
        let name = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) };
        out.push((name.to_string_lossy().into_owned(), ip));
    }
    // SAFETY: `head` came from getifaddrs and is freed exactly once.
    unsafe { libc::freeifaddrs(head) };
    out
}

pub fn describe(origin: &str, verdict: &ProbeVerdict) -> String {
    match verdict {
        ProbeVerdict::Verified => format!("{origin} reaches this gateway"),
        ProbeVerdict::Unreachable { detail } => {
            format!("{origin} did not answer from this machine ({detail})")
        }
        ProbeVerdict::Unknown { detail } => format!("{origin} could not be checked ({detail})"),
        ProbeVerdict::AnotherGateway => format!("{origin} reaches a different Lucidos gateway"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OURS: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn our_own_instance_id_is_verified() {
        let body = json!({ "role": "gateway", "instance_id": OURS });
        assert_eq!(
            classify(HealthAnswer::Body(body), OURS),
            ProbeVerdict::Verified
        );
    }

    #[test]
    fn another_gateway_is_refused() {
        let body = json!({ "role": "gateway", "instance_id": "ffff" });
        let verdict = classify(HealthAnswer::Body(body), OURS);
        assert_eq!(verdict, ProbeVerdict::AnotherGateway);
        assert!(verdict.refuses());
    }

    #[test]
    fn silence_is_unreachable_and_saves() {
        let verdict = classify(HealthAnswer::Failed("connection refused".into()), OURS);
        assert_eq!(
            verdict,
            ProbeVerdict::Unreachable {
                detail: "connection refused".into()
            }
        );
        assert!(!verdict.refuses());
    }

    #[test]
    fn an_answer_that_cannot_be_compared_is_unknown_and_saves() {
        for answer in [
            HealthAnswer::Status(401),
            HealthAnswer::Status(302),
            HealthAnswer::NotJson,
            HealthAnswer::Body(json!({ "status": "ok" })),
            HealthAnswer::Body(json!({ "role": "gateway", "status": "ok" })),
            HealthAnswer::Body(json!({ "role": "engine", "instance_id": "ffff" })),
        ] {
            let verdict = classify(answer, OURS);
            assert!(
                matches!(verdict, ProbeVerdict::Unknown { .. }),
                "{verdict:?}"
            );
            assert!(!verdict.refuses());
        }
    }

    async fn stand_in(body: serde_json::Value) -> String {
        let router = axum::Router::new().route(
            crate::peers::GATEWAY_HEALTH_PATH,
            axum::routing::get(move || {
                let body = body.clone();
                async move { axum::Json(body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.ok() });
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn the_probe_tells_this_gateway_from_another_and_from_silence() {
        let ours = crate::server::instance_id();
        let here = stand_in(json!({ "role": "gateway", "instance_id": ours })).await;
        assert_eq!(probe(&here).await, ProbeVerdict::Verified);

        let other = stand_in(json!({ "role": "gateway", "instance_id": "ffff" })).await;
        assert_eq!(probe(&other).await, ProbeVerdict::AnotherGateway);

        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = format!("http://{}", free.local_addr().unwrap());
        drop(free);
        assert!(matches!(
            probe(&dead).await,
            ProbeVerdict::Unreachable { .. }
        ));
    }

    #[tokio::test]
    async fn the_probe_does_not_follow_a_redirect() {
        let ours = crate::server::instance_id();
        let here = stand_in(json!({ "role": "gateway", "instance_id": ours })).await;
        let target = format!("{here}{}", crate::peers::GATEWAY_HEALTH_PATH);
        let router = axum::Router::new().route(
            crate::peers::GATEWAY_HEALTH_PATH,
            axum::routing::get(move || {
                let target = target.clone();
                async move { axum::response::Redirect::temporary(&target) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.ok() });
        let verdict = probe(&format!("http://127.0.0.1:{port}")).await;
        assert!(
            matches!(verdict, ProbeVerdict::Unknown { .. }),
            "{verdict:?}"
        );
    }

    #[test]
    fn the_verdict_wire_shape_is_tagged_kebab_case() {
        assert_eq!(
            serde_json::to_value(ProbeVerdict::AnotherGateway).unwrap(),
            json!({ "verdict": "another-gateway" })
        );
        assert_eq!(
            serde_json::to_value(ProbeVerdict::Unknown { detail: "x".into() }).unwrap(),
            json!({ "verdict": "unknown", "detail": "x" })
        );
    }

    #[test]
    fn loopback_and_link_local_are_never_suggested() {
        assert!(!suggestible(Ipv4Addr::LOCALHOST));
        assert!(!suggestible(Ipv4Addr::new(169, 254, 3, 4)));
        assert!(!suggestible(Ipv4Addr::UNSPECIFIED));
        assert!(suggestible(Ipv4Addr::new(192, 168, 1, 5)));
        assert!(suggestible(Ipv4Addr::new(10, 8, 0, 2)));
        assert!(suggestible(Ipv4Addr::new(100, 70, 0, 1)));
    }

    #[test]
    fn suggestions_follow_what_the_gateway_listens_on() {
        let tailnet = Ipv4Addr::new(100, 70, 0, 1);
        let lan = Ipv4Addr::new(192, 168, 1, 5);
        assert!(!listens_on(BindChoice::Loopback, lan));
        assert!(listens_on(BindChoice::All, lan));
        assert!(listens_on(BindChoice::Address(tailnet.into()), tailnet));
        assert!(!listens_on(BindChoice::Address(tailnet.into()), lan));
        assert!(suggestions("http", 5252, BindChoice::Loopback).is_empty());
    }

    #[test]
    fn suggestions_are_this_gateways_origin_on_addresses_this_machine_holds() {
        for suggestion in suggestions("http", 5252, BindChoice::All) {
            let rest = suggestion.origin.strip_prefix("http://").unwrap();
            let (ip, port) = rest.rsplit_once(':').unwrap();
            assert_eq!(port, "5252");
            let ip: Ipv4Addr = ip.parse().unwrap();
            assert!(std::net::UdpSocket::bind((ip, 0)).is_ok(), "{suggestion:?}");
            assert!(!suggestion.interface.is_empty());
            assert!(
                crate::net_config::valid_external_address(&suggestion.origin).is_ok(),
                "{suggestion:?}"
            );
        }
    }
}
