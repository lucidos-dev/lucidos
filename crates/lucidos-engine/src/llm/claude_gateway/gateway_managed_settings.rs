//! The *gateway managed settings*: the policy a Claude gateway sets for the
//! signed-in user, served at `GET {base}/managed/settings`.
//!
//! The protocol calls the body the user's Claude Code `managed-settings.json`.
//! Lucidos reads only the model allowlist and the telemetry switches from it.
//! No other setting reaches any process.
//!
//! The document is fetched after a sign-in, after each token refresh, and
//! hourly. It is stored on the account row, so a restart keeps it and a
//! sign-out drops it. A failed fetch keeps the last stored document.

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Map, Value};
use sqlx::PgPool;

use super::account::{ClaudeGatewayAccount, ClaudeGatewayAccountStore};
use super::token_endpoint::{excerpt, http_client};
use super::{GatewayAccessFactory, GatewayOrigin};
use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus, SystemEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Where the gateway serves its managed settings, relative to the origin.
const MANAGED_SETTINGS_PATH: &str = "/managed/settings";

/// How often the background task asks whether a fetch is due.
const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// One gateway's managed settings for one user. Holds a JSON object; anything
/// else reads as an empty policy.
#[derive(Clone, Default, PartialEq)]
pub struct GatewayManagedSettings(Map<String, Value>);

impl std::fmt::Debug for GatewayManagedSettings {
    /// Names the keys only: an `env` value can be a collector's credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

impl GatewayManagedSettings {
    pub fn from_document(document: &Value) -> Self {
        Self(document.as_object().cloned().unwrap_or_default())
    }

    /// The string value of `key` in the policy's `env` block.
    pub fn env(&self, key: &str) -> Option<&str> {
        self.0.get("env")?.get(key)?.as_str()
    }

    /// The policy's `availableModels` entries, verbatim. `None` when it sets
    /// none, an empty list included: a policy allowing no model at all would
    /// leave the gateway unusable, and the gateway still refuses what it bars.
    pub fn available_models(&self) -> Option<Vec<String>> {
        let entries: Vec<String> = self
            .0
            .get("availableModels")?
            .as_array()?
            .iter()
            .filter_map(|entry| entry.as_str().map(str::to_string))
            .collect();
        (!entries.is_empty()).then_some(entries)
    }

    /// The model ids the allowlist serves, when the policy sets one. Each
    /// entry loses a trailing `[...]` (as in `[1m]`), and an `id@alias`
    /// entry, which only names how telemetry reports a model, is dropped.
    /// `None` when nothing is left, so the gateway's listing stays in charge.
    pub fn served_ids(&self) -> Option<Vec<String>> {
        let mut ids: Vec<String> = Vec::new();
        for entry in self.available_models()? {
            let id = strip_bracket_suffix(&entry);
            if !id.contains('@') && !ids.iter().any(|seen| seen == id) {
                ids.push(id.to_string());
            }
        }
        (!ids.is_empty()).then_some(ids)
    }

    /// How telemetry names `model`: `id-alias` when the allowlist holds an
    /// `id@alias` entry for it, else its id. A `[1m]` suffix is dropped.
    pub fn reported_model(&self, model: &str) -> String {
        let id = strip_bracket_suffix(model);
        let prefix = format!("{id}@");
        self.available_models()
            .into_iter()
            .flatten()
            .find_map(|entry| {
                let alias = strip_bracket_suffix(&entry).strip_prefix(&prefix)?;
                (!alias.is_empty()).then(|| format!("{id}-{alias}"))
            })
            .unwrap_or_else(|| id.to_string())
    }

    /// Whether the policy asks a client for usage telemetry: Claude Code's own
    /// switches, `CLAUDE_CODE_ENABLE_TELEMETRY=1` with an OTLP logs exporter.
    pub fn telemetry_requested(&self) -> bool {
        self.env("CLAUDE_CODE_ENABLE_TELEMETRY") == Some("1")
            && self.env("OTEL_LOGS_EXPORTER") == Some("otlp")
    }
}

/// `entry` without a trailing `[...]`, such as the `[1m]` of a 1M model.
fn strip_bracket_suffix(entry: &str) -> &str {
    match entry.strip_suffix(']').and_then(|rest| rest.rfind('[')) {
        Some(open) => &entry[..open],
        None => entry,
    }
}

/// What one fetch found.
#[derive(Debug, PartialEq)]
pub(crate) enum Fetched {
    /// `304`: the stored document still holds.
    Unchanged,
    /// `404`: the gateway has no policy for this user.
    NoPolicy,
    /// A policy, with the `ETag` to send back next time.
    Policy {
        document: Value,
        etag: Option<String>,
    },
}

/// Ask `origin` for its managed settings, sending `etag` as `If-None-Match`.
pub(crate) async fn fetch(
    origin: &GatewayOrigin,
    bearer: &str,
    etag: Option<&str>,
) -> Result<Fetched, BoxError> {
    let url = format!("{origin}{MANAGED_SETTINGS_PATH}");
    let mut request = http_client()
        .get(&url)
        .bearer_auth(bearer)
        .header("Accept", "application/json");
    if let Some(etag) = etag {
        request = request.header("If-None-Match", etag);
    }
    let resp = request
        .send()
        .await
        .map_err(|e| format!("Could not ask {url} for the gateway's managed settings: {e}"))?;
    let status = resp.status();
    match status {
        StatusCode::NOT_MODIFIED => return Ok(Fetched::Unchanged),
        StatusCode::NOT_FOUND => return Ok(Fetched::NoPolicy),
        _ => {}
    }
    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = resp.text().await?;
    if status == StatusCode::UNAUTHORIZED {
        return Err(super::unauthorized(&body).into());
    }
    if !status.is_success() {
        return Err(format!("{url} answered {status}: {}", excerpt(&body)).into());
    }
    let value: Value = serde_json::from_str(&body)
        .map_err(|e| format!("{url} answered with no JSON document: {e}"))?;
    Ok(Fetched::Policy {
        document: unwrapped(value)?,
        etag,
    })
}

/// The policy itself. The protocol sends it as the body, and some gateways
/// wrap it as `{"settings": {...}}`. Either is read.
fn unwrapped(value: Value) -> Result<Value, String> {
    let document = match value {
        Value::Object(mut body) => match body.remove("settings") {
            Some(settings @ Value::Object(_)) => settings,
            Some(other) => {
                body.insert("settings".into(), other);
                Value::Object(body)
            }
            None => Value::Object(body),
        },
        other => {
            return Err(format!(
                "the managed settings are not a JSON object: {other}"
            ))
        }
    };
    Ok(document)
}

/// Fetch the managed settings for `account`, the stored sign-in, with
/// `bearer`, and store them. A failure is logged and keeps the stored ones.
pub(crate) async fn refresh(
    pool: &PgPool,
    event_bus: &EventBus,
    account: &ClaudeGatewayAccount,
    bearer: &str,
) {
    let Ok(origin) = GatewayOrigin::parse(&account.issuer) else {
        return;
    };
    let fetched = match fetch(&origin, bearer, account.managed_settings_etag.as_deref()).await {
        Ok(fetched) => fetched,
        Err(e) => {
            crate::log!("[ClaudeGateway] Keeping the stored managed settings: {e}");
            return;
        }
    };
    if let Err(e) =
        ClaudeGatewayAccountStore::store_managed_settings(pool, event_bus, account, fetched).await
    {
        crate::log!("[ClaudeGateway] Could not store the gateway's managed settings: {e}");
    }
}

/// Fetch the managed settings when a fetch is due: after a sign-in, and once
/// the stored ones are an hour old. A restart picks up where it left off,
/// since the fetch time is stored.
pub(crate) fn spawn_refresh_task(
    mut rx: tokio::sync::broadcast::Receiver<EmittedEvent>,
    pool: PgPool,
    event_bus: EventBus,
    access: GatewayAccessFactory,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(CHECK_INTERVAL);
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                received = rx.recv() => match received {
                    Ok(emitted) => match emitted.typed {
                        BusEvent::System(SystemEvent::ClaudeGatewaySignedIn { .. }) => {}
                        _ => continue,
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
            }
            refresh_if_due(&pool, &event_bus, &access).await;
        }
    });
}

async fn refresh_if_due(pool: &PgPool, event_bus: &EventBus, access: &GatewayAccessFactory) {
    let account = match ClaudeGatewayAccountStore::managed_settings_due(pool).await {
        Ok(Some(account)) => account,
        Ok(None) => return,
        Err(e) => {
            crate::log!("[ClaudeGateway] Could not read the sign-in: {e}");
            return;
        }
    };
    let Ok(origin) = GatewayOrigin::parse(&account.issuer) else {
        return;
    };
    match access(origin).bearer().await {
        Ok(bearer) => refresh(pool, event_bus, &account, &bearer).await,
        Err(e) => crate::log!("[ClaudeGateway] Not fetching the managed settings: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::claude_gateway::mock_gateway::MockGateway;
    use axum::http::StatusCode as AxumStatus;
    use serde_json::json;

    #[test]
    fn telemetry_needs_both_switches() {
        let policy = |env: Value| GatewayManagedSettings::from_document(&json!({ "env": env }));
        assert!(policy(json!({
            "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
            "OTEL_LOGS_EXPORTER": "otlp"
        }))
        .telemetry_requested());
        for env in [
            json!({"CLAUDE_CODE_ENABLE_TELEMETRY": "1"}),
            json!({"OTEL_LOGS_EXPORTER": "otlp"}),
            json!({"CLAUDE_CODE_ENABLE_TELEMETRY": "0", "OTEL_LOGS_EXPORTER": "otlp"}),
            json!({"CLAUDE_CODE_ENABLE_TELEMETRY": 1, "OTEL_LOGS_EXPORTER": "otlp"}),
            json!({"CLAUDE_CODE_ENABLE_TELEMETRY": "1", "OTEL_LOGS_EXPORTER": "console"}),
        ] {
            assert!(!policy(env.clone()).telemetry_requested(), "{env}");
        }
        assert!(!GatewayManagedSettings::default().telemetry_requested());
    }

    #[test]
    fn an_empty_allowlist_is_no_allowlist() {
        let policy = GatewayManagedSettings::from_document(&json!({"availableModels": []}));
        assert_eq!(policy.available_models(), None);
        let policy = GatewayManagedSettings::from_document(
            &json!({"availableModels": ["claude-opus-5-5", 7]}),
        );
        assert_eq!(
            policy.available_models(),
            Some(vec!["claude-opus-5-5".into()])
        );
    }

    #[test]
    fn served_ids_strip_the_suffix_and_drop_aliases() {
        let policy = GatewayManagedSettings::from_document(&json!({"availableModels": [
            "claude-opus-5-5[1m]",
            "claude-opus-5-5",
            "claude-haiku-5-5@fast",
            "claude-sonnet-5-5"
        ]}));
        assert_eq!(
            policy.served_ids(),
            Some(vec!["claude-opus-5-5".into(), "claude-sonnet-5-5".into()])
        );
        assert_eq!(GatewayManagedSettings::default().served_ids(), None);
        let aliases_only = GatewayManagedSettings::from_document(
            &json!({"availableModels": ["claude-sonnet-5-5@20261001"]}),
        );
        assert_eq!(
            aliases_only.served_ids(),
            None,
            "the listing stays in charge"
        );
    }

    /// An `id@alias` entry names how telemetry reports that model.
    #[test]
    fn the_reported_model_follows_an_alias_entry() {
        let policy = GatewayManagedSettings::from_document(&json!({"availableModels": [
            "claude-opus-5-5@20261001",
            "claude-haiku-5-5"
        ]}));
        assert_eq!(
            policy.reported_model("claude-opus-5-5[1m]"),
            "claude-opus-5-5-20261001"
        );
        assert_eq!(
            policy.reported_model("claude-haiku-5-5"),
            "claude-haiku-5-5"
        );
        assert_eq!(
            GatewayManagedSettings::default().reported_model("claude-opus-5[1m]"),
            "claude-opus-5"
        );
    }

    #[test]
    fn debug_shows_keys_never_values() {
        let policy = GatewayManagedSettings::from_document(
            &json!({"env": {"OTEL_EXPORTER_OTLP_HEADERS": "Authorization=secret"}}),
        );
        let shown = format!("{policy:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("env"), "{shown}");
    }

    #[test]
    fn a_settings_wrapper_is_read_through() {
        let policy = json!({"availableModels": ["claude-opus-5-5"]});
        assert_eq!(
            unwrapped(json!({ "settings": policy.clone() })).unwrap(),
            policy
        );
        assert_eq!(unwrapped(policy.clone()).unwrap(), policy);
        assert!(unwrapped(json!(["not", "an", "object"])).is_err());
    }

    /// `404` means no policy and `200 {}` an empty one. The protocol keeps
    /// them apart, and so does the fetch.
    #[tokio::test]
    async fn no_policy_and_an_empty_policy_differ() {
        let gateway = MockGateway::start().await;
        assert_eq!(
            fetch(&gateway.origin, "gw-token", None).await.unwrap(),
            Fetched::NoPolicy
        );
        gateway.serve_managed_settings(AxumStatus::OK, json!({}), Some("\"v1\""));
        assert_eq!(
            fetch(&gateway.origin, "gw-token", None).await.unwrap(),
            Fetched::Policy {
                document: json!({}),
                etag: Some("\"v1\"".into())
            }
        );
    }

    /// The stored `ETag` goes back as `If-None-Match`, with the bearer, and a
    /// matching one answers `304`.
    #[tokio::test]
    async fn a_matching_etag_is_unchanged() {
        let gateway = MockGateway::start().await;
        gateway.serve_managed_settings(AxumStatus::OK, json!({"env": {}}), Some("\"v1\""));
        assert_eq!(
            fetch(&gateway.origin, "gw-token", Some("\"v1\""))
                .await
                .unwrap(),
            Fetched::Unchanged
        );
        let seen = gateway
            .seen()
            .into_iter()
            .find(|s| s.path == MANAGED_SETTINGS_PATH)
            .unwrap();
        assert_eq!(seen.authorization.as_deref(), Some("Bearer gw-token"));
        assert_eq!(seen.if_none_match.as_deref(), Some("\"v1\""));
    }

    mod stored {
        use super::*;
        use crate::llm::claude_gateway::account::ClaudeGatewaySignOutReason;
        use crate::llm::claude_gateway::tokens::StoredGatewayAccess;
        use crate::llm::claude_gateway::GrantedTokens;
        use crate::test_support::{setup_test_db, teardown_test_db};
        use std::sync::Arc;

        struct Fixture {
            pool: PgPool,
            db_name: String,
            bus: EventBus,
            gateway: MockGateway,
        }

        impl Fixture {
            async fn new() -> Self {
                let (pool, db_name) = setup_test_db().await;
                let (bus, _callback_rx) = EventBus::new(pool.clone());
                Self {
                    pool,
                    db_name,
                    bus,
                    gateway: MockGateway::start().await,
                }
            }

            async fn sign_in(&self) -> ClaudeGatewayAccount {
                let tokens = GrantedTokens {
                    access_token: "gw-token".into(),
                    refresh_token: Some("r".into()),
                    token_expiry: None,
                    email: None,
                };
                ClaudeGatewayAccountStore::sign_in(
                    &self.pool,
                    &self.bus,
                    &self.gateway.origin,
                    &tokens,
                    None,
                )
                .await
                .unwrap();
                self.account().await.unwrap()
            }

            async fn account(&self) -> Option<ClaudeGatewayAccount> {
                ClaudeGatewayAccountStore::get(&self.pool).await.unwrap()
            }

            async fn announced(&self) -> Vec<Value> {
                sqlx::query_scalar(
                    "SELECT payload->'data' FROM events
                     WHERE event_type = 'ClaudeGatewayManagedSettingsChanged'
                     ORDER BY sequence",
                )
                .fetch_all(&self.pool)
                .await
                .unwrap()
            }

            async fn done(self) {
                self.pool.close().await;
                teardown_test_db(&self.db_name).await;
            }
        }

        fn telemetry_policy() -> Value {
            json!({
                "availableModels": ["claude-opus-5-5"],
                "env": {
                    "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
                    "OTEL_LOGS_EXPORTER": "otlp",
                    "OTEL_EXPORTER_OTLP_HEADERS": "x-secret=collector-credential"
                }
            })
        }

        /// The policy lands on the account row, so a restart reads it back. A
        /// change announces once, carrying no `env` value.
        #[tokio::test]
        async fn a_fetched_policy_is_stored_and_announced_once() {
            let f = Fixture::new().await;
            let account = f.sign_in().await;
            f.gateway
                .serve_managed_settings(AxumStatus::OK, telemetry_policy(), Some("\"v1\""));
            refresh(&f.pool, &f.bus, &account, "gw-token").await;
            let stored = f.account().await.unwrap();
            assert_eq!(
                stored.managed_settings.as_deref(),
                Some(&telemetry_policy())
            );
            assert_eq!(stored.managed_settings_etag.as_deref(), Some("\"v1\""));
            assert!(stored.managed_settings_fetched_at.is_some());

            refresh(&f.pool, &f.bus, &stored, "gw-token").await;
            let announced = f.announced().await;
            assert_eq!(announced.len(), 1, "a 304 announces nothing");
            assert_eq!(announced[0]["telemetry"], true);
            assert_eq!(announced[0]["available_models"], json!(["claude-opus-5-5"]));
            assert!(!announced[0].to_string().contains("collector-credential"));
            f.done().await;
        }

        /// A failed fetch keeps the last stored policy.
        #[tokio::test]
        async fn a_failed_fetch_keeps_the_stored_policy() {
            let f = Fixture::new().await;
            let account = f.sign_in().await;
            f.gateway
                .serve_managed_settings(AxumStatus::OK, telemetry_policy(), None);
            refresh(&f.pool, &f.bus, &account, "gw-token").await;
            f.gateway
                .serve_managed_settings(AxumStatus::BAD_GATEWAY, json!({}), None);
            refresh(&f.pool, &f.bus, &account, "gw-token").await;
            assert_eq!(
                f.account().await.unwrap().managed_settings.as_deref(),
                Some(&telemetry_policy())
            );
            f.done().await;
        }

        /// The policy belongs to one sign-in. A new sign-in starts with none,
        /// and a fetch for an ended sign-in stores nothing.
        #[tokio::test]
        async fn the_policy_vanishes_with_the_sign_in() {
            let f = Fixture::new().await;
            let first = f.sign_in().await;
            f.gateway
                .serve_managed_settings(AxumStatus::OK, telemetry_policy(), None);
            refresh(&f.pool, &f.bus, &first, "gw-token").await;

            let second = f.sign_in().await;
            assert!(second.managed_settings.is_none());
            assert!(second.managed_settings_fetched_at.is_none());

            refresh(&f.pool, &f.bus, &first, "gw-token").await;
            assert!(f.account().await.unwrap().managed_settings.is_none());

            ClaudeGatewayAccountStore::sign_out(
                &f.pool,
                &f.bus,
                ClaudeGatewaySignOutReason::SignedOut,
                None,
            )
            .await
            .unwrap();
            refresh(&f.pool, &f.bus, &second, "gw-token").await;
            assert!(f.account().await.is_none());
            f.done().await;
        }

        /// A sign-in starts a fetch, and a fresh policy is not fetched again.
        #[tokio::test]
        async fn the_task_fetches_after_a_sign_in() {
            let f = Fixture::new().await;
            f.gateway
                .serve_managed_settings(AxumStatus::OK, telemetry_policy(), None);
            let (pool, bus) = (f.pool.clone(), f.bus.clone());
            let refresh_lock = Arc::default();
            let factory: GatewayAccessFactory = Arc::new(move |origin| {
                Arc::new(StoredGatewayAccess::new(
                    pool.clone(),
                    bus.clone(),
                    origin,
                    Arc::clone(&refresh_lock),
                ))
            });
            spawn_refresh_task(f.bus.subscribe(), f.pool.clone(), f.bus.clone(), factory);
            f.sign_in().await;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while f.announced().await.is_empty() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(f.announced().await.len(), 1);
            assert!(ClaudeGatewayAccountStore::managed_settings_due(&f.pool)
                .await
                .unwrap()
                .is_none());
            f.done().await;
        }
    }

    #[tokio::test]
    async fn a_refusal_is_an_error() {
        let gateway = MockGateway::start().await;
        gateway.serve_managed_settings(AxumStatus::UNAUTHORIZED, json!({}), None);
        let err = fetch(&gateway.origin, "gw-token", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("sign in"), "{err}");
    }
}
