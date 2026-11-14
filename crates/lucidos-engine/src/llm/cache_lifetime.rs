//! The lifetimes a provider's prompt cache offers. The engine decides which
//! one a write got from evidence alone (`engine::cache_ttl`, ADR 0575).

use serde::{Deserialize, Serialize};

use crate::llm::model_prices::CacheShares;

/// A lifetime a provider's cache offers. Every number a reader times or
/// prices from lives here, so an Unknown TTL cannot reach one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CacheLifetime {
    #[serde(rename = "5m")]
    FiveMinutes,
    /// OpenAI's minimum from GPT-5.6 on.
    #[serde(rename = "30m")]
    ThirtyMinutes,
    #[serde(rename = "1h")]
    OneHour,
    /// OpenAI's longer lifetime from GPT-5.6 on.
    #[serde(rename = "12h")]
    TwelveHours,
}

impl CacheLifetime {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 4] = [
        Self::FiveMinutes,
        Self::ThirtyMinutes,
        Self::OneHour,
        Self::TwelveHours,
    ];

    /// The lifetime, in seconds.
    pub(crate) fn lifetime_secs(self) -> f64 {
        match self {
            Self::FiveMinutes => 300.0,
            Self::ThirtyMinutes => 1800.0,
            Self::OneHour => 3600.0,
            Self::TwelveHours => 43_200.0,
        }
    }

    /// What a write at this lifetime costs, as a share of the input price.
    pub(crate) fn write_share(self, cache: CacheShares) -> f64 {
        match self {
            Self::FiveMinutes | Self::ThirtyMinutes => cache.write_5m,
            Self::OneHour => cache.write_1h,
            Self::TwelveHours => cache.write_12h,
        }
    }

    /// The lifetime, in the words the session context states it.
    pub(crate) fn lifetime_words(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5 minutes",
            Self::ThirtyMinutes => "30 minutes",
            Self::OneHour => "1 hour",
            Self::TwelveHours => "12 hours",
        }
    }

    /// The longest blocking call whose next request still lands inside the
    /// cache, with room for the call's own output and turn overhead.
    pub(crate) fn blocking_cap_words(self) -> &'static str {
        match self {
            Self::FiveMinutes => "4 minutes",
            Self::ThirtyMinutes => "25 minutes",
            Self::OneHour => "50 minutes",
            Self::TwelveHours => "11 hours",
        }
    }
}
