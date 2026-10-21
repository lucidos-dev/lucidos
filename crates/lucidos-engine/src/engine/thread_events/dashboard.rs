//! The two *dashboards*, and the legacy `HomeBoardArranged` tile that seeds
//! them (ADR 0432, ADR 0496).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::core::apps::{TileSize, WidgetSize};

/// One of the two dashboards. The phone's swipe layout shows `phone`; the
/// split layout shows `desktop`, a tablet included.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Dashboard {
    Phone,
    Desktop,
}

impl Dashboard {
    pub const ALL: [Self; 2] = [Self::Phone, Self::Desktop];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Phone => "phone",
            Self::Desktop => "desktop",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == name)
    }

    /// A caller's `dashboard` argument, or why it names no dashboard.
    pub fn from_arg(raw: &str) -> Result<Self, String> {
        Self::parse(raw).ok_or_else(|| format!("dashboard must be phone or desktop, not '{raw}'"))
    }

    /// The dashboard a device shows, read off its user agent: a phone shows
    /// `phone`, anything else `desktop`. An Android tablet omits `Mobile`.
    pub fn for_user_agent(ua: &str) -> Self {
        if ua.contains("iPhone") || (ua.contains("Android") && ua.contains("Mobile")) {
            Self::Phone
        } else {
            Self::Desktop
        }
    }
}

/// One tile of a `DashboardArranged`: a widget instance or an app shortcut on
/// that dashboard, and the size it shows at.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ArrangedTile {
    pub app_id: String,
    /// The instance's *widget params*. Absent means none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, Value>>,
    pub size: TileSize,
}

/// One tile of a legacy `HomeBoardArranged`, written before the two
/// dashboards had their own tiles. A hidden tile seeds nothing.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LegacyArrangedTile {
    pub app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, Value>>,
    pub size: WidgetSize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}
