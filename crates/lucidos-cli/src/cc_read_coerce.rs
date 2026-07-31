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

/// Parse `"[N]"` or `"[N, M]"` (with arbitrary whitespace). Returns
/// `Some((start, end))` where `end` is `None` for the single-element form.
fn parse_range_string(s: &str) -> Option<(u64, Option<u64>)> {
    let inner = s.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut parts = inner.split(',').map(str::trim);
    let start = parts.next()?.parse::<u64>().ok()?;
    let end = match parts.next() {
        None => None,
        Some(s) => Some(s.parse::<u64>().ok()?),
    };
    if parts.next().is_some() {
        return None;
    }
    if end.is_some_and(|e| e < start) {
        return None;
    }
    Some((start, end))
}

/// If `input[key]` is a numeric string, rewrite it as a JSON number and
/// return true. Non-numeric strings, missing keys, and existing numbers
/// pass through unchanged so CC's validator still surfaces real errors.
fn coerce_numeric_string(input: &mut Value, key: &str) -> bool {
    let Some(s) = input.get(key).and_then(Value::as_str) else {
        return false;
    };
    let Ok(n) = s.parse::<u64>() else {
        return false;
    };
    input[key] = Value::from(n);
    true
}

/// Coerce string-typed integer values for `offset` and `limit` to numbers.
/// Returns `(input, mutated)`. Non-numeric strings, missing fields, and
/// already-numeric values pass through unchanged.
///
/// Two shapes are coerced for `offset`:
///   1. Pure numeric string: `"123"` → `123`.
///   2. Range string: `"[start]"` → `offset=start`; `"[start, end]"` →
///      `offset=start, limit=end-start+1` (only when no explicit `limit`
///      was supplied — an explicit limit overrides the derived one).
///
/// For `limit`, only the numeric string shape is recognised; a range
/// string in `limit` has no obvious meaning, so it passes through.
pub(crate) fn coerce(mut input: Value) -> (Value, bool) {
    let mut mutated = coerce_numeric_string(&mut input, "offset");
    if !mutated {
        if let Some(s) = input.get("offset").and_then(Value::as_str) {
            if let Some((start, end)) = parse_range_string(s) {
                input["offset"] = Value::from(start);
                if let Some(end) = end {
                    if !input.get("limit").is_some_and(|v| v.is_number()) {
                        input["limit"] = Value::from(end - start + 1);
                    }
                }
                mutated = true;
            }
        }
    }
    if coerce_numeric_string(&mut input, "limit") {
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
    fn range_string_offset_becomes_numeric_start_and_derives_limit() {
        let input = json!({"file_path": "/tmp/x", "offset": "[497, 532]"});
        let (out, mutated) = coerce(input);
        assert!(mutated);
        assert_eq!(out["offset"], json!(497));
        assert_eq!(out["limit"], json!(36));
    }

    #[test]
    fn range_string_offset_keeps_explicit_limit() {
        // Explicit limit overrides the derived one — model is allowed to
        // override and we don't second-guess contradictory shapes.
        let input = json!({"file_path": "/tmp/x", "offset": "[235, 260]", "limit": 30});
        let (out, mutated) = coerce(input);
        assert!(mutated);
        assert_eq!(out["offset"], json!(235));
        assert_eq!(out["limit"], json!(30));
    }

    #[test]
    fn single_element_range_string_offset_is_coerced() {
        let input = json!({"file_path": "/tmp/x", "offset": "[200]", "limit": 100});
        let (out, mutated) = coerce(input);
        assert!(mutated);
        assert_eq!(out["offset"], json!(200));
        assert_eq!(out["limit"], json!(100));
    }

    #[test]
    fn single_element_range_string_offset_no_limit_keeps_no_limit() {
        let input = json!({"file_path": "/tmp/x", "offset": "[200]"});
        let (out, mutated) = coerce(input);
        assert!(mutated);
        assert_eq!(out["offset"], json!(200));
        assert!(out.get("limit").is_none());
    }

    #[test]
    fn range_string_with_inverted_bounds_passes_through() {
        // end < start is meaningless; let CC's validator surface it.
        let input = json!({"file_path": "/tmp/x", "offset": "[60, 1]"});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated);
        assert_eq!(out, input);
    }

    #[test]
    fn range_string_with_extra_elements_passes_through() {
        let input = json!({"file_path": "/tmp/x", "offset": "[1, 2, 3]"});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated);
        assert_eq!(out, input);
    }

    #[test]
    fn range_string_with_whitespace_is_coerced() {
        let input = json!({"file_path": "/tmp/x", "offset": "[ 100 , 200 ]"});
        let (out, mutated) = coerce(input);
        assert!(mutated);
        assert_eq!(out["offset"], json!(100));
        assert_eq!(out["limit"], json!(101));
    }

    #[test]
    fn range_string_with_non_numeric_components_passes_through() {
        let input = json!({"file_path": "/tmp/x", "offset": "[a, b]"});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated);
        assert_eq!(out, input);
    }

    #[test]
    fn range_string_on_limit_passes_through() {
        // Range on limit has no obvious meaning; only numeric-string coercion applies.
        let input = json!({"file_path": "/tmp/x", "limit": "[10, 20]"});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated);
        assert_eq!(out, input);
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
