use serde::{Deserialize, Serialize};

/// The purpose of a plan, supplied explicitly when planning starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanKind {
    Design,
    #[default]
    Implementation,
}

impl PlanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Design => "design",
            Self::Implementation => "implementation",
        }
    }
}
