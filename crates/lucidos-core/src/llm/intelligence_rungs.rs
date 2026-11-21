use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::core::coding_agent::CodingAgent;
use crate::core::preference_catalog as prefs;

/// Where an intelligence ladder applies: who picks from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LadderSurface {
    /// The Lucidos Agent's chat threads.
    Chat,
    ClaudeCode,
    Codex,
}

impl LadderSurface {
    pub const ALL: [Self; 3] = [Self::Chat, Self::ClaudeCode, Self::Codex];

    /// The backend a coding-agent surface runs on. `None` for chat.
    pub fn coding_agent(self) -> Option<CodingAgent> {
        match self {
            Self::Chat => None,
            Self::ClaudeCode => Some(CodingAgent::ClaudeCode),
            Self::Codex => Some(CodingAgent::Codex),
        }
    }

    /// The wire string: `chat`, `claude-code` or `codex`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::ClaudeCode => CodingAgent::ClaudeCode.as_str(),
            Self::Codex => CodingAgent::Codex.as_str(),
        }
    }

    /// The name a refusal uses: "the Lucidos Agent intelligence ladder".
    pub fn display_name(self) -> &'static str {
        self.coding_agent()
            .map_or("Lucidos Agent", CodingAgent::display_name)
    }

    /// The efforts a rung on this surface may name. Chat takes `none`, which
    /// no coding agent offers.
    pub fn efforts(self) -> &'static [&'static str] {
        match self {
            Self::Chat => prefs::REASONING_EFFORTS,
            Self::ClaudeCode | Self::Codex => prefs::CODING_AGENT_EFFORTS,
        }
    }

    /// The owner's override for this surface.
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

    /// The *intelligence profile* this surface ranks on: chat on general,
    /// the coding agents on coding.
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

/// One pair's bands in the rung estimates. A token or time band counts steps
/// from the reference pair's, on its own grid. An intelligence band is a
/// profile's estimated score in steps of 2 points, so the file never carries
/// the score itself (ADR 0527).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EstimatedPair {
    pub model: String,
    pub effort: String,
    /// The *general* profile, which chat ranks on.
    pub general_band: i32,
    /// The *coding* profile, which the coding agents rank on.
    pub coding_band: i32,
    /// No coding score was published; the model gap estimates its coding band.
    /// Covered scores may also be smoothed by the monotone effort envelope.
    pub coding_estimated: bool,
    /// Billed tokens per task in output-token equivalents. Times the model's
    /// output price, it is the cost per task.
    pub token_band: i32,
    pub time_band: i32,
}

/// One pair of a surface's shipped list.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfacePair {
    pub model: String,
    pub effort: String,
    /// Global frontier flag retained for offline comparisons.
    pub on: bool,
    /// The shipped default remains on when available, even if dominated.
    pub pinned: bool,
    /// Every dominating pair, including ones outside the global frontier.
    pub dominators: Vec<IntelligenceRung>,
}

/// Each *ladder surface*'s shipped pairs, least to most intelligent on its
/// profile.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfacePairs {
    pub chat: Vec<SurfacePair>,
    pub claude_code: Vec<SurfacePair>,
    pub codex: Vec<SurfacePair>,
}

/// `llm/rung_estimates.json`, less its `_note`.
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

/// The rung estimates every surface shares.
pub fn rung_estimates() -> &'static RungEstimates {
    &RUNG_ESTIMATES
}

/// One model at one effort.
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

/// The suffix that runs a model in its 1M-context variant.
pub const CONTEXT_VARIANT: &str = "[1m]";

/// The model an intelligence rung names, without the decorations that keep
/// the same model: a `[1m]` *context variant* and a `@default` version pin.
pub fn intelligence_rung_model_base(id: &str) -> String {
    strip_version_pin(id.strip_suffix(CONTEXT_VARIANT).unwrap_or(id))
}

/// A model id without the `@default` version pin the Claude Code picker writes
/// on some rows. Shared by the two callers that must see through it, so they
/// cannot disagree about what the decoration means.
pub fn strip_version_pin(id: &str) -> String {
    id.replace("@default", "")
}
