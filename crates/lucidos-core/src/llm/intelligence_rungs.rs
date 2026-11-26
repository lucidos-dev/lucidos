use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::core::coding_agent::CodingAgent;
use crate::core::preference_catalog as prefs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LadderSurface {
    Chat,
    ClaudeCode,
    Codex,
}

impl LadderSurface {
    pub const ALL: [Self; 3] = [Self::Chat, Self::ClaudeCode, Self::Codex];

    pub fn coding_agent(self) -> Option<CodingAgent> {
        match self {
            Self::Chat => None,
            Self::ClaudeCode => Some(CodingAgent::ClaudeCode),
            Self::Codex => Some(CodingAgent::Codex),
        }
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::ClaudeCode => CodingAgent::ClaudeCode.as_str(),
            Self::Codex => CodingAgent::Codex.as_str(),
        }
    }

    pub fn display_name(self) -> &'static str {
        self.coding_agent()
            .map_or("Lucidos Agent", CodingAgent::display_name)
    }

    pub fn efforts(self) -> &'static [&'static str] {
        match self {
            Self::Chat => prefs::REASONING_EFFORTS,
            Self::ClaudeCode | Self::Codex => prefs::CODING_AGENT_EFFORTS,
        }
    }

    pub fn override_pref(self) -> &'static prefs::Pref<prefs::Optional> {
        match self {
            Self::Chat => &prefs::INTELLIGENCE_LADDER_CHAT,
            Self::ClaudeCode => &prefs::INTELLIGENCE_LADDER_CLAUDE_CODE,
            Self::Codex => &prefs::INTELLIGENCE_LADDER_CODEX,
        }
    }

    pub fn shipped_pairs(self) -> &'static [SurfacePair] {
        let surfaces = &rung_estimates().surfaces;
        match self {
            Self::Chat => &surfaces.chat,
            Self::ClaudeCode => &surfaces.claude_code,
            Self::Codex => &surfaces.codex,
        }
    }

    pub fn profile_band(self, pair: &EstimatedPair) -> i32 {
        match self {
            Self::Chat => pair.general_band,
            Self::ClaudeCode | Self::Codex => pair.coding_band,
        }
    }
}

impl From<CodingAgent> for LadderSurface {
    fn from(agent: CodingAgent) -> Self {
        match agent {
            CodingAgent::ClaudeCode => Self::ClaudeCode,
            CodingAgent::Codex => Self::Codex,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EstimatedPair {
    pub model: String,
    pub effort: String,
    pub general_band: i32,
    pub coding_band: i32,
    pub coding_estimated: bool,
    pub token_band: i32,
    pub time_band: i32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfacePair {
    pub model: String,
    pub effort: String,
    pub on: bool,
    pub pinned: bool,
    pub dominators: Vec<IntelligenceRung>,
}

/// INVARIANT: Each *ladder surface*'s shipped pairs, least to most intelligent on its
/// profile.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfacePairs {
    pub chat: Vec<SurfacePair>,
    pub claude_code: Vec<SurfacePair>,
    pub codex: Vec<SurfacePair>,
}

#[derive(Debug, Deserialize)]
pub struct RungEstimates {
    pub source: String,
    pub estimated: String,
    pub pairs: Vec<EstimatedPair>,
    pub surfaces: SurfacePairs,
}

pub const RUNG_ESTIMATES_JSON: &str = include_str!("rung_estimates.json");

static RUNG_ESTIMATES: LazyLock<RungEstimates> = LazyLock::new(|| {
    serde_json::from_str(RUNG_ESTIMATES_JSON)
        .expect("llm/rung_estimates.json is malformed: rerun estimate_rungs.py")
});

pub fn rung_estimates() -> &'static RungEstimates {
    &RUNG_ESTIMATES
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntelligenceRung {
    pub model: String,
    pub effort: String,
}

impl IntelligenceRung {
    pub fn new(model: &str, effort: &str) -> Self {
        Self {
            model: model.to_string(),
            effort: effort.to_string(),
        }
    }
}

impl std::fmt::Display for IntelligenceRung {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}", self.model, self.effort)
    }
}

pub const CONTEXT_VARIANT: &str = "[1m]";

pub fn intelligence_rung_model_base(id: &str) -> String {
    strip_version_pin(id.strip_suffix(CONTEXT_VARIANT).unwrap_or(id))
}

pub fn strip_version_pin(id: &str) -> String {
    id.replace("@default", "")
}
