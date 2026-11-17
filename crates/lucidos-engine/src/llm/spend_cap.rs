//! Structured billing refusals, distinct from rate limits and capacity errors.

use chrono::{DateTime, NaiveDateTime, Utc};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use std::sync::OnceLock;

use super::ProviderKind;

/// The call owner's hold policy, independent of provider transport code.
#[async_trait::async_trait]
pub(crate) trait SpendCapPolicy: Send + Sync {
    async fn check(
        &self,
        provider: Option<ProviderKind>,
        interactive: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn failed(&self, error: &(dyn std::error::Error + Send + Sync + 'static));
    async fn succeeded(&self, provider: Option<ProviderKind>);
}

/// Callers stop retries and hold background work on this refusal.
#[derive(Clone, Debug)]
pub struct SpendCapReached {
    pub provider: ProviderKind,
    pub resets_at: Option<DateTime<Utc>>,
    pub message: String,
}

impl std::fmt::Display for SpendCapReached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Spend cap reached for {}: {}",
            self.provider.as_str(),
            self.message
        )
    }
}

impl std::error::Error for SpendCapReached {}

impl SpendCapReached {
    /// Read the provider's error structure, before any retry or auth decision.
    pub fn from_http(
        provider: ProviderKind,
        status: u16,
        headers: &HeaderMap,
        body: &str,
    ) -> Option<Self> {
        Self::from_http_at(provider, status, headers, body, Utc::now())
    }

    fn from_http_at(
        provider: ProviderKind,
        status: u16,
        headers: &HeaderMap,
        body: &str,
        now: DateTime<Utc>,
    ) -> Option<Self> {
        let value: serde_json::Value =
            serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
        let error = &value["error"];
        if status != 402 && !billing_error(error) {
            return None;
        }
        let message = error["message"].as_str().unwrap_or(body).to_owned();
        let retry_at = headers
            .get(RETRY_AFTER)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| {
                if let Ok(seconds) = s.trim().parse::<i64>() {
                    return (seconds >= 0)
                        .then(|| chrono::Duration::try_seconds(seconds))
                        .flatten()
                        .and_then(|delta| now.checked_add_signed(delta));
                }
                DateTime::parse_from_rfc2822(s)
                    .ok()
                    .map(|date| date.with_timezone(&Utc))
            });
        Some(Self {
            provider,
            resets_at: retry_at.into_iter().chain(reset_clause(&message)).max(),
            message,
        })
    }
}

fn billing_error(error: &serde_json::Value) -> bool {
    error["type"] == "billing_error" || error["code"] == "insufficient_quota"
}

pub fn is_spend_cap_http(status: u16, body: &str) -> bool {
    status == 402
        || serde_json::from_str::<serde_json::Value>(body)
            .is_ok_and(|value| billing_error(&value["error"]))
}

/// A failed-turn notice; coding-agent notices never start engine holds.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpendCapNotice {
    pub provider: String,
    pub resets_at: Option<DateTime<Utc>>,
    pub message: String,
}

impl SpendCapNotice {
    pub fn from_error(error: &(dyn std::error::Error + Send + Sync + 'static)) -> Option<Self> {
        if let Some(cap) = error.downcast_ref::<SpendCapReached>() {
            return Some(Self {
                provider: cap.provider.as_str().into(),
                resets_at: cap.resets_at,
                message: cap.message.clone(),
            });
        }
        if error
            .downcast_ref::<super::chatgpt_plan::ChatGptPlanError>()
            .is_some_and(|error| {
                error.failure == super::chatgpt_plan::ChatGptPlanFailure::LimitReached
            })
        {
            return Some(Self {
                provider: ProviderKind::ChatGptPlan.as_str().into(),
                resets_at: None,
                message: error.to_string(),
            });
        }
        None
    }

    pub fn from_agent_text(agent: &str, message: &str) -> Option<Self> {
        is_spend_cap_text(message).then(|| Self {
            provider: agent.into(),
            resets_at: reset_clause(message),
            message: message.into(),
        })
    }
}

fn reset_clause(message: &str) -> Option<DateTime<Utc>> {
    static RESET: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = RESET.get_or_init(|| {
        regex::Regex::new(r"(?i)\bresets\s+(\d{4}-\d{2}-\d{2})\s+(\d{2}:\d{2}(?::\d{2})?)\s+UTC\b")
            .expect("constant reset clause")
    });
    let parts = pattern.captures(message)?;
    let date = format!("{} {}", &parts[1], &parts[2]);
    ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(&date, format).ok())
        .map(|date| date.and_utc())
}

/// String-only consumers also see formatted HTTP errors from coding agents.
/// The structured HTTP path above remains the authority for engine calls.
pub fn is_spend_cap_text(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("spend cap reached for ")
        || lower.contains("spend limit reached")
        || lower.contains("billing_error")
        || lower.contains("insufficient_quota")
        || lower.contains("\"billing_error\"")
        || lower.contains("\"insufficient_quota\"")
        || lower.contains("402 payment required")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn now() -> DateTime<Utc> {
        "2026-11-10T23:00:00Z".parse().unwrap()
    }
    fn refusal(status: u16, headers: &HeaderMap, body: &str) -> Option<SpendCapReached> {
        SpendCapReached::from_http_at(ProviderKind::ClaudeGateway, status, headers, body, now())
    }

    #[test]
    fn billing_refusal_is_not_capacity_or_transient() {
        let body = r#"{"type":"error","error":{"type":"billing_error","message":"spend limit reached (daily; resets 2026-11-11 00:00 UTC); raise your cap at https://spend.example.com/cap"}}"#;
        let cap = refusal(429, &HeaderMap::new(), body).unwrap();
        assert_eq!(cap.resets_at, Some("2026-11-11T00:00:00Z".parse().unwrap()));
        for text in [
            cap.to_string(),
            format!("Claude gateway API error (429 Too Many Requests): {body} (after 4 attempts)"),
        ] {
            assert!(!crate::llm::is_capacity_error(&text));
            assert!(!crate::llm::is_transient_error(&text));
            assert!(!crate::llm::is_retryable_error(&text));
        }
        assert!(!crate::llm::should_retry_http(429, body, 1));
    }

    #[test]
    fn structures_identify_spend_caps_at_any_status() {
        for (status, body) in [
            (
                400,
                r#"{"error":{"type":"billing_error","message":"spent"}}"#,
            ),
            (
                429,
                r#"{"error":{"code":"insufficient_quota","message":"spent"}}"#,
            ),
            (402, "Payment required"),
        ] {
            let cap = refusal(status, &HeaderMap::new(), body).unwrap();
            assert_eq!(cap.provider, ProviderKind::ClaudeGateway);
            assert!(cap.resets_at.is_none());
        }
        for (status, body) in [
            (429, r#"{"error":{"type":"rate_limit_error"}}"#),
            (529, "overloaded"),
            (429, r#"{"error":{"status":"RESOURCE_EXHAUSTED"}}"#),
            (403, "billing disabled"),
        ] {
            assert!(refusal(status, &HeaderMap::new(), body).is_none());
        }
    }

    #[test]
    fn reset_uses_later_of_clause_and_retry_after() {
        let body = r#"{"error":{"type":"billing_error","message":"resets 2026-11-11 00:00 UTC"}}"#;
        for (header, expected) in [
            ("30", "2026-11-11T00:00:00Z"),
            ("7200", "2026-11-11T01:00:00Z"),
            ("Wed, 11 Nov 2026 02:00:00 GMT", "2026-11-11T02:00:00Z"),
            ("bad", "2026-11-11T00:00:00Z"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(RETRY_AFTER, header.parse().unwrap());
            assert_eq!(
                refusal(429, &headers, body).unwrap().resets_at,
                Some(expected.parse().unwrap())
            );
        }
    }
}

/// Fallback only when the provider did not name a reset time.
pub const SPEND_CAP_HOLD_DEFAULT: chrono::Duration = chrono::Duration::hours(1);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SpendCapHold {
    pub provider: String,
    pub resets_at: Option<DateTime<Utc>>,
    pub held_until: DateTime<Utc>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpendCapHoldEndCause {
    Reset,
    Succeeded,
    Cleared,
}

#[derive(Clone, Debug)]
pub enum SpendCapHoldChange {
    Started(SpendCapHold),
    Ended {
        provider: ProviderKind,
        cause: SpendCapHoldEndCause,
    },
}

#[derive(Clone, Debug)]
struct HeldSpendCap {
    refusal: SpendCapReached,
    held_until: DateTime<Utc>,
}

impl HeldSpendCap {
    fn snapshot(&self) -> SpendCapHold {
        SpendCapHold {
            provider: self.refusal.provider.as_str().to_owned(),
            resets_at: self.refusal.resets_at,
            held_until: self.held_until,
            message: self.refusal.message.clone(),
        }
    }
}

/// One engine's provider holds. The owner serializes transitions with their
/// announcements so a success cannot announce an end before the start.
#[derive(Debug, Default)]
pub struct SpendCapHolds {
    held: std::collections::HashMap<ProviderKind, HeldSpendCap>,
}

impl SpendCapHolds {
    pub fn refusal(&self, provider: ProviderKind) -> Option<SpendCapReached> {
        self.held.get(&provider).map(|held| held.refusal.clone())
    }

    pub fn snapshots(&self) -> Vec<SpendCapHold> {
        let mut holds: Vec<_> = self.held.values().map(HeldSpendCap::snapshot).collect();
        holds.sort_by(|a, b| a.provider.cmp(&b.provider));
        holds
    }

    /// Called before a gate and status read. Expiry announces exactly once.
    pub fn expire(&mut self, now: DateTime<Utc>) -> Vec<SpendCapHoldChange> {
        let expired: Vec<_> = self
            .held
            .iter()
            .filter_map(|(provider, hold)| (hold.held_until <= now).then_some(*provider))
            .collect();
        expired
            .into_iter()
            .filter_map(|provider| self.end(provider, SpendCapHoldEndCause::Reset))
            .collect()
    }

    pub fn reached(
        &mut self,
        refusal: SpendCapReached,
        now: DateTime<Utc>,
    ) -> Option<SpendCapHoldChange> {
        use std::collections::hash_map::Entry;
        match self.held.entry(refusal.provider) {
            Entry::Occupied(_) => None,
            Entry::Vacant(entry) => {
                let held_until = refusal
                    .resets_at
                    .filter(|reset| *reset > now)
                    .unwrap_or(now + SPEND_CAP_HOLD_DEFAULT);
                let hold = HeldSpendCap {
                    refusal,
                    held_until,
                };
                let change = SpendCapHoldChange::Started(hold.snapshot());
                entry.insert(hold);
                Some(change)
            }
        }
    }

    pub fn end(
        &mut self,
        provider: ProviderKind,
        cause: SpendCapHoldEndCause,
    ) -> Option<SpendCapHoldChange> {
        self.held
            .remove(&provider)
            .map(|_| SpendCapHoldChange::Ended { provider, cause })
    }
}

#[cfg(test)]
mod hold_tests {
    use super::*;
    fn now() -> DateTime<Utc> {
        "2026-11-10T23:00:00Z".parse().unwrap()
    }
    fn cap(provider: ProviderKind, resets_at: Option<DateTime<Utc>>) -> SpendCapReached {
        SpendCapReached {
            provider,
            resets_at,
            message: "spent".into(),
        }
    }

    #[test]
    fn provider_holds_expire_at_the_named_reset_and_announce_once() {
        let mut holds = SpendCapHolds::default();
        let reset = now() + chrono::Duration::minutes(10);
        assert!(matches!(
            holds.reached(cap(ProviderKind::ClaudeGateway, Some(reset)), now()),
            Some(SpendCapHoldChange::Started(_))
        ));
        assert!(holds
            .reached(cap(ProviderKind::ClaudeGateway, Some(reset)), now())
            .is_none());
        assert!(holds.refusal(ProviderKind::OpenAi).is_none());
        assert!(holds
            .expire(reset - chrono::Duration::seconds(1))
            .is_empty());
        assert_eq!(holds.snapshots()[0].held_until, reset);
        assert!(matches!(
            holds.expire(reset).as_slice(),
            [SpendCapHoldChange::Ended {
                cause: SpendCapHoldEndCause::Reset,
                ..
            }]
        ));
        assert!(holds.expire(reset).is_empty());
    }

    #[test]
    fn a_stale_reset_keeps_the_refusal_held_without_immediate_retries() {
        for reset in [now(), now() - chrono::Duration::seconds(1)] {
            let mut holds = SpendCapHolds::default();
            holds.reached(cap(ProviderKind::OpenAi, Some(reset)), now());
            let snapshot = &holds.snapshots()[0];
            assert_eq!(snapshot.resets_at, Some(reset));
            assert_eq!(snapshot.held_until, now() + SPEND_CAP_HOLD_DEFAULT);
            assert!(holds.expire(now()).is_empty());
        }
    }

    #[test]
    fn unnamed_reset_holds_for_one_hour_and_success_or_clear_ends_it_once() {
        for cause in [
            SpendCapHoldEndCause::Succeeded,
            SpendCapHoldEndCause::Cleared,
        ] {
            let mut holds = SpendCapHolds::default();
            holds.reached(cap(ProviderKind::OpenAi, None), now());
            assert_eq!(
                holds.snapshots()[0].held_until,
                now() + chrono::Duration::hours(1)
            );
            assert!(holds.end(ProviderKind::OpenAi, cause).is_some());
            assert!(holds.end(ProviderKind::OpenAi, cause).is_none());
            assert!(holds.refusal(ProviderKind::OpenAi).is_none());
        }
        let mut holds = SpendCapHolds::default();
        holds.reached(cap(ProviderKind::OpenAi, None), now());
        assert!(holds
            .expire(now() + chrono::Duration::minutes(59))
            .is_empty());
        assert_eq!(holds.expire(now() + chrono::Duration::hours(1)).len(), 1);
    }
}
