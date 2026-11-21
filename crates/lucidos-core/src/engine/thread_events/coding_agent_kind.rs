use serde::{Deserialize, Serialize};

/// The kind of coding-agent thread. Serialized on `SessionStarted` events and
/// stored in `thread_summaries.coding_agent_kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodingAgentKind {
    /// Edits Lucidos itself (the engine source tree). Default for legacy rows
    /// where no kind was recorded.
    #[default]
    Lucidos,
    /// Edits a single app folder at `data/apps/<id>/` in the user's workspace.
    App,
    /// Edits a user-registered external git repository.
    External,
}

impl CodingAgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lucidos => "lucidos",
            Self::App => "app",
            Self::External => "external",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "app" => Self::App,
            "external" => Self::External,
            _ => Self::Lucidos,
        }
    }
}
