use serde::{Deserialize, Serialize};

use crate::llm::model_prices::CacheShares;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CacheLifetime {
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "30m")]
    ThirtyMinutes,
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "12h")]
    TwelveHours,
}

impl CacheLifetime {
    #[cfg(any(test, feature = "test-support"))]
    pub const ALL: [Self; 4] = [
        Self::FiveMinutes,
        Self::ThirtyMinutes,
        Self::OneHour,
        Self::TwelveHours,
    ];

    pub fn lifetime_secs(self) -> f64 {
        match self {
            Self::FiveMinutes => 300.0,
            Self::ThirtyMinutes => 1800.0,
            Self::OneHour => 3600.0,
            Self::TwelveHours => 43_200.0,
        }
    }

    pub fn write_share(self, cache: CacheShares) -> f64 {
        match self {
            Self::FiveMinutes | Self::ThirtyMinutes => cache.write_5m,
            Self::OneHour => cache.write_1h,
            Self::TwelveHours => cache.write_12h,
        }
    }

    pub fn lifetime_words(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5 minutes",
            Self::ThirtyMinutes => "30 minutes",
            Self::OneHour => "1 hour",
            Self::TwelveHours => "12 hours",
        }
    }

    /// INVARIANT: The longest blocking call whose next request still lands inside the
    /// cache, with room for the call's own output and turn overhead.
    pub fn blocking_cap_words(self) -> &'static str {
        match self {
            Self::FiveMinutes => "4 minutes",
            Self::ThirtyMinutes => "25 minutes",
            Self::OneHour => "50 minutes",
            Self::TwelveHours => "11 hours",
        }
    }
}

use std::ops::RangeInclusive;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeatStart {
    Cold,
    /// INVARIANT: A prefill goes straight out.
    Direct,
}

impl CacheLifetime {
    pub fn beat_window(self, start: BeatStart) -> RangeInclusive<f64> {
        match (self, start) {
            (Self::FiveMinutes, BeatStart::Cold) => 230.0..=260.0,
            (Self::FiveMinutes, BeatStart::Direct) => 260.0..=280.0,
            (Self::ThirtyMinutes, BeatStart::Cold) => 1500.0..=1560.0,
            (Self::ThirtyMinutes, BeatStart::Direct) => 1560.0..=1620.0,
            (Self::OneHour, BeatStart::Cold) => 3300.0..=3360.0,
            (Self::OneHour, BeatStart::Direct) => 3500.0..=3540.0,
            (Self::TwelveHours, BeatStart::Cold) => 42_600.0..=42_660.0,
            (Self::TwelveHours, BeatStart::Direct) => 42_900.0..=42_960.0,
        }
    }

    pub fn rewrite_in_beats(self, cache: CacheShares) -> f64 {
        self.write_share(cache) / cache.read
    }

    pub fn beat_every_secs(self, start: BeatStart) -> f64 {
        *self.beat_window(start).start()
    }
}
