//! Whether usage telemetry is on, and where it goes.
//!
//! The gateway's managed settings turn it on with Claude Code's own switches.
//! The protocol locks the export to `{base}/v1/logs` with the bearer. Only the
//! policy can name a collector on another host, which never sees the bearer.
//! The engine's own environment can opt out, as Claude Code's can.

use reqwest::header::{HeaderName, HeaderValue};

use crate::llm::claude_gateway::gateway_managed_settings::GatewayManagedSettings;
use crate::llm::claude_gateway::{is_loopback, GatewayOrigin};

/// The OTLP/HTTP encoding a batch is sent in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Protobuf,
    Json,
}

impl Protocol {
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Protobuf => "application/x-protobuf",
            Self::Json => "application/json",
        }
    }
}

/// Who receives the batch, which decides what authenticates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// The gateway itself, with the bearer.
    Gateway,
    /// A collector on another host, with the policy's own headers. Never the
    /// bearer.
    Collector { headers: Vec<(String, String)> },
}

/// Where telemetry goes when it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    pub protocol: Protocol,
    pub destination: Destination,
}

/// Why telemetry is off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Off {
    /// The engine's environment opts out.
    OptedOut,
    /// The gateway's policy does not ask for telemetry.
    NotRequested,
    /// The policy names a transport Lucidos does not speak, such as `grpc`.
    UnsupportedProtocol(String),
    /// The policy names a collector Lucidos will not send to.
    UnusableEndpoint(String),
}

impl std::fmt::Display for Off {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OptedOut => f.write_str(
                "the engine's environment opts out (CLAUDE_CODE_ENABLE_TELEMETRY=0 or \
                 DISABLE_TELEMETRY=1)",
            ),
            Self::NotRequested => f.write_str("the gateway's managed settings do not ask for it"),
            Self::UnsupportedProtocol(protocol) => write!(
                f,
                "the managed settings ask for OTLP over {protocol}, and Lucidos sends only \
                 http/protobuf or http/json"
            ),
            Self::UnusableEndpoint(why) => write!(f, "the managed settings' collector {why}"),
        }
    }
}

/// Reads one variable of the engine's environment, for the opt-out.
pub type EngineEnv = fn(&str) -> Option<String>;

/// The engine process's own environment.
pub fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Whether the engine's environment opts out, as Claude Code's can.
pub fn opted_out(engine_env: EngineEnv) -> bool {
    engine_env("CLAUDE_CODE_ENABLE_TELEMETRY").as_deref() == Some("0")
        || engine_env("DISABLE_TELEMETRY").as_deref() == Some("1")
}

/// Where telemetry for the gateway at `origin` goes under `policy`, or why it
/// is off. `engine_env` decides the opt-out.
pub fn target(
    origin: &GatewayOrigin,
    policy: Option<&GatewayManagedSettings>,
    engine_env: EngineEnv,
) -> Result<Target, Off> {
    if opted_out(engine_env) {
        return Err(Off::OptedOut);
    }
    let policy = policy
        .filter(|policy| policy.telemetry_requested())
        .ok_or(Off::NotRequested)?;
    let protocol = match policy.env("OTEL_EXPORTER_OTLP_PROTOCOL") {
        None | Some("http/protobuf") => Protocol::Protobuf,
        Some("http/json") => Protocol::Json,
        Some(other) => return Err(Off::UnsupportedProtocol(other.to_string())),
    };
    let endpoint = policy
        .env("OTEL_EXPORTER_OTLP_ENDPOINT")
        .filter(|endpoint| !origin.owns(endpoint));
    let Some(endpoint) = endpoint else {
        return Ok(Target {
            url: format!("{origin}/v1/logs"),
            protocol,
            destination: Destination::Gateway,
        });
    };
    let url = collector_url(endpoint).map_err(Off::UnusableEndpoint)?;
    let headers = policy
        .env("OTEL_EXPORTER_OTLP_HEADERS")
        .map(parse_headers)
        .transpose()
        .map_err(Off::UnusableEndpoint)?
        .unwrap_or_default();
    Ok(Target {
        url,
        protocol,
        destination: Destination::Collector { headers },
    })
}

/// `{endpoint}/v1/logs` for a collector on another host: https, or plain
/// http on a loopback host, with no credentials in the URL.
fn collector_url(endpoint: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(endpoint).map_err(|_| format!("'{endpoint}' is not a URL"))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    match parsed.scheme() {
        "https" => {}
        "http" if is_loopback(&host) => {}
        _ => return Err(format!("'{endpoint}' must use https")),
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("'{endpoint}' must not carry credentials"));
    }
    Ok(format!("{}/v1/logs", endpoint.trim_end_matches('/')))
}

/// `OTEL_EXPORTER_OTLP_HEADERS`: comma-separated `key=value` pairs, each value
/// percent-encoded, as the OTLP exporter specification writes them.
fn parse_headers(raw: &str) -> Result<Vec<(String, String)>, String> {
    raw.split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| "headers are not key=value pairs".to_string())?;
            let name = name.trim();
            let value = urlencoding::decode(value.trim())
                .map_err(|_| format!("header {name} is not percent-encoded UTF-8"))?;
            if HeaderName::from_bytes(name.as_bytes()).is_err()
                || HeaderValue::from_str(&value).is_err()
            {
                return Err(format!("header {name} is not a valid HTTP header"));
            }
            Ok((name.to_string(), value.into_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const GATEWAY: &str = "https://gateway.example";

    fn origin() -> GatewayOrigin {
        GatewayOrigin::parse(GATEWAY).unwrap()
    }

    fn policy(extra: Value) -> GatewayManagedSettings {
        let mut env = json!({
            "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
            "OTEL_LOGS_EXPORTER": "otlp"
        });
        env.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        GatewayManagedSettings::from_document(&json!({ "env": env }))
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn target_of(policy: &GatewayManagedSettings) -> Result<Target, Off> {
        target(&origin(), Some(policy), no_env)
    }

    #[test]
    fn with_no_endpoint_the_gateway_receives_it() {
        assert_eq!(
            target_of(&policy(json!({}))),
            Ok(Target {
                url: format!("{GATEWAY}/v1/logs"),
                protocol: Protocol::Protobuf,
                destination: Destination::Gateway,
            })
        );
    }

    /// An endpoint on the gateway origin is the gateway, whatever its path:
    /// the protocol locks the export to `{base}/v1/logs`.
    #[test]
    fn an_endpoint_naming_the_gateway_is_the_gateway() {
        let target = target_of(&policy(json!({
            "OTEL_EXPORTER_OTLP_ENDPOINT": "https://GATEWAY.example/otel",
            "OTEL_EXPORTER_OTLP_PROTOCOL": "http/json"
        })))
        .unwrap();
        assert_eq!(target.url, format!("{GATEWAY}/v1/logs"));
        assert_eq!(target.protocol, Protocol::Json);
        assert_eq!(target.destination, Destination::Gateway);
    }

    #[test]
    fn a_collector_on_another_host_gets_its_own_headers() {
        let target = target_of(&policy(json!({
            "OTEL_EXPORTER_OTLP_ENDPOINT": "https://collector.example/",
            "OTEL_EXPORTER_OTLP_HEADERS": "x-api-key=abc%3D%3D, x-team=platform"
        })))
        .unwrap();
        assert_eq!(target.url, "https://collector.example/v1/logs");
        assert_eq!(
            target.destination,
            Destination::Collector {
                headers: vec![
                    ("x-api-key".into(), "abc==".into()),
                    ("x-team".into(), "platform".into())
                ]
            }
        );
    }

    #[test]
    fn an_unusable_collector_turns_it_off() {
        for (endpoint, headers) in [
            ("http://collector.example", ""),
            ("ftp://collector.example", ""),
            ("not a url", ""),
            ("https://user:pw@collector.example", ""),
            ("https://collector.example", "no-equals-sign"),
            ("https://collector.example", "bad header=x"),
        ] {
            let result = target_of(&policy(json!({
                "OTEL_EXPORTER_OTLP_ENDPOINT": endpoint,
                "OTEL_EXPORTER_OTLP_HEADERS": headers
            })));
            assert!(
                matches!(result, Err(Off::UnusableEndpoint(_))),
                "{endpoint} {headers}: {result:?}"
            );
        }
        assert!(target_of(&policy(json!({
            "OTEL_EXPORTER_OTLP_ENDPOINT": "http://127.0.0.1:4318"
        })))
        .is_ok());
    }

    #[test]
    fn only_the_http_transports_are_spoken() {
        assert_eq!(
            target_of(&policy(json!({"OTEL_EXPORTER_OTLP_PROTOCOL": "grpc"}))),
            Err(Off::UnsupportedProtocol("grpc".into()))
        );
    }

    #[test]
    fn it_is_off_unless_the_policy_asks() {
        assert_eq!(target(&origin(), None, no_env), Err(Off::NotRequested));
        let silent = GatewayManagedSettings::from_document(&json!({"env": {}}));
        assert_eq!(
            target(&origin(), Some(&silent), no_env),
            Err(Off::NotRequested)
        );
    }

    /// The engine's own opt-out wins over the policy, as Claude Code's does.
    #[test]
    fn the_engine_environment_opts_out() {
        let opted_out: [EngineEnv; 2] = [
            |k| (k == "CLAUDE_CODE_ENABLE_TELEMETRY").then(|| "0".to_string()),
            |k| (k == "DISABLE_TELEMETRY").then(|| "1".to_string()),
        ];
        for env in opted_out {
            assert_eq!(
                target(&origin(), Some(&policy(json!({}))), env),
                Err(Off::OptedOut)
            );
        }
        let unrelated: EngineEnv = |k| (k == "DISABLE_TELEMETRY").then(|| "0".to_string());
        assert!(target(&origin(), Some(&policy(json!({}))), unrelated).is_ok());
    }
}
