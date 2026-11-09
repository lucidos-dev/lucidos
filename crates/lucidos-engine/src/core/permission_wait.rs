//! The *permission wait*: how long an unattended coding-agent session's parked
//! permission card waits for an answer before it is denied.
//!
//! One parser and one wire form for every layer: the trigger config, its
//! `trigger.toml`, the HTTP API, the SDK, the UI and the workspace preference
//! `unattended_permission_wait`. The wire form is `"deny"`, a whole number of
//! minutes, or `"indefinite"`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::time::Duration;

/// The longest finite wait, in minutes: one day.
pub const MAX_PERMISSION_WAIT_MINUTES: u16 = 1440;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionWait {
    /// Never park a card: deny at once, as before parking existed.
    Deny,
    /// Park a card, and deny it when nobody answers within this many minutes.
    Minutes(u16),
    /// Park a card and wait for an answer like an attended prompt.
    Indefinite,
}

impl PermissionWait {
    const DENY: &'static str = "deny";
    const INDEFINITE: &'static str = "indefinite";

    /// Parse the text form: `deny`, `indefinite`, or minutes from 1 to 1440.
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            Self::DENY => Ok(Self::Deny),
            Self::INDEFINITE => Ok(Self::Indefinite),
            other => other
                .parse::<u16>()
                .ok()
                .filter(|m| (1..=MAX_PERMISSION_WAIT_MINUTES).contains(m))
                .map(Self::Minutes)
                .ok_or_else(|| {
                    format!(
                        "a permission wait is \"deny\", \"indefinite\", or whole minutes from 1 \
                         to {MAX_PERMISSION_WAIT_MINUTES} (got {other:?})"
                    )
                }),
        }
    }

    /// Parse the JSON form: a string [`Self::parse`] reads, or a number of minutes.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        match value {
            Value::String(s) => Self::parse(s),
            Value::Number(n) => Self::parse(&n.to_string()),
            other => Err(format!(
                "a permission wait is a string or a number (got {other})"
            )),
        }
    }

    /// The JSON form: `"deny"`, a number of minutes, or `"indefinite"`.
    pub fn to_json(self) -> Value {
        match self {
            Self::Deny => Value::from(Self::DENY),
            Self::Minutes(m) => Value::from(m),
            Self::Indefinite => Value::from(Self::INDEFINITE),
        }
    }

    /// How long a parked card waits. `None` means it never parks; `Some(None)`
    /// means it waits with no bound.
    pub fn bound(self) -> Option<Option<Duration>> {
        match self {
            Self::Deny => None,
            Self::Minutes(m) => Some(Some(Duration::from_secs(u64::from(m) * 60))),
            Self::Indefinite => Some(None),
        }
    }
}

impl std::fmt::Display for PermissionWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deny => f.write_str(Self::DENY),
            Self::Minutes(m) => write!(f, "{m}"),
            Self::Indefinite => f.write_str(Self::INDEFINITE),
        }
    }
}

impl Serialize for PermissionWait {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PermissionWait {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_json(&Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// The workspace's wait for an unattended run whose root names none.
pub async fn workspace_permission_wait(pool: &sqlx::PgPool) -> PermissionWait {
    let raw = crate::core::prefs::UNATTENDED_PERMISSION_WAIT
        .read(pool)
        .await;
    PermissionWait::parse(&raw).expect("the catalog validates every value it resolves to")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_each_text_form() {
        assert_eq!(PermissionWait::parse("deny"), Ok(PermissionWait::Deny));
        assert_eq!(
            PermissionWait::parse("indefinite"),
            Ok(PermissionWait::Indefinite)
        );
        assert_eq!(
            PermissionWait::parse(" 30 "),
            Ok(PermissionWait::Minutes(30))
        );
        assert_eq!(PermissionWait::parse("1"), Ok(PermissionWait::Minutes(1)));
        assert_eq!(
            PermissionWait::parse("1440"),
            Ok(PermissionWait::Minutes(MAX_PERMISSION_WAIT_MINUTES))
        );
    }

    #[test]
    fn refuses_what_no_layer_accepts() {
        for raw in ["0", "1441", "-5", "2.5", "", "forever", "Deny"] {
            assert!(
                PermissionWait::parse(raw).is_err(),
                "{raw:?} must be refused"
            );
        }
        assert!(PermissionWait::from_json(&json!(true)).is_err());
        assert!(PermissionWait::from_json(&json!(null)).is_err());
        assert!(PermissionWait::from_json(&json!(2.5)).is_err());
    }

    #[test]
    fn the_json_form_round_trips() {
        for wait in [
            PermissionWait::Deny,
            PermissionWait::Minutes(45),
            PermissionWait::Indefinite,
        ] {
            let wire = serde_json::to_value(wait).unwrap();
            assert_eq!(wire, wait.to_json());
            assert_eq!(
                serde_json::from_value::<PermissionWait>(wire).unwrap(),
                wait
            );
            assert_eq!(PermissionWait::parse(&wait.to_string()), Ok(wait));
        }
        // The wire contract: minutes are a JSON number, the others are strings.
        assert_eq!(PermissionWait::Minutes(45).to_json(), json!(45));
        assert_eq!(PermissionWait::Deny.to_json(), json!("deny"));
    }

    #[test]
    fn bound_says_whether_and_how_long_a_card_parks() {
        assert_eq!(PermissionWait::Deny.bound(), None);
        assert_eq!(
            PermissionWait::Minutes(2).bound(),
            Some(Some(Duration::from_secs(120)))
        );
        assert_eq!(PermissionWait::Indefinite.bound(), Some(None));
    }
}
