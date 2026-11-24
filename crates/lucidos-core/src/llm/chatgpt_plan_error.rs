//! The typed failures of the plan route, read from OpenAI's `error.code` and
//! never from message text. The retry loop branches on them, and the limit
//! pause reads [`ChatGptPlanFailure::LimitReached`].

use std::fmt;

/// What a plan failure means for Lucidos. One variant per row of the plan's
/// error table (ADR 0636).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatGptPlanFailure {
    /// The weekly cap this app may spend is used up. Never retried in place.
    LimitReached,
    /// Plan usage is briefly unavailable. The retry loop backs off.
    Unavailable,
    /// OpenAI does not let this account spend its plan here.
    NotEligible,
    /// The request carried a field the plan route refuses: a Lucidos bug.
    UnsupportedCapability { param: Option<String> },
    /// OpenAI no longer accepts the signed-in user's token.
    InvalidUser,
    /// The route is not open to this account or region.
    NotConfigured,
}

impl ChatGptPlanFailure {
    /// The failure an OpenAI error code names, or `None` for a code that is
    /// not specific to the plan route.
    fn from_code(code: &str, param: Option<&str>) -> Option<Self> {
        Some(match code {
            "subscription_sharing_usage_limit_exceeded" => Self::LimitReached,
            "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => {
                Self::Unavailable
            }
            "subscription_sharing_user_not_eligible" => Self::NotEligible,
            "subscription_sharing_unsupported_capability" => Self::UnsupportedCapability {
                param: param.map(str::to_string),
            },
            "subscription_sharing_invalid_user" => Self::InvalidUser,
            "subscription_sharing_route_not_supported" => Self::NotConfigured,
            code if code.starts_with("chatpass_v2_") => Self::NotConfigured,
            _ => return None,
        })
    }

    /// Whether the retry loop may send the same request again after a backoff.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

/// A plan route failure, with what the log needs to trace it at OpenAI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptPlanError {
    pub failure: ChatGptPlanFailure,
    /// `None` for a failure that arrived mid-stream.
    pub status: Option<u16>,
    pub code: String,
    /// OpenAI's own message, quoted in the failure text.
    message: String,
    pub request_id: Option<String>,
}

impl ChatGptPlanError {
    /// Classify an HTTP error body. `None` when the body names no plan code,
    /// such as the `{"detail": "..."}` shape or a generic server error.
    pub fn from_http(status: u16, body: &str, request_id: Option<String>) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(body).ok()?;
        let mut error = Self::from_error_object(value.get("error")?)?;
        error.status = Some(status);
        error.request_id = request_id;
        Some(error)
    }

    /// Classify an `error` object: the HTTP body's, or a stream event's.
    pub fn from_error_object(error: &serde_json::Value) -> Option<Self> {
        let code = error.get("code")?.as_str()?;
        let param = error.get("param").and_then(|p| p.as_str());
        Some(Self {
            failure: ChatGptPlanFailure::from_code(code, param)?,
            status: None,
            code: code.to_string(),
            message: error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or_default()
                .to_string(),
            request_id: None,
        })
    }

    /// One log line naming the status, the code and the request id.
    pub fn log_line(&self) -> String {
        format!(
            "status={} code={} request_id={} message={}",
            self.status.map_or("stream".to_string(), |s| s.to_string()),
            self.code,
            self.request_id.as_deref().unwrap_or("none"),
            self.message,
        )
    }
}

impl fmt::Display for ChatGptPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let said = &self.message;
        match &self.failure {
            ChatGptPlanFailure::LimitReached => write!(
                f,
                "Your ChatGPT plan has no usage left for Lucidos. See ChatGPT Settings > Usage, \
                 or switch this model to another route or pick another model. OpenAI said: {said}"
            ),
            ChatGptPlanFailure::Unavailable => write!(
                f,
                "ChatGPT plan usage is not available at the moment. Try again shortly. \
                 OpenAI said: {said}"
            ),
            ChatGptPlanFailure::NotEligible => write!(
                f,
                "OpenAI does not let this account use its ChatGPT plan in Lucidos. \
                 OpenAI said: {said}"
            ),
            ChatGptPlanFailure::UnsupportedCapability { param } => write!(
                f,
                "The ChatGPT plan route refused part of the request ({}). This is a Lucidos \
                 bug. OpenAI said: {said}",
                param.as_deref().unwrap_or("no field named"),
            ),
            ChatGptPlanFailure::InvalidUser => write!(
                f,
                concat!(
                    "OpenAI no longer accepts the ChatGPT sign-in. Sign in again in ",
                    providers_settings!(),
                    ". OpenAI said: {said}"
                ),
                said = said
            ),
            ChatGptPlanFailure::NotConfigured => write!(
                f,
                "The ChatGPT plan route is not open to this account. OpenAI said: {said}"
            ),
        }
    }
}

impl std::error::Error for ChatGptPlanError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each documented code maps to its row, read from the `error` body shape
    /// the probes recorded. These pin wire shapes, so the literals are the
    /// contract.
    #[test]
    fn each_documented_code_classifies() {
        let cases = [
            (
                429,
                "subscription_sharing_usage_limit_exceeded",
                ChatGptPlanFailure::LimitReached,
            ),
            (
                503,
                "subscription_sharing_usage_unavailable",
                ChatGptPlanFailure::Unavailable,
            ),
            (
                503,
                "subscription_sharing_user_unavailable",
                ChatGptPlanFailure::Unavailable,
            ),
            (
                403,
                "subscription_sharing_user_not_eligible",
                ChatGptPlanFailure::NotEligible,
            ),
            (
                400,
                "subscription_sharing_unsupported_capability",
                ChatGptPlanFailure::UnsupportedCapability {
                    param: Some("temperature".to_string()),
                },
            ),
            (
                401,
                "subscription_sharing_invalid_user",
                ChatGptPlanFailure::InvalidUser,
            ),
            (
                403,
                "subscription_sharing_route_not_supported",
                ChatGptPlanFailure::NotConfigured,
            ),
            (
                403,
                "chatpass_v2_region_blocked",
                ChatGptPlanFailure::NotConfigured,
            ),
        ];
        for (status, code, failure) in cases {
            let body = format!(
                r#"{{"error":{{"message":"m","type":"invalid_request_error","param":"temperature","code":"{code}"}}}}"#
            );
            let error = ChatGptPlanError::from_http(status, &body, Some("req_1".to_string()))
                .unwrap_or_else(|| panic!("{code} must classify"));
            assert_eq!(error.failure, failure, "{code}");
            assert_eq!(error.status, Some(status));
            assert_eq!(error.request_id.as_deref(), Some("req_1"));
        }
    }

    /// The `detail` shape and a generic code are not plan failures: the
    /// ordinary error path handles them.
    #[test]
    fn a_body_with_no_plan_code_is_not_a_plan_failure() {
        for body in [
            r#"{"detail": "Unsupported service_tier: flex"}"#,
            r#"{"error":{"message":"m","type":"server_error","param":null,"code":"server_error"}}"#,
            "not json",
        ] {
            assert!(
                ChatGptPlanError::from_http(500, body, None).is_none(),
                "{body}"
            );
        }
    }

    /// Only a passing unavailability is retried. A limit message carries no
    /// wording the string-based retry checks read as transient.
    #[test]
    fn only_unavailability_is_retryable() {
        assert!(ChatGptPlanFailure::Unavailable.is_retryable());
        let limit = ChatGptPlanError::from_error_object(&serde_json::json!({
            "code": "subscription_sharing_usage_limit_exceeded",
            "message": "You have hit your usage limit.",
        }))
        .unwrap();
        assert!(!limit.failure.is_retryable());
        assert!(!crate::llm::is_retryable_error(&limit.to_string()));
        assert!(limit.to_string().contains("Settings > Usage"));
    }
}
