//! Condition evaluator for event subscriptions.
//!
//! Evaluates a JSON condition against an event payload.
//! Operators: $eq, $ne, $lt, $lte, $gt, $gte, $in, $nin, $regex.
//! A bare value (no operator) is treated as $eq.
//! A key names a *field path*, so `workflow_run.event` reads one level down.
//! `$or` in key position takes a list of conditions and ORs them.
//!
//! **One predicate language, two consumers.** This lived under `triggers/` when
//! a trigger was the only thing that could subscribe to an event. A thread's
//! *event wait* subscribes with the same shape, so the evaluator moved here
//! rather than being duplicated: a `condition` that matches for a trigger must
//! match for a wait, and the only way to guarantee that is one function. See
//! [`super::EventSubscription::matches`], which is the sole caller both
//! dispatch paths go through.

use regex::Regex;
use serde_json::Value;

/// The combinator, and the only key that is read as one rather than as a field
/// path. Every other `$`-prefixed key in that position is reserved, so a later
/// combinator can land without reinterpreting a stored condition.
const OR: &str = "$or";

/// How deep `$or` may nest. A condition is caller-supplied data, so the depth
/// is bounded at both ends: [`validate`] refuses a deeper one at the write
/// surface, and [`evaluate`] answers false rather than recursing off the stack.
const MAX_OR_DEPTH: usize = 8;

/// Every operator, in the order the error message lists them.
const OPERATORS: &[&str] = &[
    "$eq", "$ne", "$lt", "$lte", "$gt", "$gte", "$in", "$nin", "$regex",
];

/// What a field path resolves to when nothing is there.
///
/// A `static`, because the resolver hands out a `&'static Value` for the absent
/// case and `Value` has drop glue, so a `const` would not promote.
static ABSENT: Value = Value::Null;

/// Evaluate a condition against an event payload.
/// Returns true if the payload matches (or condition is None).
pub fn evaluate(condition: Option<&Value>, payload: &Value) -> bool {
    match condition {
        None => true,
        Some(c) => evaluate_condition(c, payload, 0),
    }
}

fn evaluate_condition(condition: &Value, payload: &Value, depth: usize) -> bool {
    match condition.as_object() {
        None => false,
        Some(fields) => fields.iter().all(|(key, op)| {
            if key == OR {
                return evaluate_or(op, payload, depth);
            }
            // Reserved, so it never names a payload field. Otherwise a payload
            // carrying a literal `$and` key would satisfy a condition naming
            // it, and a later combinator would then change that stored
            // condition's verdict. [`validate`] refuses one at the write
            // surface; this is the same answer for anything already stored.
            if key.starts_with('$') {
                return false;
            }
            evaluate_op(op, resolve(payload, key))
        }),
    }
}

/// `$or`: any branch matching satisfies it, and it ANDs with its siblings
/// because it is one key among the condition's others.
fn evaluate_or(branches: &Value, payload: &Value, depth: usize) -> bool {
    if depth >= MAX_OR_DEPTH {
        return false;
    }
    match branches.as_array() {
        Some(list) => list
            .iter()
            .any(|branch| evaluate_condition(branch, payload, depth + 1)),
        None => false,
    }
}

/// Resolve a **field path** against the payload.
///
/// **The exact key wins at every level.** That first lookup is what the flat
/// evaluator always did, so no stored condition can change verdict: traversal
/// runs only where the old lookup already found nothing. It is also the only way
/// a third-party payload's literal dotted key stays nameable.
///
/// The split is at the FIRST dot, so the rule is "the whole remaining key, or a
/// left-to-right walk" rather than "the longest matching prefix".
///
/// Anything unresolvable is [`ABSENT`], the same JSON null a missing top-level
/// key has always produced. That covers a segment naming a scalar, an object
/// without the key, and any array on the path: a numeric segment is an ordinary
/// object key and never an index.
///
/// Iterative rather than recursive, because the number of segments is
/// caller-supplied.
fn resolve<'a>(payload: &'a Value, key: &str) -> &'a Value {
    let mut current = payload;
    let mut rest = key;
    loop {
        if let Some(found) = current.get(rest) {
            return found;
        }
        let Some((head, tail)) = rest.split_once('.') else {
            return &ABSENT;
        };
        let Some(child) = current.get(head) else {
            return &ABSENT;
        };
        current = child;
        rest = tail;
    }
}

fn evaluate_op(op: &Value, actual: &Value) -> bool {
    match op.as_object() {
        Some(ops) => ops
            .iter()
            .all(|(operator, expected)| match operator.as_str() {
                "$eq" => actual == expected,
                "$ne" => actual != expected,
                "$lt" => compare(actual, expected, |a, b| a < b),
                "$lte" => compare(actual, expected, |a, b| a <= b),
                "$gt" => compare(actual, expected, |a, b| a > b),
                "$gte" => compare(actual, expected, |a, b| a >= b),
                "$in" => in_list(expected, actual),
                // A malformed list fails closed on both, rather than `$nin`
                // negating its way into matching everything.
                "$nin" => expected.is_array() && !in_list(expected, actual),
                "$regex" => matches_regex(expected, actual),
                _ => false,
            }),
        None => actual == op, // bare value = equality check
    }
}

fn in_list(expected: &Value, actual: &Value) -> bool {
    expected
        .as_array()
        .map(|arr| arr.contains(actual))
        .unwrap_or(false)
}

/// `$regex`: an unanchored search over a JSON string.
///
/// Only a string is text, so a number, an object, an array and an absent path
/// are all misses. That groups the operator with `$in` rather than with `$ne`.
/// A pattern that does not compile matches nothing here; [`validate`] refuses it
/// at the write surface so it never gets that far.
fn matches_regex(pattern: &Value, actual: &Value) -> bool {
    let (Some(pattern), Some(actual)) = (pattern.as_str(), actual.as_str()) else {
        return false;
    };
    Regex::new(pattern)
        .map(|re| re.is_match(actual))
        .unwrap_or(false)
}

fn compare(a: &Value, b: &Value, cmp: fn(f64, f64) -> bool) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => cmp(a, b),
        _ => false,
    }
}

/// **Refuse a condition that could never match**, at the write surface.
///
/// An unsupported operator evaluates to false and says nothing, so a
/// subscription carrying one arms clean and waits forever. That is the failure
/// `super::check_subscriptions` exists to end, and `$regex` and `$or` each add
/// another way to hit it: an unparseable pattern, and a malformed branch list.
///
/// **It runs at write only, never over stored data.** A condition already in a
/// `TriggerCreated` or `EventWaitStarted` payload is never re-validated, so no
/// stored subscription can change verdict.
pub fn validate(condition: &Value) -> Result<(), String> {
    validate_condition(condition, 0)
}

fn validate_condition(condition: &Value, depth: usize) -> Result<(), String> {
    let Some(fields) = condition.as_object() else {
        return Err(
            "a condition must be a JSON object mapping field paths to values or operators"
                .to_string(),
        );
    };
    for (key, op) in fields {
        if key == OR {
            validate_or(op, depth)?;
            continue;
        }
        if key.starts_with('$') {
            return Err(format!(
                "'{key}' is not a combinator. `{OR}` is the only one, and every other \
                 `$`-prefixed name is reserved for a future one."
            ));
        }
        validate_op(key, op)?;
    }
    Ok(())
}

fn validate_or(branches: &Value, depth: usize) -> Result<(), String> {
    if depth >= MAX_OR_DEPTH {
        return Err(format!("`{OR}` may nest at most {MAX_OR_DEPTH} deep"));
    }
    let Some(list) = branches.as_array() else {
        return Err(format!("`{OR}` takes an array of conditions"));
    };
    if list.is_empty() {
        return Err(format!(
            "`{OR}` takes at least one condition; an empty list can never match"
        ));
    }
    for branch in list {
        validate_condition(branch, depth + 1)?;
    }
    Ok(())
}

fn validate_op(field: &str, op: &Value) -> Result<(), String> {
    let Some(ops) = op.as_object() else {
        return Ok(()); // a bare value is an equality check against anything
    };
    for (operator, expected) in ops {
        if !operator.starts_with('$') {
            return Err(format!(
                "'{field}' is followed by an object naming '{operator}', which is not an \
                 operator. To match a nested field, name the whole path: '{field}.{operator}'."
            ));
        }
        match operator.as_str() {
            "$eq" | "$ne" => {}
            "$lt" | "$lte" | "$gt" | "$gte" if !expected.is_number() => {
                return Err(format!("'{field}' {operator} compares numbers"));
            }
            "$in" | "$nin" if !expected.is_array() => {
                return Err(format!("'{field}' {operator} takes an array of values"));
            }
            "$lt" | "$lte" | "$gt" | "$gte" | "$in" | "$nin" => {}
            "$regex" => {
                let Some(pattern) = expected.as_str() else {
                    return Err(format!("'{field}' $regex takes a pattern string"));
                };
                Regex::new(pattern)
                    .map_err(|e| format!("'{field}' $regex is not a valid pattern: {e}"))?;
            }
            _ => {
                return Err(format!(
                    "'{operator}' is not an operator. Use one of: {}.",
                    OPERATORS.join(", ")
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn no_condition_always_matches() {
        assert!(evaluate(None, &json!({"anything": true})));
    }

    #[test]
    fn lt_operator() {
        let cond = json!({"sleep_score": {"$lt": 70}});
        assert!(evaluate(Some(&cond), &json!({"sleep_score": 55})));
        assert!(!evaluate(Some(&cond), &json!({"sleep_score": 85})));
    }

    #[test]
    fn lte_operator() {
        let cond = json!({"score": {"$lte": 70}});
        assert!(evaluate(Some(&cond), &json!({"score": 70})));
        assert!(!evaluate(Some(&cond), &json!({"score": 71})));
    }

    #[test]
    fn gt_operator() {
        let cond = json!({"temp": {"$gt": 30}});
        assert!(evaluate(Some(&cond), &json!({"temp": 35})));
        assert!(!evaluate(Some(&cond), &json!({"temp": 25})));
    }

    #[test]
    fn gte_operator() {
        let cond = json!({"temp": {"$gte": 30}});
        assert!(evaluate(Some(&cond), &json!({"temp": 30})));
        assert!(!evaluate(Some(&cond), &json!({"temp": 29})));
    }

    #[test]
    fn eq_operator() {
        let cond = json!({"status": {"$eq": "critical"}});
        assert!(evaluate(Some(&cond), &json!({"status": "critical"})));
        assert!(!evaluate(Some(&cond), &json!({"status": "ok"})));
    }

    #[test]
    fn ne_operator() {
        let cond = json!({"status": {"$ne": "ok"}});
        assert!(evaluate(Some(&cond), &json!({"status": "critical"})));
        assert!(!evaluate(Some(&cond), &json!({"status": "ok"})));
    }

    #[test]
    fn in_operator() {
        let cond = json!({"level": {"$in": ["warn", "error"]}});
        assert!(evaluate(Some(&cond), &json!({"level": "error"})));
        assert!(!evaluate(Some(&cond), &json!({"level": "info"})));
    }

    #[test]
    fn in_operator_with_numbers() {
        let cond = json!({"code": {"$in": [200, 201, 204]}});
        assert!(evaluate(Some(&cond), &json!({"code": 200})));
        assert!(!evaluate(Some(&cond), &json!({"code": 404})));
    }

    #[test]
    fn multiple_fields() {
        let cond = json!({"score": {"$lt": 70}, "category": {"$eq": "sleep"}});
        assert!(evaluate(
            Some(&cond),
            &json!({"score": 55, "category": "sleep"})
        ));
        assert!(!evaluate(
            Some(&cond),
            &json!({"score": 55, "category": "activity"})
        ));
    }

    #[test]
    fn bare_value_is_equality() {
        let cond = json!({"status": "active"});
        assert!(evaluate(Some(&cond), &json!({"status": "active"})));
        assert!(!evaluate(Some(&cond), &json!({"status": "paused"})));
    }

    #[test]
    fn missing_field_does_not_match() {
        let cond = json!({"score": {"$lt": 70}});
        assert!(!evaluate(Some(&cond), &json!({"other": 55})));
    }

    #[test]
    fn non_object_condition_returns_false() {
        let cond = json!("not an object");
        assert!(!evaluate(Some(&cond), &json!({"anything": true})));
    }

    #[test]
    fn multiple_operators_on_same_field() {
        let cond = json!({"score": {"$gte": 50, "$lt": 80}});
        assert!(evaluate(Some(&cond), &json!({"score": 65})));
        assert!(!evaluate(Some(&cond), &json!({"score": 85})));
        assert!(!evaluate(Some(&cond), &json!({"score": 40})));
    }

    /// The example used to be `$regex`, which is now an operator. Adding one
    /// revives any stored condition that named it, since such a condition was
    /// inert; `validate` refuses a new one at the write surface.
    #[test]
    fn unknown_operator_returns_false() {
        let cond = json!({"score": {"$like": ".*"}});
        assert!(!evaluate(Some(&cond), &json!({"score": "hello"})));
    }

    /// A `$`-prefixed key in FIELD position is reserved, so it is not resolved
    /// against the payload.
    ///
    /// The second case is the one that matters, and it is why the reservation
    /// has to be enforced in `evaluate` and not only in `validate`. A payload
    /// that literally carries the key would otherwise satisfy the condition, so
    /// promoting `$and` to a combinator later would change a stored verdict.
    #[test]
    fn a_reserved_field_key_never_matches() {
        let cond = json!({"$and": [{"a": 1}]});
        assert!(!evaluate(Some(&cond), &json!({"a": 1})));
        assert!(!evaluate(Some(&cond), &json!({"$and": [{"a": 1}]})));
        // The same holds for the shape that used to match everything.
        assert!(!evaluate(Some(&json!({"$foo": null})), &json!({"a": 1})));
    }

    #[test]
    fn compare_non_numeric_returns_false() {
        let cond = json!({"name": {"$lt": "foo"}});
        assert!(!evaluate(Some(&cond), &json!({"name": "bar"})));
    }

    // ── Field paths ─────────────────────────────────────────────────

    #[test]
    fn nested_path_matches_one_level_down() {
        let cond = json!({"workflow_run.event": "schedule"});
        assert!(evaluate(
            Some(&cond),
            &json!({"action": "completed", "workflow_run": {"event": "schedule"}})
        ));
        assert!(!evaluate(
            Some(&cond),
            &json!({"action": "completed", "workflow_run": {"event": "push"}})
        ));
    }

    #[test]
    fn nested_path_matches_several_levels_down() {
        let cond = json!({"a.b.c": 1});
        assert!(evaluate(Some(&cond), &json!({"a": {"b": {"c": 1}}})));
        assert!(!evaluate(Some(&cond), &json!({"a": {"b": {"c": 2}}})));
    }

    /// Decision 1. Exact-key-first is what makes back-compat structural: the
    /// first lookup is the one the flat evaluator always did.
    #[test]
    fn literal_dotted_key_wins_over_the_path() {
        let cond = json!({"a.b": "literal"});
        assert!(evaluate(
            Some(&cond),
            &json!({"a.b": "literal", "a": {"b": "nested"}})
        ));
        assert!(!evaluate(Some(&cond), &json!({"a.b": "nested"})));
    }

    /// The exact key wins at EVERY level, not just the top one.
    #[test]
    fn literal_dotted_key_wins_below_the_top_level() {
        let cond = json!({"a.b.c": 1});
        assert!(evaluate(Some(&cond), &json!({"a": {"b.c": 1}})));
    }

    /// The split is at the FIRST dot, so a literal prefix is never a head.
    #[test]
    fn the_walk_splits_at_the_first_dot() {
        let cond = json!({"a.b.c": 1});
        let payload = json!({"a": {"b": {"c": 1}}, "a.b": {"c": 2}});
        assert!(evaluate(Some(&cond), &payload));
    }

    #[test]
    fn every_operator_reads_a_nested_path() {
        let payload = json!({"usage": {"input_tokens": 250000, "model": "b"}});
        for (cond, expected) in [
            (json!({"usage.input_tokens": {"$gt": 200000}}), true),
            (json!({"usage.input_tokens": {"$gt": 300000}}), false),
            (json!({"usage.input_tokens": {"$gte": 250000}}), true),
            (json!({"usage.input_tokens": {"$lt": 300000}}), true),
            (json!({"usage.input_tokens": {"$lte": 1}}), false),
            (json!({"usage.model": {"$eq": "b"}}), true),
            (json!({"usage.model": {"$ne": "b"}}), false),
            (json!({"usage.model": {"$in": ["a", "b"]}}), true),
            (json!({"usage.model": {"$nin": ["a", "b"]}}), false),
            (json!({"usage.model": {"$regex": "^b$"}}), true),
        ] {
            assert_eq!(evaluate(Some(&cond), &payload), expected, "{cond}");
        }
    }

    // ── A path that does not exist ──────────────────────────────────

    /// Decision 2. A missing path is null, exactly like a missing top-level key.
    #[test]
    fn a_missing_path_is_null() {
        let cond = json!({"a.b": null});
        assert!(evaluate(Some(&cond), &json!({})));
        assert!(evaluate(Some(&cond), &json!({"a": {}})));
        assert!(evaluate(Some(&cond), &json!({"other": 1})));
    }

    #[test]
    fn a_walk_into_a_scalar_is_null() {
        assert!(evaluate(Some(&json!({"a.b": null})), &json!({"a": 5})));
        assert!(!evaluate(Some(&json!({"a.b": 5})), &json!({"a": 5})));
    }

    #[test]
    fn a_missing_path_does_not_match_a_value() {
        let cond = json!({"workflow_run.event": "schedule"});
        assert!(!evaluate(Some(&cond), &json!({"action": "completed"})));
    }

    /// The corollary the docs lean on: this reads as "exists and is not null".
    #[test]
    fn ne_null_reads_as_exists() {
        let cond = json!({"usage.input_tokens": {"$ne": null}});
        assert!(evaluate(
            Some(&cond),
            &json!({"usage": {"input_tokens": 1}})
        ));
        assert!(!evaluate(Some(&cond), &json!({"usage": {}})));
        assert!(!evaluate(Some(&cond), &json!({})));
    }

    // ── Arrays ──────────────────────────────────────────────────────

    /// Decision 3. A numeric segment is an ordinary object key, never an index.
    #[test]
    fn a_numeric_segment_is_an_object_key() {
        let cond = json!({"a.0": 1});
        assert!(evaluate(Some(&cond), &json!({"a": {"0": 1}})));
    }

    #[test]
    fn an_array_on_the_path_ends_resolution() {
        let cond = json!({"a.0": 1});
        assert!(!evaluate(Some(&cond), &json!({"a": [1, 2]})));
        assert!(evaluate(Some(&json!({"a.0": null})), &json!({"a": [1, 2]})));
    }

    // ── $nin ────────────────────────────────────────────────────────

    #[test]
    fn nin_operator() {
        let cond = json!({"conclusion": {"$nin": ["success", "skipped"]}});
        assert!(evaluate(Some(&cond), &json!({"conclusion": "failure"})));
        assert!(!evaluate(Some(&cond), &json!({"conclusion": "success"})));
    }

    /// Consistent with `$ne`: an absent path is null, and null is in no list.
    #[test]
    fn nin_matches_an_absent_path() {
        let cond = json!({"workflow_run.conclusion": {"$nin": ["success"]}});
        assert!(evaluate(Some(&cond), &json!({})));
    }

    /// A malformed list fails closed, the same way `$in` does.
    #[test]
    fn nin_with_a_non_array_never_matches() {
        let cond = json!({"x": {"$nin": "success"}});
        assert!(!evaluate(Some(&cond), &json!({"x": "failure"})));
    }

    // ── $regex ──────────────────────────────────────────────────────

    #[test]
    fn regex_is_a_substring_search() {
        let cond = json!({"args.command": {"$regex": "cargo test"}});
        let payload = json!({"args": {"command": "cd repo && cargo test --lib"}});
        assert!(evaluate(Some(&cond), &payload));
    }

    #[test]
    fn regex_anchors_explicitly() {
        let payload = json!({"name": "run_bash"});
        assert!(evaluate(
            Some(&json!({"name": {"$regex": "^run"}})),
            &payload
        ));
        assert!(!evaluate(
            Some(&json!({"name": {"$regex": "^bash$"}})),
            &payload
        ));
    }

    #[test]
    fn regex_takes_case_from_the_inline_flag() {
        let payload = json!({"name": "Run_Bash"});
        assert!(!evaluate(
            Some(&json!({"name": {"$regex": "run"}})),
            &payload
        ));
        assert!(evaluate(
            Some(&json!({"name": {"$regex": "(?i)run"}})),
            &payload
        ));
    }

    /// Decision 5. Only a JSON string is text, so everything else is a miss.
    #[test]
    fn regex_never_matches_a_non_string() {
        let cond = json!({"x": {"$regex": "1"}});
        assert!(!evaluate(Some(&cond), &json!({"x": 1})));
        assert!(!evaluate(Some(&cond), &json!({"x": {"y": "1"}})));
        assert!(!evaluate(Some(&cond), &json!({"x": ["1"]})));
        assert!(!evaluate(Some(&cond), &json!({})));
    }

    /// Evaluation never panics on a bad pattern; the write surface refuses it.
    #[test]
    fn an_uncompilable_regex_never_matches() {
        let cond = json!({"x": {"$regex": "["}});
        assert!(!evaluate(Some(&cond), &json!({"x": "["})));
    }

    // ── $or ─────────────────────────────────────────────────────────

    #[test]
    fn or_matches_any_branch() {
        let cond = json!({"$or": [{"conclusion": "failure"}, {"run_attempt": {"$gt": 3}}]});
        assert!(evaluate(Some(&cond), &json!({"conclusion": "failure"})));
        assert!(evaluate(Some(&cond), &json!({"run_attempt": 4})));
        assert!(!evaluate(
            Some(&cond),
            &json!({"conclusion": "success", "run_attempt": 1})
        ));
    }

    /// The point of the operator: `A AND (X OR Y)` inside one entry.
    #[test]
    fn or_ands_with_its_siblings() {
        let cond = json!({
            "action": "completed",
            "$or": [
                {"workflow_run.conclusion": "failure"},
                {"workflow_run.conclusion": "timed_out"},
            ],
        });
        let failed = json!({"action": "completed", "workflow_run": {"conclusion": "failure"}});
        let running = json!({"action": "requested", "workflow_run": {"conclusion": "failure"}});
        assert!(evaluate(Some(&cond), &failed));
        assert!(!evaluate(Some(&cond), &running));
    }

    #[test]
    fn or_branches_are_whole_conditions() {
        let cond = json!({"$or": [{"a": 1, "b": 2}, {"c.d": 3}]});
        assert!(evaluate(Some(&cond), &json!({"a": 1, "b": 2})));
        assert!(!evaluate(Some(&cond), &json!({"a": 1, "b": 9})));
        assert!(evaluate(Some(&cond), &json!({"c": {"d": 3}})));
    }

    #[test]
    fn or_nests() {
        let cond = json!({"$or": [{"$or": [{"a": 1}]}, {"b": 2}]});
        assert!(evaluate(Some(&cond), &json!({"a": 1})));
        assert!(evaluate(Some(&cond), &json!({"b": 2})));
        assert!(!evaluate(Some(&cond), &json!({"c": 3})));
    }

    #[test]
    fn a_malformed_or_never_matches() {
        assert!(!evaluate(Some(&json!({"$or": []})), &json!({"a": 1})));
        assert!(!evaluate(Some(&json!({"$or": "a"})), &json!({"a": 1})));
        assert!(!evaluate(Some(&json!({"$or": [1]})), &json!({"a": 1})));
    }

    // ── The motivating case, end to end ─────────────────────────────

    /// ADR 0119's worked example, on the payload shape that forced it. A GitHub
    /// `workflow_run` delivery carries `action` at the top and the rest under
    /// `workflow_run`, so this whole filter used to need a script.
    #[test]
    fn a_nested_webhook_filter_needs_no_script() {
        let cond = json!({
            "action": "completed",
            "workflow_run.event": "schedule",
            "workflow_run.conclusion": {
                "$nin": ["success", "skipped", "cancelled", "neutral"],
            },
        });
        for (action, event, conclusion, expected) in [
            ("completed", "schedule", "failure", true),
            ("completed", "schedule", "timed_out", true),
            ("completed", "schedule", "startup_failure", true),
            ("completed", "schedule", "success", false),
            ("completed", "schedule", "cancelled", false),
            // A manual run, whatever it concluded. The `event` cut is the half
            // the script used to own.
            ("completed", "workflow_dispatch", "failure", false),
            ("completed", "workflow_dispatch", "success", false),
            // The two deliveries before the run ends.
            ("requested", "schedule", "failure", false),
            ("in_progress", "schedule", "failure", false),
        ] {
            let payload = json!({
                "action": action,
                "workflow_run": {"event": event, "conclusion": conclusion},
            });
            assert_eq!(
                evaluate(Some(&cond), &payload),
                expected,
                "{action} / {event} / {conclusion}"
            );
        }
        assert!(validate(&cond).is_ok());
    }

    /// A run still going carries a null `conclusion`, and `$nin` matches an
    /// absent path. The `action` cut is what keeps that out, so the two clauses
    /// are not redundant.
    #[test]
    fn the_action_cut_carries_the_unfinished_run() {
        let cond = json!({
            "action": "completed",
            "workflow_run.conclusion": {"$nin": ["success"]},
        });
        let running = json!({"action": "in_progress", "workflow_run": {"conclusion": null}});
        let finished = json!({"action": "completed", "workflow_run": {"conclusion": null}});
        assert!(!evaluate(Some(&cond), &running));
        assert!(evaluate(Some(&cond), &finished));
    }

    // ── Caller-supplied data cannot exhaust the stack ───────────────

    #[test]
    fn a_very_long_path_resolves_without_overflowing() {
        let key = vec!["a"; 100_000].join(".");
        let cond = json!({ key: null });
        assert!(evaluate(Some(&cond), &json!({"a": 1})));
    }

    /// **The two depth checks have to agree.** They are separate `>=` tests in
    /// separate functions. A validate that accepted one level deeper than
    /// evaluate matches would hand back the silent dead subscription that the
    /// write-time refusal exists to end.
    #[test]
    fn the_or_depth_boundary_is_the_same_on_both_sides() {
        let nest = |levels: usize| {
            let mut cond = json!({"a": 1});
            for _ in 0..levels {
                cond = json!({ "$or": [cond] });
            }
            cond
        };
        let deepest = nest(MAX_OR_DEPTH);
        assert!(validate(&deepest).is_ok());
        assert!(evaluate(Some(&deepest), &json!({"a": 1})));

        let too_deep = nest(MAX_OR_DEPTH + 1);
        assert!(validate(&too_deep).is_err());
        assert!(!evaluate(Some(&too_deep), &json!({"a": 1})));
    }

    // ── validate ────────────────────────────────────────────────────

    #[test]
    fn validate_accepts_what_evaluate_supports() {
        for cond in [
            json!({}),
            json!({"action": "completed"}),
            json!({"workflow_run.event": "schedule"}),
            json!({"x": {"$gte": 1, "$lt": 9}}),
            json!({"x": {"$in": [1, 2]}, "y": {"$nin": ["a"]}}),
            json!({"x": {"$regex": "(?i)^cargo test"}}),
            json!({"$or": [{"a": 1}, {"b": {"$ne": null}}]}),
        ] {
            assert!(validate(&cond).is_ok(), "{cond}");
        }
    }

    #[test]
    fn validate_refuses_a_non_object_condition() {
        assert!(validate(&json!("not an object")).is_err());
        assert!(validate(&json!([{"a": 1}])).is_err());
    }

    #[test]
    fn validate_refuses_an_unknown_operator() {
        let err = validate(&json!({"x": {"$matches": "a"}})).unwrap_err();
        assert!(err.contains("$matches"), "{err}");
        assert!(err.contains("$regex"), "{err}");
    }

    /// The mistake someone actually makes, so the message names the path.
    #[test]
    fn validate_points_a_nested_object_at_the_field_path() {
        let err = validate(&json!({"workflow_run": {"event": "schedule"}})).unwrap_err();
        assert!(err.contains("workflow_run.event"), "{err}");
    }

    #[test]
    fn validate_refuses_a_reserved_key_in_field_position() {
        let err = validate(&json!({"$and": [{"a": 1}]})).unwrap_err();
        assert!(err.contains("$and"), "{err}");
    }

    #[test]
    fn validate_refuses_a_malformed_operand() {
        assert!(validate(&json!({"x": {"$in": "a"}})).is_err());
        assert!(validate(&json!({"x": {"$nin": "a"}})).is_err());
        assert!(validate(&json!({"x": {"$gt": "a"}})).is_err());
        assert!(validate(&json!({"x": {"$regex": 1}})).is_err());
    }

    #[test]
    fn validate_refuses_an_uncompilable_regex() {
        let err = validate(&json!({"x": {"$regex": "["}})).unwrap_err();
        assert!(err.contains("$regex"), "{err}");
    }

    #[test]
    fn validate_refuses_a_malformed_or() {
        assert!(validate(&json!({"$or": "a"})).is_err());
        assert!(validate(&json!({"$or": []})).is_err());
        assert!(validate(&json!({"$or": [1]})).is_err());
        assert!(validate(&json!({"$or": [{"x": {"$regex": "["}}]})).is_err());
    }

    /// **`OPERATORS`, `evaluate_op` and `validate_op` are three hand-written
    /// copies of one set.** Drift between the last two is the exact failure the
    /// write-time refusal exists to end: an operator `validate` accepts and
    /// `evaluate_op` does not implement arms clean and never matches.
    ///
    /// A new operator therefore has to add its row here, and the row fails
    /// unless both sides carry it.
    #[test]
    fn every_operator_validates_and_matches() {
        let rows: &[(&str, Value, Value)] = &[
            ("$eq", json!({"x": {"$eq": 1}}), json!({"x": 1})),
            ("$ne", json!({"x": {"$ne": 1}}), json!({"x": 2})),
            ("$lt", json!({"x": {"$lt": 2}}), json!({"x": 1})),
            ("$lte", json!({"x": {"$lte": 1}}), json!({"x": 1})),
            ("$gt", json!({"x": {"$gt": 1}}), json!({"x": 2})),
            ("$gte", json!({"x": {"$gte": 1}}), json!({"x": 1})),
            ("$in", json!({"x": {"$in": [1]}}), json!({"x": 1})),
            ("$nin", json!({"x": {"$nin": [2]}}), json!({"x": 1})),
            ("$regex", json!({"x": {"$regex": "a"}}), json!({"x": "a"})),
        ];
        let covered: Vec<&str> = rows.iter().map(|(op, _, _)| *op).collect();
        assert_eq!(covered, OPERATORS, "every operator needs a row here");
        for (op, cond, payload) in rows {
            assert!(validate(cond).is_ok(), "{op} is refused by validate");
            assert!(evaluate(Some(cond), payload), "{op} matches nothing");
        }
    }
}
