//! The trigger **actor** (ADR 0623): the payload field holding the email of
//! the person behind a fire.
//!
//! The engine never names a source. The field is a payload path in the syntax
//! trigger conditions use. A fire records the email it resolves to, and a
//! display maps that email to an operator. `system-knowhow/triggers.md`
//! § "Actor" is the author's guide.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::event_subscription::condition;

/// The key holding a trigger's actor field in its lifecycle event payloads.
/// Not `actor`: the event bus writes the event's own actor under that key.
pub const ACTOR_FIELD_KEY: &str = "actor_field";

/// A trigger's actor field: one payload path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActorField(pub String);

const SHAPE_ERROR: &str = "actor must be a payload path naming an email field, \
     e.g. \"payload.sender.email\"";

impl ActorField {
    /// Parse a submitted field. `null` and `""` mean none, which clears one.
    pub fn parse(field: Option<&Value>) -> Result<Option<Self>, String> {
        match field {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(path)) => {
                let path = path.trim();
                Ok((!path.is_empty()).then(|| Self(path.to_string())))
            }
            Some(_) => Err(SHAPE_ERROR.to_string()),
        }
    }

    /// Refuse a field on a script trigger: a script fire records no origin.
    pub fn check_run(
        field: Option<&Self>,
        run: &crate::triggers::TriggerRun,
    ) -> Result<(), String> {
        match (field, run) {
            (Some(_), crate::triggers::TriggerRun::Script { .. }) => Err(
                "actor needs an intent trigger: a script fire starts no thread to credit"
                    .to_string(),
            ),
            _ => Ok(()),
        }
    }

    /// Read a stored field. A malformed one reads as none, so one bad event
    /// row cannot wedge the trigger registry.
    pub(crate) fn from_stored(field: Option<&Value>, trigger_id: &str) -> Option<Self> {
        Self::parse(field).unwrap_or_else(|e| {
            log!("[Triggers] Ignored actor on trigger {trigger_id}: {e}");
            None
        })
    }

    /// The email this payload names, lowercased. `None` when the field is
    /// absent or does not hold one email.
    pub fn email_in(&self, payload: &Value) -> Option<String> {
        let email = condition::resolve(payload, &self.0)?.as_str()?.trim();
        let (local, domain) = email.split_once('@')?;
        let one_email = !local.is_empty()
            && !domain.is_empty()
            && !domain.contains('@')
            && !email.contains(char::is_whitespace);
        one_email.then(|| email.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(path: &str) -> ActorField {
        ActorField(path.to_string())
    }

    #[test]
    fn parse_takes_a_path_and_clears_on_null_or_blank() {
        let path = json!(" payload.sender.email ");
        assert_eq!(
            ActorField::parse(Some(&path)),
            Ok(Some(field("payload.sender.email")))
        );
        assert_eq!(ActorField::parse(Some(&Value::Null)), Ok(None));
        assert_eq!(ActorField::parse(Some(&json!("  "))), Ok(None));
        assert_eq!(ActorField::parse(None), Ok(None));
        assert!(ActorField::parse(Some(&json!(["payload.a"]))).is_err());
    }

    #[test]
    fn a_script_trigger_refuses_an_actor() {
        let script = crate::triggers::TriggerRun::Script {
            path: "triggers/x/run.sh".into(),
        };
        assert!(ActorField::check_run(Some(&field("payload.a")), &script).is_err());
        assert!(ActorField::check_run(None, &script).is_ok());
    }

    #[test]
    fn email_in_resolves_and_lowercases_one_email() {
        let payload = json!({"payload": {"sender": {"email": " Sam@Example.com "}}});
        assert_eq!(
            field("payload.sender.email").email_in(&payload),
            Some("sam@example.com".to_string())
        );
    }

    #[test]
    fn email_in_refuses_what_is_not_one_email() {
        for value in [
            json!("sam"),
            json!("@example.com"),
            json!("sam@"),
            json!("a@b@c"),
            json!("sam smith@example.com"),
            json!(42),
            json!(null),
        ] {
            let payload = json!({"payload": {"email": value}});
            assert_eq!(field("payload.email").email_in(&payload), None, "{value}");
        }
        assert_eq!(field("payload.missing").email_in(&json!({})), None);
    }
}
