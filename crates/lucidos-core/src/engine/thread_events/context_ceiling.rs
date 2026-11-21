use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Context size, in input tokens, at or under which a handoff never pays.
/// Estimates are noisy there, and a context this small still works well.
pub const HANDOFF_FLOOR_TOKENS: u64 = 100_000;

/// The current *agent session*'s context reading, the one definition of a
/// thread's context size. `None` fields are unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionReading {
    /// The latest `usage.input_tokens` of the session's own turns. A later
    /// `CodingAgentCompacted` replaces it with `post_tokens`, unknown when
    /// Claude Code did not report it. A fresh session has none yet.
    pub tokens: Option<u64>,
    /// The context window of the newest capture that records one.
    pub window: Option<u64>,
}

/// The share of the recorded window no ceiling goes above, in percent.
const WINDOW_CAP_PERCENT: u64 = 75;

/// A session's context reading past its effective ceiling, as
/// `ContextCeilingReached` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CeilingCrossing {
    pub context_tokens: u64,
    /// The effective ceiling, in tokens.
    pub ceiling: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
}

/// The largest size a ceiling names, in thousands of tokens.
const MAX_THOUSANDS: u64 = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextCeiling {
    /// No ceiling: the session grows until its backend compacts.
    Off,
    /// A size in tokens.
    Tokens(u64),
    /// A share of the session's recorded window, in percent.
    WindowShare(u8),
}

impl ContextCeiling {
    const OFF: &'static str = "off";

    /// Parse the text form: `off`, `<N>k`, or `<P>%`.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let refused = || {
            format!(
                "a context ceiling is \"off\", a size such as \"300k\", or a window share \
                 such as \"30%\" (got {raw:?})"
            )
        };
        if raw == Self::OFF {
            return Ok(Self::Off);
        }
        if let Some(share) = raw.strip_suffix('%') {
            return share
                .parse::<u8>()
                .ok()
                .filter(|p| (1..=100).contains(p))
                .map(Self::WindowShare)
                .ok_or_else(refused);
        }
        raw.strip_suffix('k')
            .and_then(|n| n.parse::<u64>().ok())
            .filter(|n| (1..=MAX_THOUSANDS).contains(n))
            .map(|n| Self::Tokens(n * 1_000))
            .ok_or_else(refused)
    }

    /// The ceiling in tokens for a session whose newest capture records
    /// `window`. `None` when it never fires: `off`, or a share with no window.
    pub fn effective(self, window: Option<u64>) -> Option<u64> {
        let window = window.filter(|w| *w > 0);
        let value = match self {
            Self::Off => return None,
            Self::Tokens(n) => n,
            Self::WindowShare(p) => window? * u64::from(p) / 100,
        };
        let capped = match window {
            Some(w) => value.min(w * WINDOW_CAP_PERCENT / 100),
            None => value,
        };
        Some(capped.max(HANDOFF_FLOOR_TOKENS))
    }

    /// The crossing `reading` makes of this ceiling, if it tops it. An unknown
    /// reading never does, and neither does a ceiling with no effective size.
    pub fn crossed_by(self, reading: SessionReading) -> Option<CeilingCrossing> {
        let context_tokens = reading.tokens?;
        let ceiling = self.effective(reading.window)?;
        (context_tokens > ceiling).then_some(CeilingCrossing {
            context_tokens,
            ceiling,
            window: reading.window,
        })
    }

    /// Whether `self` lets a session grow past where `from` stops it. A
    /// comparison the window cannot settle reads as a raise, since an unknown
    /// never authorizes.
    pub fn raises(self, from: Self, window: Option<u64>) -> bool {
        match (self, from) {
            (_, Self::Off) => false,
            (Self::Off, _) => true,
            (Self::Tokens(a), Self::Tokens(b)) => a > b,
            (Self::WindowShare(a), Self::WindowShare(b)) => a > b,
            _ => match (self.effective(window), from.effective(window)) {
                (Some(a), Some(b)) => a > b,
                _ => true,
            },
        }
    }
}

impl std::fmt::Display for ContextCeiling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => f.write_str(Self::OFF),
            Self::Tokens(n) => write!(f, "{}k", n / 1_000),
            Self::WindowShare(p) => write!(f, "{p}%"),
        }
    }
}

impl Serialize for ContextCeiling {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ContextCeiling {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Where a thread's `ContextCeilingSet` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CeilingSource {
    /// Set on the thread itself, by a person or an agent.
    Thread,
    /// Copied from the trigger that fired the thread.
    Trigger,
}
