//! Condition evaluator for event subscriptions.
//!
//! Evaluates a JSON condition against an event payload.
//! Supports operators: $eq, $ne, $lt, $lte, $gt, $gte, $in.
//! A bare value (no operator) is treated as $eq.
//!
//! **One predicate language, two consumers.** This lived under `triggers/` when
//! a trigger was the only thing that could subscribe to an event. A thread's
//! *event wait* subscribes with the same shape, so the evaluator moved here
//! rather than being duplicated: a `condition` that matches for a trigger must
//! match for a wait, and the only way to guarantee that is one function. See
//! [`super::EventSubscription::matches`], which is the sole caller both
//! dispatch paths go through.

use serde_json::Value;

/// Evaluate a condition against an event payload.
/// Returns true if the payload matches (or condition is None).
pub fn evaluate(condition: Option<&Value>, payload: &Value) -> bool {
    let condition = match condition {
        None => return true,
        Some(c) => c,
    };

    match condition.as_object() {
        None => false,
        Some(fields) => fields.iter().all(|(key, op)| {
            let actual = &payload[key];
            evaluate_op(op, actual)
        }),
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
                "$in" => expected
                    .as_array()
                    .map(|arr| arr.contains(actual))
                    .unwrap_or(false),
                _ => false,
            }),
        None => actual == op, // bare value = equality check
    }
}

fn compare(a: &Value, b: &Value, cmp: fn(f64, f64) -> bool) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => cmp(a, b),
        _ => false,
    }
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

    #[test]
    fn unknown_operator_returns_false() {
        let cond = json!({"score": {"$regex": ".*"}});
        assert!(!evaluate(Some(&cond), &json!({"score": "hello"})));
    }

    #[test]
    fn compare_non_numeric_returns_false() {
        let cond = json!({"name": {"$lt": "foo"}});
        assert!(!evaluate(Some(&cond), &json!({"name": "bar"})));
    }
}
