//! The Home board's arrangement, as `HomeBoardArranged` records it (ADR 0432).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::core::apps::WidgetSize;

/// Which arrangement of the Home board a screen shows. The phone's swipe
/// layout is `phone`; the split layout is `desktop`, a tablet included.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutClass {
    Phone,
    Desktop,
}

impl LayoutClass {
    pub const ALL: [Self; 2] = [Self::Phone, Self::Desktop];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Phone => "phone",
            Self::Desktop => "desktop",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|layout| layout.as_str() == name)
    }

    /// A caller's `layout` argument, or why it names no layout class.
    pub fn from_arg(raw: &str) -> Result<Self, String> {
        Self::parse(raw).ok_or_else(|| format!("layout must be phone or desktop, not '{raw}'"))
    }
}

/// One tile of an arrangement: a widget instance pinned to Home, the size it
/// shows at, and whether this layout class hides it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ArrangedTile {
    pub app_id: String,
    /// The instance's *widget params*. Absent means none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, Value>>,
    pub size: WidgetSize,
    /// Hidden on this layout class, so it mounts no frame there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}
