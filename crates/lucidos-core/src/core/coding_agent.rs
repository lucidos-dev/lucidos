#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodingAgent {
    ClaudeCode,
    Codex,
}

impl CodingAgent {
    pub const ALL: [Self; 2] = [Self::ClaudeCode, Self::Codex];

    /// INVARIANT: Kebab-case backend name shared by serde, thread summaries and HTTP.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "codex" => Self::Codex,
            _ => Self::ClaudeCode,
        }
    }
}
