use serde::Deserialize;
use serde_json::Value;
use std::io::Read;
use std::path::{Component, Path};

use crate::workspace::BoxError;

#[derive(Debug, Deserialize)]
struct HookPayload {
    tool_input: Value,
    #[serde(default)]
    cwd: Option<String>,
}

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

fn permission_decision(input: &Value, cwd: Option<&str>) -> &'static str {
    let inside = match (input.get("file_path").and_then(Value::as_str), cwd) {
        (Some(file), Some(cwd)) => path_is_inside(Path::new(file), Path::new(cwd)),
        _ => false,
    };
    if inside {
        "allow"
    } else {
        "ask"
    }
}

fn path_is_inside(path: &Path, root: &Path) -> bool {
    let full = root.join(path);
    root.is_absolute()
        && !full.components().any(|c| c == Component::ParentDir)
        && full.starts_with(root)
}

pub(crate) fn build_hook_output(coerced_input: &Value, cwd: Option<&str>) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": permission_decision(coerced_input, cwd),
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
        println!("{}", build_hook_output(&coerced, payload.cwd.as_deref()));
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
        let input = json!({"file_path": "/tmp/x", "offset": "not-a-number"});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated);
        assert_eq!(out, input);
    }

    #[test]
    fn negative_string_is_not_coerced() {
        let input = json!({"file_path": "/tmp/x", "offset": "-10"});
        let (out, mutated) = coerce(input);
        assert!(!mutated);
        assert_eq!(out["offset"], json!("-10"));
    }

    #[test]
    fn other_string_fields_are_not_touched() {
        let input = json!({"file_path": "12345", "offset": 0});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated);
        assert_eq!(out, input);
    }

    #[test]
    fn build_hook_output_uses_documented_envelope() {
        let coerced = json!({"file_path": "/tmp/x", "offset": 16384});
        let out = build_hook_output(&coerced, Some("/tmp"));
        let parsed: Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(parsed["hookSpecificOutput"]["updatedInput"], coerced);
    }

    #[test]
    fn a_coerced_read_outside_the_working_directory_asks() {
        let decision = |file: &str, cwd: Option<&str>| {
            let input = json!({"file_path": file, "offset": 1});
            let out: Value = serde_json::from_str(&build_hook_output(&input, cwd)).unwrap();
            out["hookSpecificOutput"]["permissionDecision"].clone()
        };
        assert_eq!(decision("/home/u/.ssh/id_rsa", Some("/home/u/wt")), "ask");
        assert_eq!(
            decision("/home/u/wt/../.ssh/id_rsa", Some("/home/u/wt")),
            "ask"
        );
        assert_eq!(decision("/home/u/wtx/notes.md", Some("/home/u/wt")), "ask");
        assert_eq!(decision("/home/u/wt/src/main.rs", None), "ask");
        assert_eq!(
            decision("/home/u/wt/src/main.rs", Some("/home/u/wt")),
            "allow"
        );
        assert_eq!(decision("src/main.rs", Some("/home/u/wt")), "allow");
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
        let input = json!({"file_path": "/tmp/x", "limit": "[10, 20]"});
        let (out, mutated) = coerce(input.clone());
        assert!(!mutated);
        assert_eq!(out, input);
    }

    #[test]
    fn coerced_envelope_round_trips_through_cc_validator_shape() {
        let raw = json!({"file_path": "/x/run_session.rs", "offset": "16384"});
        let (coerced, mutated) = coerce(raw);
        assert!(mutated);
        let envelope: Value =
            serde_json::from_str(&build_hook_output(&coerced, Some("/x"))).expect("valid JSON");
        assert!(
            envelope["hookSpecificOutput"]["updatedInput"]["offset"].is_number(),
            "envelope.updatedInput.offset must be a JSON number — that is the whole point of this hook",
        );
    }
}
