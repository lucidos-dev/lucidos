/// Where a granted "Always allow" click is remembered.
///
///   * `Narrow` / `Broad`: a standing *tool grant* of this workspace, handed to
///     CC via `--allowedTools` on every spawn. Survives engine restart, but
///     only takes effect for tools/paths CC actually respects.
///   * `Session`: a *tool grant* the thread holds, durable across an engine
///     restart. Works for *every* tool and *every* file path (including CC's own
///     protected paths like `.claude/` and `.git/`), because the engine
///     intercepts before the prompt fires. A command that may write there is
///     the exception: only the bare `Bash` line covers it.
///
/// Wire form: `"narrow"` / `"broad"` / `"session"` (snake_case enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowScope {
    Narrow,
    Broad,
    Session,
}

/// The grant patterns a permission card offers, one per "Always allow"-style
/// button. A missing pattern means no such button. The engine stamps it on the
/// request event, the card renders exactly these buttons, and a click stores
/// exactly the stamped pattern (ADR 0571).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GrantOffer {
    /// The standing deny line the Remember menu can store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<String>,
    /// "Always allow": a persisted line, `Bash` or a bare tool name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broad: Option<String>,
    /// "Always allow `<head>` commands": a persisted line such as `Bash(git:*)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrow: Option<String>,
    /// "Allow for this thread": a pattern the thread holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Why the card offers no lasting grant at all, which it tells the owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withheld: Option<OfferWithheld>,
}

/// Why a card offers only Allow once and Deny.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OfferWithheld {
    /// The request reads a secret path, and no grant covers a secret read.
    SecretRead,
    /// Standing denies could not be checked for this command.
    StandingDenyUnavailable,
    /// Both the private-file warning and the unavailable check apply.
    StandingDenyUnavailableSecretRead,
}

impl OfferWithheld {
    pub fn standing_deny_unavailable(self) -> bool {
        matches!(
            self,
            Self::StandingDenyUnavailable | Self::StandingDenyUnavailableSecretRead
        )
    }
}

impl GrantOffer {
    /// An offer of no lasting grant, for `reason`.
    pub fn withheld(reason: OfferWithheld) -> Self {
        Self {
            withheld: Some(reason),
            ..Self::default()
        }
    }

    pub fn pattern(&self, scope: AllowScope) -> Option<&str> {
        match scope {
            AllowScope::Broad => self.broad.as_deref(),
            AllowScope::Narrow => self.narrow.as_deref(),
            AllowScope::Session => self.session.as_deref(),
        }
    }
}
