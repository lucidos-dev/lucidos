//! Reasons carried by provider sign-out events.

/// Why a Claude gateway sign-in ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudeGatewaySignOutReason {
    /// Someone signed out, from Settings, the CLI or the API.
    SignedOut,
    /// `claude_gateway_url` stopped naming the gateway that issued the tokens.
    UrlChanged,
    /// The gateway rejected a token nothing can renew, so the user must sign
    /// in again.
    Rejected,
}

impl ClaudeGatewaySignOutReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SignedOut => "signed-out",
            Self::UrlChanged => "url-changed",
            Self::Rejected => "rejected",
        }
    }
}

/// Why a ChatGPT plan sign-in ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChatGptPlanSignOutReason {
    /// Someone signed out, from Settings, the CLI or the API.
    SignedOut,
    /// OpenAI refused the refresh token for good, so the user must sign in
    /// again.
    Rejected,
    /// Someone forgot the account and its registration, so the next sign-in
    /// may use any ChatGPT account.
    Forgotten,
}

impl ChatGptPlanSignOutReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SignedOut => "signed-out",
            Self::Rejected => "rejected",
            Self::Forgotten => "forgotten",
        }
    }
}
