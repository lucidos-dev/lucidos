use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::core::apps::{TileSize, WidgetSize};

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

    pub fn from_arg(raw: &str) -> Result<Self, String> {
        Self::parse(raw).ok_or_else(|| format!("dashboard must be phone or desktop, not '{raw}'"))
    }

    pub fn for_user_agent(ua: &str) -> Self {
        if ua.contains("iPhone") || (ua.contains("Android") && ua.contains("Mobile")) {
            Self::Phone
        } else {
            Self::Desktop
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ArrangedTile {
    pub app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, Value>>,
    pub size: TileSize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LegacyArrangedTile {
    pub app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, Value>>,
    pub size: WidgetSize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

pub type WidgetParams = std::collections::BTreeMap<String, serde_json::Value>;
