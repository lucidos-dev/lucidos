use lucidos_core::core::event_hooks::{Refusal, RefusalCause, ReturnedEvent};
use lucidos_core::core::event_subscription::validate_emittable_event_type;
use lucidos_core::core::{redact_subprocess_output_in_json, sanitize_for_jsonb};
use serde::Deserialize;
use serde_json::Value;

use crate::json_strings::map_object_strings;

#[derive(Debug, Clone, PartialEq)]
pub enum HookAnswer {
    Allow { events: Vec<ReturnedEvent> },
    NeedsInput { argument: String },
    Refuse(Refusal),
}

pub(crate) fn map_event_strings(
    event: ReturnedEvent,
    f: &impl Fn(&str) -> String,
) -> Result<ReturnedEvent, String> {
    Ok(ReturnedEvent {
        event_type: f(&event.event_type),
        payload: map_object_strings(event.payload, f)?,
    })
}

pub(crate) fn masked_event(event: ReturnedEvent, secrets: &[String]) -> ReturnedEvent {
    let mut payload = Value::Object(event.payload);
    redact_subprocess_output_in_json(&mut payload, secrets);
    let Value::Object(payload) = payload else {
        unreachable!("masking keeps an object an object");
    };
    ReturnedEvent { payload, ..event }
}

#[derive(Deserialize)]
#[serde(tag = "answer", rename_all = "kebab-case", deny_unknown_fields)]
enum WireAnswer {
    Allow {
        #[serde(default)]
        events: Vec<ReturnedEvent>,
    },
    NeedsInput {
        argument: String,
    },
    Refuse {
        reason: String,
    },
}

pub(crate) fn parse(stdout: &str) -> Result<HookAnswer, String> {
    let wire: WireAnswer = serde_json::from_str(stdout.trim())
        .map_err(|e| sanitize_for_jsonb(&format!("the stdout is not an answer: {e}")))?;
    match wire {
        WireAnswer::Allow { events } => {
            let events = events
                .into_iter()
                .map(|event| map_event_strings(event, &sanitize_for_jsonb))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|key| {
                    format!("two keys of an event payload read as '{key}' without NUL bytes")
                })?;
            for event in &events {
                validate_emittable_event_type(&event.event_type)
                    .map_err(|e| format!("a hook returns domain events only: {e}"))?;
            }
            Ok(HookAnswer::Allow { events })
        }
        WireAnswer::NeedsInput { argument } => Ok(HookAnswer::NeedsInput {
            argument: non_blank("argument", &argument)?,
        }),
        WireAnswer::Refuse { reason } => Ok(HookAnswer::Refuse(Refusal {
            cause: RefusalCause::Refused,
            reason: non_blank("reason", &reason)?,
            detail: None,
        })),
    }
}

fn non_blank(field: &str, value: &str) -> Result<String, String> {
    let value = sanitize_for_jsonb(value.trim());
    if value.is_empty() {
        return Err(format!("{field} must not be blank."));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn refusal_reason(answer: HookAnswer) -> String {
        match answer {
            HookAnswer::Refuse(Refusal {
                cause: RefusalCause::Refused,
                reason,
                detail: None,
            }) => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn allow_needs_no_events() {
        assert_eq!(
            parse("  {\"answer\": \"allow\"}\n"),
            Ok(HookAnswer::Allow { events: Vec::new() })
        );
    }

    #[test]
    fn allow_returns_domain_events() {
        let answer = parse(
            r#"{"answer": "allow", "events": [
                {"event_type": "TicketLinked", "payload": {"summary": "Linked"}},
                {"event_type": "ReviewNoted"}
            ]}"#,
        )
        .unwrap();
        let HookAnswer::Allow { events } = answer else {
            panic!("expected allow, got {answer:?}");
        };
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "TicketLinked");
        assert_eq!(
            Value::Object(events[0].payload.clone()),
            json!({"summary": "Linked"})
        );
        assert!(events[1].payload.is_empty());
    }

    #[test]
    fn an_engine_event_is_no_answer() {
        for name in ["ChangeApplied", "Change\u{0}Applied", "MessageReceived", ""] {
            let stdout = json!({"answer": "allow", "events": [{"event_type": name}]});
            let err = parse(&stdout.to_string()).unwrap_err();
            assert!(err.contains("domain events only"), "{name}: {err}");
        }
    }

    #[test]
    fn an_answer_holds_no_nul() {
        let answer = parse(
            r#"{"answer": "allow", "events": [
                {"event_type": "Ticket\u0000Linked", "payload": {"k\u0000": ["a\u0000b"]}}
            ]}"#,
        )
        .unwrap();
        let HookAnswer::Allow { events } = answer else {
            panic!("expected allow, got {answer:?}");
        };
        assert_eq!(events[0].event_type, "TicketLinked");
        assert_eq!(
            Value::Object(events[0].payload.clone()),
            json!({"k": ["ab"]})
        );

        let err = parse(r#"{"answer": "x\u0000"}"#).unwrap_err();
        assert!(!err.contains('\0'), "{err:?}");
    }

    #[test]
    fn keys_that_collide_without_nul_refuse() {
        let err = parse(
            r#"{"answer": "allow", "events": [
                {"event_type": "TicketLinked", "payload": {"n": {"k": 1, "k\u0000": 2}}}
            ]}"#,
        )
        .unwrap_err();
        assert!(err.contains("'k'"), "{err:?}");
        assert!(!err.contains('\0'), "{err:?}");
    }

    #[test]
    fn needs_input_names_an_argument() {
        assert_eq!(
            parse(r#"{"answer": "needs-input", "argument": "ticket"}"#),
            Ok(HookAnswer::NeedsInput {
                argument: "ticket".to_string()
            })
        );
        assert!(parse(r#"{"answer": "needs-input", "argument": " "}"#).is_err());
        assert!(parse(r#"{"answer": "needs-input"}"#).is_err());
    }

    #[test]
    fn refuse_carries_its_reason() {
        let answer = parse(r#"{"answer": "refuse", "reason": " No plan approved. "}"#).unwrap();
        assert_eq!(refusal_reason(answer), "No plan approved.");
        assert!(parse(r#"{"answer": "refuse", "reason": ""}"#).is_err());
        assert!(parse(r#"{"answer": "refuse"}"#).is_err());
    }

    #[test]
    fn anything_else_is_no_answer() {
        for stdout in [
            "",
            "allow",
            "{}",
            r#"{"answer": "deny", "reason": "x"}"#,
            r#"{"answer": "needs_input", "argument": "x"}"#,
            r#"{"answer": "refuse", "reson": "x"}"#,
            r#"{"answer": "allow", "event": []}"#,
            r#"{"answer": "allow", "events": [{"event_type": "A", "data": {}}]}"#,
            r#"{"answer": "allow", "events": [{"event_type": "A", "payload": []}]}"#,
            r#"{"answer": "allow"} {"answer": "refuse", "reason": "x"}"#,
        ] {
            assert!(parse(stdout).is_err(), "{stdout:?} parsed");
        }
    }
}
