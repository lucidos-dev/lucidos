use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodingAgentKind {
    #[default]
    Lucidos,
    App,
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
