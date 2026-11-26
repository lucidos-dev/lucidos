use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadState {
    Composing,
    Active,
    Discarded,
}

impl ThreadState {
    pub fn from_db_str(s: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        match s {
            "composing" => Ok(Self::Composing),
            "active" => Ok(Self::Active),
            "discarded" => Ok(Self::Discarded),
            other => Err(format!(
                "thread_summaries.state has unexpected value '{}' (expected composing|active|discarded)",
                other
            )
            .into()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Composing => "composing",
            Self::Active => "active",
            Self::Discarded => "discarded",
        }
    }

    pub fn can_change_mode(self) -> bool {
        matches!(self, Self::Composing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_composing_allows_a_mode_change() {
        assert!(ThreadState::Composing.can_change_mode());
        assert!(!ThreadState::Active.can_change_mode());
        assert!(!ThreadState::Discarded.can_change_mode());
    }

    #[test]
    fn from_db_str_round_trips() {
        for s in [
            ThreadState::Composing,
            ThreadState::Active,
            ThreadState::Discarded,
        ] {
            assert_eq!(ThreadState::from_db_str(s.as_str()).unwrap(), s);
        }
    }

    #[test]
    fn from_db_str_rejects_archived_and_unknown() {
        assert!(ThreadState::from_db_str("archived").is_err());
        assert!(ThreadState::from_db_str("bogus").is_err());
        assert!(ThreadState::from_db_str("").is_err());
    }
}
