//! PreToolUse hook for Claude Code's `Read` tool — coerces string-typed
//! integer fields (`offset`, `limit`) to numbers before CC validates the
//! input.
//!
//! Background: the model occasionally sends `"offset": "16384"` (string)
//! instead of `"offset": 16384` (numeric). CC's input validator then
//! returns `<tool_use_error>InputValidationError: The parameter 'offset'
//! type is expected as 'number' but provided as 'string'</tool_use_error>`
//! and the call wastes a turn. Workspace-learning observed this 6 times
//! across 3 distinct threads in 24h, so it is a recurring model habit
//! rather than a one-off — cheap to absorb on the engine side.
//!
//! The fix uses CC's PreToolUse `updatedInput` mechanism (v2.0.10+):
//! this hook parses `tool_input` from stdin, coerces string-typed integers
//! to numbers for the two known fields, and emits the corrected input as
//! `updatedInput`. CC then runs Read with the corrected input. We only
//! print the JSON envelope when at least one field was actually rewritten
//! — well-behaved callers fall through with no overhead beyond the
//! subprocess spawn and a single JSON parse.
//!
//! Wired into `<workspace>/.lucidos/cc-settings.json` via the engine's
//! `cc_settings.rs`. Fails OPEN on parse / I/O errors so a hook bug can't
//! brick every Read call.

use serde::Deserialize;
use serde_json::Value;
use std::io::Read;

use crate::workspace::BoxError;

#[derive(Debug, Deserialize)]
struct HookPayload {
    tool_input: Value,
}

/// Coerce string-typed integer values for the known integer-typed Read
/// fields (`offset`, `limit`) to numbers in place. Returns `(input, mutated)`
/// — `mutated` is true iff at least one field was rewritten. Non-numeric
/// strings, missing fields, and already-numeric values pass through
/// unchanged so CC's own validator surfaces real errors normally.
pub(crate) fn coerce(mut input: Value) -> (Value, bool) {
    let mut mutated = false;
    for key in ["offset", "limit"] {
        let Some(field) = input.get(key) else { continue };
        let Some(s) = field.as_str() else { continue };
        let Ok(n) = s.parse::<u64>() else { continue };
        input[key] = Value::from(n);
        mutated = true;
    }
    (input, mutated)
}

pub(crate) fn build_hook_output(coerced_input: &Value) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": coerced_input,
        }
    })
    .to_string()
}

pub(crate) fn run() -> Result<(), BoxError> {
    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        eprintln!("cc-read-coerce: stdin read failed, allowing: {}", e);
        return Ok(());
    }
    let payload: HookPayload = match serde_json::from_str(&buf) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cc-read-coerce: payload parse failed, allowing: {}", e);
            return Ok(());
        }
    };
    let (coerced, mutated) = coerce(payload.tool_input);
    if mutated {
        println!("{}", build_hook_output(&coerced));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numeric_offset_passes_through_untouched() {
        let input = json!({"file_path": "/tmp/x", "offset": 123});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated, "well-behaved input must not be marked as mutated");
        assert_eq!(out, input);
    }

    #[test]
    fn string_offset_is_coerced_to_number() {
        let input = json!({"file_path": "/tmp/x", "offset": "16384"});
        let (out, mutated) = coerce(input);
        assert!(mutated);
        assert_eq!(out["offset"], json!(16384));
        assert!(
            out["offset"].is_number(),
            "coerced offset must be a JSON number, not a string"
        );
    }

    #[test]
    fn string_limit_is_coerced_to_number() {
        let input = json!({"file_path": "/tmp/x", "limit": "200"});
        let (out, mutated) = coerce(input);
        assert!(mutated);
        assert_eq!(out["limit"], json!(200));
        assert!(out["limit"].is_number());
    }

    #[test]
    fn both_string_offset_and_limit_are_coerced() {
        let input = json!({"file_path": "/tmp/x", "offset": "100", "limit": "50"});
        let (out, mutated) = coerce(input);
        assert!(mutated);
        assert_eq!(out["offset"], json!(100));
        assert_eq!(out["limit"], json!(50));
    }

    #[test]
    fn missing_offset_and_limit_is_a_no_op() {
        let input = json!({"file_path": "/tmp/x"});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated);
        assert_eq!(out, input);
    }

    #[test]
    fn non_numeric_string_passes_through_unchanged() {
        // Garbage string — CC's own validator should still surface this as
        // an error so the model learns the schema. We only coerce when the
        // string parses cleanly as an unsigned integer.
        let input = json!({"file_path": "/tmp/x", "offset": "not-a-number"});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated);
        assert_eq!(out, input);
    }

    #[test]
    fn negative_string_is_not_coerced() {
        // Read offset/limit are unsigned integers. A negative value would
        // be a model bug worth surfacing — don't silently coerce.
        let input = json!({"file_path": "/tmp/x", "offset": "-10"});
        let (out, mutated) = coerce(input);
        assert!(!mutated);
        assert_eq!(out["offset"], json!("-10"));
    }

    #[test]
    fn other_string_fields_are_not_touched() {
        // file_path is a string; coercion must be field-scoped to offset/limit.
        let input = json!({"file_path": "12345", "offset": 0});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated);
        assert_eq!(out, input);
    }

    #[test]
    fn build_hook_output_uses_documented_envelope() {
        let coerced = json!({"file_path": "/tmp/x", "offset": 16384});
        let out = build_hook_output(&coerced);
        let parsed: Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(parsed["hookSpecificOutput"]["updatedInput"], coerced);
    }

    #[test]
    fn coerced_envelope_round_trips_through_cc_validator_shape() {
        // Sanity check: the exact failure shape from the report —
        // string offset on a real file_path — must produce a numeric offset
        // in the envelope's updatedInput.
        let raw = json!({"file_path": "/x/run_session.rs", "offset": "16384"});
        let (coerced, mutated) = coerce(raw);
        assert!(mutated);
        let envelope: Value =
            serde_json::from_str(&build_hook_output(&coerced)).expect("valid JSON");
        assert!(
            envelope["hookSpecificOutput"]["updatedInput"]["offset"].is_number(),
            "envelope.updatedInput.offset must be a JSON number — that is the whole point of this hook",
        );
    }
}
