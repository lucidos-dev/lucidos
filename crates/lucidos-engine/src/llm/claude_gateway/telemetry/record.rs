//! One `claude_code.api_request` or `claude_code.api_error` record, built from
//! a persisted model call.
//!
//! Its attributes follow Claude Code's monitoring contract, so a gateway reads
//! a Lucidos record as it reads one from Claude Code. A record carries counts,
//! cost, timing and ids only: never a prompt, a reply, a tool input, a path or
//! a provider's error text.
//!
//! A model with no price card reports no `cost_usd` at all. An unknown price
//! is not a free call, so a record never claims $0 for it.

use chrono::{DateTime, SecondsFormat, Utc};
use uuid::Uuid;

use crate::engine::{ApiUsage, ContextPurpose};
use crate::llm::call_trace::CallFailure;
use crate::llm::claude_gateway::gateway_managed_settings::GatewayManagedSettings;
use crate::llm::claude_gateway::token_endpoint::jwt_claims;

/// The `event.name` of a call that answered. Its body is this after the
/// `claude_code.` prefix.
const API_REQUEST: &str = "api_request";

/// The `event.name` of a call that failed.
const API_ERROR: &str = "api_error";

/// How Lucidos names itself as the client.
const CLIENT_NAME: &str = "lucidos";

/// `identity.source` for a user named by the gateway sign-in's JWT, as Claude
/// Code reports it.
const IDENTITY_SOURCE: &str = "gateway-oidc";

/// `speed` of every call. Lucidos never asks for fast mode.
const SPEED: &str = "normal";

/// The `effort` values Claude Code's contract accepts. A call at another tier,
/// such as `none`, reports no effort.
const REPORTED_EFFORTS: &[&str] = crate::core::preference_catalog::CODING_AGENT_EFFORTS;

/// What one persisted gateway call says about itself.
pub(crate) struct CallRow {
    /// The row's own event sequence. Paired with `session.id` and the event
    /// name, a collector can use it to drop a record sent twice.
    pub sequence: i64,
    pub thread_id: Option<Uuid>,
    pub created: DateTime<Utc>,
    /// The model Lucidos asked for.
    pub model: String,
    pub duration_ms: Option<u64>,
    pub purpose: ContextPurpose,
    /// The reasoning tier the call went out at, when it had one.
    pub reasoning_effort: Option<String>,
    /// The turn the call belongs to, when one is known.
    pub prompt_id: Option<Uuid>,
    /// The provider's id for the response, when it had one.
    pub request_id: Option<String>,
    pub outcome: Outcome,
}

/// How the call ended, with the facts only that ending has.
pub(crate) enum Outcome {
    Completed {
        usage: ApiUsage,
        /// Time to the first streamed token, when one arrived.
        ttft_ms: Option<u64>,
    },
    Failed {
        failure: CallFailure,
        /// The last attempt's HTTP status, when a response arrived.
        status_code: Option<u16>,
        attempts: u32,
    },
}

/// Who signed in, from the access token's claims. `None` inside when the token
/// is not a JWT.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Identity(Option<Claims>);

#[derive(Debug, PartialEq)]
struct Claims {
    email: Option<String>,
    id: Option<String>,
    /// The `groups` claim, comma-joined. `None` when empty.
    groups: Option<String>,
}

impl Identity {
    pub(crate) fn of_bearer(bearer: &str) -> Self {
        let Some(claims) = jwt_claims(bearer) else {
            return Self::default();
        };
        let claim = |name: &str| claims[name].as_str().map(str::to_string);
        // Some IdPs send a lone group as a bare string rather than an array.
        let groups: Vec<&str> = match &claims["groups"] {
            serde_json::Value::String(group) => vec![group.as_str()],
            other => other
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|group| group.as_str())
                .collect(),
        };
        Self(Some(Claims {
            email: claim("email"),
            id: claim("sub"),
            groups: (!groups.is_empty()).then(|| groups.join(",")),
        }))
    }

    /// The identity attributes, in the order Claude Code sends them.
    fn attributes(&self) -> Vec<(&'static str, String)> {
        let Some(claims) = &self.0 else {
            return Vec::new();
        };
        let mut attributes = vec![("identity.source", IDENTITY_SOURCE.to_string())];
        attributes.extend(claims.id.clone().map(|id| ("user.id", id)));
        attributes.extend(claims.email.clone().map(|email| ("user.email", email)));
        attributes.extend(claims.groups.clone().map(|groups| ("user.groups", groups)));
        attributes
    }
}

/// One log record, ready to encode.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LogRecord {
    /// The `event.name`, without the `claude_code.` prefix.
    pub event_name: &'static str,
    pub time_unix_nano: u64,
    /// Every attribute is a string, as Claude Code sends them.
    pub attributes: Vec<(&'static str, String)>,
}

impl LogRecord {
    /// The record body: `claude_code.` and the event name.
    pub(crate) fn body(&self) -> String {
        format!("claude_code.{}", self.event_name)
    }
}

/// The record for `row`. `policy` maps the model to the name telemetry
/// reports it under.
pub(crate) fn log_record(
    row: &CallRow,
    policy: Option<&GatewayManagedSettings>,
    identity: &Identity,
) -> LogRecord {
    let model = match policy {
        Some(policy) => policy.reported_model(&row.model),
        None => GatewayManagedSettings::default().reported_model(&row.model),
    };
    let (event_name, outcome_attributes) = match &row.outcome {
        Outcome::Completed { usage, ttft_ms } => (
            API_REQUEST,
            completed_attributes(row, &model, usage, *ttft_ms),
        ),
        Outcome::Failed {
            failure,
            status_code,
            attempts,
        } => (
            API_ERROR,
            failed_attributes(row, &model, *failure, *status_code, *attempts),
        ),
    };
    let mut attributes = identity.attributes();
    attributes.extend([
        ("event.name", event_name.to_string()),
        (
            "event.timestamp",
            row.created.to_rfc3339_opts(SecondsFormat::Millis, true),
        ),
        ("event.sequence", row.sequence.to_string()),
    ]);
    attributes.extend(row.prompt_id.map(|id| ("prompt.id", id.to_string())));
    attributes.extend(row.thread_id.map(|id| ("session.id", id.to_string())));
    attributes.extend([
        ("terminal.type", CLIENT_NAME.to_string()),
        ("client.name", CLIENT_NAME.to_string()),
    ]);
    attributes.extend(outcome_attributes);
    attributes.extend([
        ("speed", SPEED.to_string()),
        ("query_source", query_source(row.purpose)),
    ]);
    if let Some(effort) = row
        .reasoning_effort
        .as_deref()
        .filter(|effort| REPORTED_EFFORTS.contains(effort))
    {
        attributes.push(("effort", effort.to_string()));
    }
    attributes.extend(row.request_id.clone().map(|id| ("request_id", id)));
    LogRecord {
        event_name,
        time_unix_nano: unix_nanos(row.created),
        attributes,
    }
}

/// The `api_request` attributes that come before `speed`.
fn completed_attributes(
    row: &CallRow,
    model: &str,
    usage: &ApiUsage,
    ttft_ms: Option<u64>,
) -> Vec<(&'static str, String)> {
    let fresh_input = usage
        .input_tokens
        .saturating_sub(usage.cache_read_tokens)
        .saturating_sub(usage.cache_creation_tokens);
    let day = row.created.with_timezone(&chrono::Local).date_naive();
    let cost = usage.usd_on(&row.model, day);
    let cache_write_1h = usage
        .cache_creation_1h_tokens
        .unwrap_or(0)
        .min(usage.cache_creation_tokens);
    let mut attributes = vec![
        ("model", model.to_string()),
        ("input_tokens", fresh_input.to_string()),
        ("output_tokens", usage.output_tokens.to_string()),
        ("cache_read_tokens", usage.cache_read_tokens.to_string()),
        (
            "cache_creation_tokens",
            usage.cache_creation_tokens.to_string(),
        ),
        (
            "cache_creation_5m_tokens",
            (usage.cache_creation_tokens - cache_write_1h).to_string(),
        ),
        ("cache_creation_1h_tokens", cache_write_1h.to_string()),
    ];
    if let Some(cost) = cost {
        attributes.extend([
            ("cost_usd", cost.to_string()),
            ("cost_usd_micros", (cost * 1_000_000.0).round().to_string()),
        ]);
    }
    attributes.push(("duration_ms", row.duration_ms.unwrap_or(0).to_string()));
    attributes.extend(ttft_ms.map(|ms| ("ttft_ms", ms.to_string())));
    attributes
}

/// The `api_error` attributes that come before `speed`. `attempt` is the
/// attempt the call failed on, as Claude Code counts it: from 1, so a call
/// that failed before any send reports attempt 1.
fn failed_attributes(
    row: &CallRow,
    model: &str,
    failure: CallFailure,
    status_code: Option<u16>,
    attempts: u32,
) -> Vec<(&'static str, String)> {
    let mut attributes = vec![
        ("model", model.to_string()),
        ("error", error_class(failure, status_code).to_string()),
    ];
    attributes.extend(status_code.map(|status| ("status_code", status.to_string())));
    attributes.extend([
        ("duration_ms", row.duration_ms.unwrap_or(0).to_string()),
        ("attempt", attempts.max(1).to_string()),
    ]);
    attributes
}

/// Claude Code's error class slug for a failure, the value of `error` in its
/// class mode. Never the provider's message: that can echo request content.
/// Every slug matches `^[a-z0-9_]+$`, which that mode requires.
fn error_class(failure: CallFailure, status_code: Option<u16>) -> &'static str {
    match (failure, status_code) {
        (CallFailure::Http, Some(429)) => "rate_limit",
        (CallFailure::Http, Some(529)) => "server_overload",
        (CallFailure::Http, Some(401 | 403)) => "auth_error",
        (CallFailure::Http, Some(404)) => "model_not_found",
        (CallFailure::Http, Some(413)) => "request_too_large",
        (CallFailure::Connection | CallFailure::Stream, _) => "connection_error",
        (CallFailure::Timeout, _) => "api_timeout",
        (CallFailure::Http | CallFailure::Other, _) => "unknown",
    }
}

/// Which kind of call it was: `chat` for an agent's turn, `compaction` for
/// history summarisation, else the auxiliary purpose's own name.
fn query_source(purpose: ContextPurpose) -> String {
    match purpose {
        ContextPurpose::Turn => "chat".to_string(),
        ContextPurpose::ConversationSummary => "compaction".to_string(),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|name| name.as_str().map(str::to_string))
            .unwrap_or_else(|| "auxiliary".to_string()),
    }
}

pub(crate) fn unix_nanos(at: DateTime<Utc>) -> u64 {
    at.timestamp_nanos_opt()
        .and_then(|nanos| u64::try_from(nanos).ok())
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::llm::claude_gateway::token_endpoint::tests::id_token;
    use crate::llm::model_prices::price_table;
    use serde_json::json;

    pub(crate) fn row(sequence: i64) -> CallRow {
        CallRow {
            sequence,
            thread_id: Some(Uuid::nil()),
            created: DateTime::parse_from_rfc3339("2026-01-02T03:04:05.678Z")
                .unwrap()
                .with_timezone(&Utc),
            model: "claude-sonnet-5-5[1m]".into(),
            duration_ms: Some(1_234),
            purpose: ContextPurpose::Turn,
            reasoning_effort: Some("high".into()),
            prompt_id: None,
            request_id: None,
            outcome: Outcome::Completed {
                usage: ApiUsage {
                    input_tokens: 1_000,
                    output_tokens: 50,
                    cache_read_tokens: 600,
                    cache_creation_tokens: 100,
                    modality: None,
                    service_tier: None,
                    cache_creation_1h_tokens: Some(40),
                },
                ttft_ms: None,
            },
        }
    }

    fn failed_row(failure: CallFailure, status_code: Option<u16>) -> CallRow {
        CallRow {
            outcome: Outcome::Failed {
                failure,
                status_code,
                attempts: 3,
            },
            ..row(7)
        }
    }

    fn attribute<'a>(record: &'a LogRecord, key: &str) -> Option<&'a str> {
        record
            .attributes
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn a_turn_reports_fresh_input_cost_and_timing() {
        let record = log_record(&row(42), None, &Identity::default());
        let get = |key| attribute(&record, key);
        assert_eq!(get("event.name"), Some("api_request"));
        assert_eq!(get("event.timestamp"), Some("2026-01-02T03:04:05.678Z"));
        assert_eq!(get("event.sequence"), Some("42"));
        assert_eq!(get("session.id"), Some(Uuid::nil().to_string().as_str()));
        assert_eq!(get("client.name"), Some("lucidos"));
        assert_eq!(get("terminal.type"), Some("lucidos"));
        assert_eq!(get("model"), Some("claude-sonnet-5-5"));
        assert_eq!(
            get("input_tokens"),
            Some("300"),
            "the cache counts are parts"
        );
        assert_eq!(get("output_tokens"), Some("50"));
        assert_eq!(get("cache_read_tokens"), Some("600"));
        assert_eq!(get("cache_creation_tokens"), Some("100"));
        assert_eq!(get("cache_creation_5m_tokens"), Some("60"));
        assert_eq!(get("cache_creation_1h_tokens"), Some("40"));
        assert_eq!(get("duration_ms"), Some("1234"));
        assert_eq!(get("speed"), Some("normal"));
        assert_eq!(get("query_source"), Some("chat"));
        assert_eq!(get("effort"), Some("high"));
        assert_eq!(get("identity.source"), None, "no JWT, no identity");
        assert_eq!(get("user.email"), None);
        let cost: f64 = get("cost_usd").unwrap().parse().unwrap();
        assert!(cost > 0.0);
        assert_eq!(
            get("cost_usd_micros"),
            Some((cost * 1e6).round().to_string().as_str())
        );
        assert_eq!(record.time_unix_nano, unix_nanos(row(42).created));
        assert_eq!(record.body(), "claude_code.api_request");
    }

    /// Cached tokens are parts of the input total, each at its own share.
    #[test]
    fn a_call_prices_each_input_class_once() {
        let usage = ApiUsage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_read_tokens: 500_000,
            cache_creation_tokens: 200_000,
            modality: None,
            service_tier: None,
            cache_creation_1h_tokens: Some(100_000),
        };
        // Fresh 300k, read 500k, writes 100k at 5 minutes and 100k at 1 hour,
        // then 100k out.
        let today = crate::llm::model_prices::today();
        let sonnet = price_table()
            .price_on("claude-sonnet-5-5", today)
            .expect("priced");
        let input = 300_000.0
            + 500_000.0 * sonnet.cache.read
            + 100_000.0 * sonnet.cache.write_5m
            + 100_000.0 * sonnet.cache.write_1h;
        let expected = (input * sonnet.input + 100_000.0 * sonnet.output) / 1_000_000.0;
        let cost = usage.usd_on("claude-sonnet-5-5[1m]", today).unwrap();
        assert!((cost - expected).abs() < 1e-9, "{cost} vs {expected}");
        assert_eq!(usage.usd_on("my-local-model", today), None);
    }

    /// An unknown price is not a free call, so the record leaves cost out.
    #[test]
    fn a_model_without_a_price_reports_no_cost() {
        let mut unpriced = row(1);
        unpriced.model = "my-local-model".into();
        let record = log_record(&unpriced, None, &Identity::default());
        assert_eq!(attribute(&record, "cost_usd"), None);
        assert_eq!(attribute(&record, "cost_usd_micros"), None);
        assert_eq!(attribute(&record, "output_tokens"), Some("50"));
    }

    #[test]
    fn the_purpose_names_the_query_source() {
        let source = |purpose| {
            let mut capture = row(1);
            capture.purpose = purpose;
            attribute(
                &log_record(&capture, None, &Identity::default()),
                "query_source",
            )
            .unwrap()
            .to_string()
        };
        assert_eq!(source(ContextPurpose::ConversationSummary), "compaction");
        assert_eq!(source(ContextPurpose::Title), "title");
        assert_eq!(
            source(ContextPurpose::SummaryCompaction),
            "summary_compaction"
        );
    }

    #[test]
    fn the_policy_alias_names_the_model() {
        let policy = GatewayManagedSettings::from_document(
            &json!({"availableModels": ["claude-sonnet-5-5@20261001"]}),
        );
        let record = log_record(&row(1), Some(&policy), &Identity::default());
        assert_eq!(
            attribute(&record, "model"),
            Some("claude-sonnet-5-5-20261001")
        );
    }

    /// Only Claude Code's own tiers are reported, and a call without one
    /// reports none.
    #[test]
    fn effort_is_reported_only_at_a_contract_tier() {
        let effort = |tier: Option<&str>| {
            let mut capture = row(1);
            capture.reasoning_effort = tier.map(str::to_string);
            attribute(&log_record(&capture, None, &Identity::default()), "effort")
                .map(str::to_string)
        };
        assert_eq!(effort(Some("xhigh")).as_deref(), Some("xhigh"));
        assert_eq!(effort(Some("none")), None);
        assert_eq!(effort(None), None);
    }

    #[test]
    fn a_jwt_bearer_names_the_user_and_its_source() {
        let bearer = id_token(json!({
            "email": "user@example.com",
            "sub": "u-1",
            "groups": ["engineering", "admins"],
        }));
        let identity = Identity::of_bearer(&bearer);
        let record = log_record(&row(1), None, &identity);
        let identity_attributes: Vec<(&str, &str)> = record.attributes[..4]
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();
        assert_eq!(
            identity_attributes,
            vec![
                ("identity.source", "gateway-oidc"),
                ("user.id", "u-1"),
                ("user.email", "user@example.com"),
                ("user.groups", "engineering,admins"),
            ]
        );
        assert_eq!(Identity::of_bearer("opaque-token"), Identity::default());
    }

    #[test]
    fn a_jwt_without_claims_still_names_its_source() {
        let identity = Identity::of_bearer(&id_token(json!({})));
        let record = log_record(&row(1), None, &identity);
        assert_eq!(attribute(&record, "identity.source"), Some("gateway-oidc"));
        assert_eq!(attribute(&record, "user.groups"), None);
    }

    #[test]
    fn a_lone_group_claim_still_names_the_group() {
        let identity = Identity::of_bearer(&id_token(json!({ "groups": "admins" })));
        let record = log_record(&row(1), None, &identity);
        assert_eq!(attribute(&record, "user.groups"), Some("admins"));
    }

    const FAILURES: [CallFailure; 5] = [
        CallFailure::Http,
        CallFailure::Connection,
        CallFailure::Timeout,
        CallFailure::Stream,
        CallFailure::Other,
    ];

    fn error_slug(failure: CallFailure, status: Option<u16>) -> String {
        let record = log_record(&failed_row(failure, status), None, &Identity::default());
        attribute(&record, "error").unwrap().to_string()
    }

    #[test]
    fn a_call_that_failed_before_any_send_reports_attempt_one() {
        let mut failed = failed_row(CallFailure::Other, None);
        failed.outcome = Outcome::Failed {
            failure: CallFailure::Other,
            status_code: None,
            attempts: 0,
        };
        let record = log_record(&failed, None, &Identity::default());
        assert_eq!(attribute(&record, "attempt"), Some("1"));
    }

    /// The attribute names and values below are the contract of Claude Code
    /// 2.1.300, read from its binary. They are literal on purpose: a rename
    /// here is a change a collector would notice.
    #[test]
    fn a_failed_call_is_an_api_error_with_its_class() {
        let mut failed = failed_row(CallFailure::Http, Some(429));
        failed.request_id = Some("req_01".into());
        let record = log_record(&failed, None, &Identity::default());
        let get = |key| attribute(&record, key);
        assert_eq!(record.body(), "claude_code.api_error");
        assert_eq!(get("event.name"), Some("api_error"));
        assert_eq!(get("event.sequence"), Some("7"));
        assert_eq!(get("model"), Some("claude-sonnet-5-5"));
        assert_eq!(get("error"), Some("rate_limit"));
        assert_eq!(get("status_code"), Some("429"));
        assert_eq!(get("duration_ms"), Some("1234"));
        assert_eq!(get("attempt"), Some("3"));
        assert_eq!(get("request_id"), Some("req_01"));
        assert_eq!(get("speed"), Some("normal"));
        assert_eq!(get("query_source"), Some("chat"));
        assert_eq!(get("effort"), Some("high"));
        assert_eq!(get("session.id"), Some(Uuid::nil().to_string().as_str()));
        for usage_only in ["input_tokens", "cost_usd", "ttft_ms", "cache_read_tokens"] {
            assert_eq!(get(usage_only), None, "a failed call has no {usage_only}");
        }
    }

    #[test]
    fn a_status_names_its_error_class() {
        let class = |status| error_slug(CallFailure::Http, Some(status));
        assert_eq!(class(429), "rate_limit");
        assert_eq!(class(529), "server_overload");
        assert_eq!(class(401), "auth_error");
        assert_eq!(class(403), "auth_error");
        assert_eq!(class(404), "model_not_found");
        assert_eq!(class(413), "request_too_large");
        assert_eq!(class(500), "unknown");
        assert_eq!(error_slug(CallFailure::Http, None), "unknown");
    }

    #[test]
    fn a_transport_failure_names_its_class_whatever_the_status() {
        assert_eq!(
            error_slug(CallFailure::Connection, None),
            "connection_error"
        );
        assert_eq!(
            error_slug(CallFailure::Stream, Some(200)),
            "connection_error"
        );
        assert_eq!(error_slug(CallFailure::Timeout, None), "api_timeout");
        assert_eq!(error_slug(CallFailure::Other, None), "unknown");
    }

    /// Claude Code's class mode drops an `error` that is not a bare slug, and
    /// a message here could echo request content.
    #[test]
    fn every_error_class_is_a_slug() {
        for failure in FAILURES {
            for status in [
                None,
                Some(0),
                Some(200),
                Some(401),
                Some(429),
                Some(500),
                Some(529),
            ] {
                let slug = error_slug(failure, status);
                assert!(
                    !slug.is_empty()
                        && slug
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                    "{failure:?} {status:?} gave {slug:?}"
                );
            }
        }
    }

    #[test]
    fn an_error_without_a_status_or_request_id_omits_both() {
        let record = log_record(
            &failed_row(CallFailure::Connection, None),
            None,
            &Identity::default(),
        );
        assert_eq!(attribute(&record, "status_code"), None);
        assert_eq!(attribute(&record, "request_id"), None);
    }

    #[test]
    fn a_request_reports_its_ttft_request_id_and_prompt() {
        let prompt = Uuid::from_u128(0xabc);
        let mut call = row(1);
        call.prompt_id = Some(prompt);
        call.request_id = Some("req_02".into());
        if let Outcome::Completed { ttft_ms, .. } = &mut call.outcome {
            *ttft_ms = Some(321);
        }
        let record = log_record(&call, None, &Identity::default());
        assert_eq!(attribute(&record, "ttft_ms"), Some("321"));
        assert_eq!(attribute(&record, "request_id"), Some("req_02"));
        assert_eq!(
            attribute(&record, "prompt.id"),
            Some(prompt.to_string().as_str())
        );
    }

    /// Absent stays absent: never an empty string or a zero.
    #[test]
    fn what_a_call_lacks_is_not_reported() {
        let record = log_record(&row(1), None, &Identity::default());
        for absent in ["ttft_ms", "request_id", "prompt.id"] {
            assert_eq!(attribute(&record, absent), None, "{absent}");
        }
    }

    /// `prompt.id` follows `event.sequence`, where Claude Code puts it.
    #[test]
    fn the_prompt_id_follows_the_event_sequence() {
        for call in [row(1), failed_row(CallFailure::Timeout, None)] {
            let call = CallRow {
                prompt_id: Some(Uuid::nil()),
                ..call
            };
            let record = log_record(&call, None, &Identity::default());
            let names: Vec<&str> = record.attributes.iter().map(|(k, _)| *k).collect();
            let sequence = names.iter().position(|k| *k == "event.sequence").unwrap();
            assert_eq!(names[sequence + 1], "prompt.id", "{names:?}");
        }
    }
}
