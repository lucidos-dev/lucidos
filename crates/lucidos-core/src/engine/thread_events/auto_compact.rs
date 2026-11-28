use serde::{Deserialize, Serialize};

pub const TRIAL_WINDOW_TOKENS: u64 = 400_000;

pub const TRIAL_MODES: &[&str] = &[
    TrialMode::Off.as_str(),
    TrialMode::Ab.as_str(),
    TrialMode::On.as_str(),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialMode {
    Off,
    Ab,
    On,
}

impl TrialMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Ab, Self::On];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ab => "ab",
            Self::On => "on",
        }
    }

    pub fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == value)
            .unwrap_or(Self::Off)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoCompactArm {
    Control,
    Window,
    Override,
}
