use std::{borrow::Cow, fmt};

pub const BETA_FEED_URL: &str = "https://updates.lucidos.dev/beta/latest.json";
pub const NIGHTLY_FEED_URL: &str = "https://updates.lucidos.dev/nightly/latest.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomName(String);

impl CustomName {
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty()
            || raw.trim() != raw
            || raw.chars().count() > 64
            || raw
                .chars()
                .any(|c| c.is_control() || matches!(c, ':' | '/' | '\\'))
        {
            return Err("a custom slot name needs 1 to 64 characters, no surrounding whitespace, controls, colon or slashes".into());
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Channel {
    #[default]
    Stable,
    Beta,
    Nightly,
    Local,
    Custom(CustomName),
}

impl Channel {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "stable" => Ok(Self::Stable),
            "beta" => Ok(Self::Beta),
            "nightly" => Ok(Self::Nightly),
            "local" => Ok(Self::Local),
            _ => match raw.strip_prefix("custom:") {
                Some(name) => CustomName::parse(name).map(Self::Custom),
                None => Err(format!("unknown update channel: {raw}")),
            },
        }
    }

    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            Self::Stable => "stable".into(),
            Self::Beta => "beta".into(),
            Self::Nightly => "nightly".into(),
            Self::Local => "local".into(),
            Self::Custom(name) => format!("custom:{}", name.as_str()).into(),
        }
    }

    pub fn stability_rank(&self) -> Option<u8> {
        match self {
            Self::Stable => Some(0),
            Self::Beta => Some(1),
            Self::Nightly => Some(2),
            Self::Local | Self::Custom(_) => None,
        }
    }

    pub fn serves(&self, install_shape: Option<InstallShape>) -> bool {
        match self {
            Self::Stable | Self::Custom(_) => true,
            Self::Beta | Self::Local => install_shape == Some(InstallShape::DesktopApp),
            Self::Nightly => false,
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallShape {
    DesktopApp,
    InstallerRerun,
}

impl InstallShape {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DesktopApp => "desktop-app",
            Self::InstallerRerun => "installer-rerun",
        }
    }
}
