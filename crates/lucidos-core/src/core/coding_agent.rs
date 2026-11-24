/// Identifier for the coding-agent backend. The engine maps each kind to a
/// concrete `AgentRuntime` implementation in its agent registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodingAgent {
    ClaudeCode,
    Codex,
}

impl CodingAgent {
    /// Every backend, in picker order. The LLM tool schemas read their enum
    /// from here.
    pub const ALL: [Self; 2] = [Self::ClaudeCode, Self::Codex];

    /// Kebab-case backend name shared by serde, thread summaries and HTTP.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    /// The name the user knows this backend by.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    /// Parse a stored backend string. Unknown / legacy NULL-ish values fall
    /// back to `ClaudeCode` — every thread persisted before the column
    /// existed was a Claude Code thread.
    pub fn parse(s: &str) -> Self {
        match s {
            "codex" => Self::Codex,
            _ => Self::ClaudeCode,
        }
    }
}
