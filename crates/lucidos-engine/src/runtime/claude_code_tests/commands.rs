use super::*;

#[test]
fn cc_control_request_interrupt_serializes() {
    let json = cc_control_request_to_json(&ControlRequest::Interrupt, "test-id-123");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "control_request");
    assert_eq!(parsed["request_id"], "test-id-123");
    assert_eq!(parsed["request"]["subtype"], "interrupt");
}

#[test]
fn cc_control_request_set_model_serializes() {
    let json = cc_control_request_to_json(
        &ControlRequest::SetModel {
            model: "claude-sonnet-4-6".to_string(),
        },
        "test-id-456",
    );
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "control_request");
    assert_eq!(parsed["request"]["subtype"], "set_model");
    assert_eq!(parsed["request"]["model"], "claude-sonnet-4-6");
}

#[test]
fn cc_control_request_set_permission_mode_serializes() {
    let json = cc_control_request_to_json(
        &ControlRequest::SetPermissionMode {
            mode: "plan".to_string(),
        },
        "test-id-789",
    );
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["request"]["subtype"], "set_permission_mode");
    assert_eq!(parsed["request"]["mode"], "plan");
}

fn assert_command_options(
    defs: &serde_json::Value,
    subtype: &str,
    key: &str,
    expected_values: &[&str],
) {
    let cmd = defs
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["subtype"] == subtype)
        .unwrap_or_else(|| panic!("{} command should exist", subtype));
    let param = &cmd["params"][0];
    assert_eq!(param["key"], key);
    let options = param["options"]
        .as_array()
        .unwrap_or_else(|| panic!("{} param should have options", key));
    assert!(
        options.len() >= expected_values.len(),
        "{}: expected at least {} options, got {}",
        subtype,
        expected_values.len(),
        options.len()
    );
    for opt in options {
        assert!(opt["value"].is_string(), "option missing value");
        assert!(opt["label"].is_string(), "option missing label");
        assert!(opt["description"].is_string(), "option missing description");
    }
    let values: Vec<&str> = options
        .iter()
        .map(|o| o["value"].as_str().unwrap())
        .collect();
    for ev in expected_values {
        assert!(values.contains(ev), "{}: missing {} option", subtype, ev);
    }
}

#[test]
fn command_definitions_include_model_options() {
    let defs = cc_command_definitions();
    assert_command_options(
        &defs,
        "set_model",
        "model",
        &["default", "sonnet", "opus", "haiku"],
    );
}

#[test]
fn command_definitions_include_reasoning_effort_options() {
    let defs = cc_command_definitions();
    assert_command_options(
        &defs,
        "set_reasoning_effort",
        "effort",
        &["low", "medium", "high", "xhigh", "max"],
    );
}

#[test]
fn control_request_deserializes_all_variants() {
    let cases = vec![
        (r#"{"subtype":"interrupt"}"#, "interrupt"),
        (
            r#"{"subtype":"set_model","model":"claude-sonnet-4-6"}"#,
            "set_model",
        ),
        (
            r#"{"subtype":"set_permission_mode","mode":"plan"}"#,
            "set_permission_mode",
        ),
        (
            r#"{"subtype":"set_reasoning_effort","effort":"high"}"#,
            "set_reasoning_effort",
        ),
    ];
    for (json, expected_subtype) in cases {
        let req: ControlRequest = serde_json::from_str(json).unwrap();
        let serialized = cc_control_request_to_json(&req, "test-id");
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            parsed["request"]["subtype"], expected_subtype,
            "Failed for: {}",
            json
        );
    }
}

#[test]
fn read_cc_default_effort_reads_settings() {
    let result = read_cc_default_effort();
    if let Some(ref v) = result {
        assert!(is_valid_effort(v), "Unexpected effort level: {}", v);
    }
}

#[test]
fn normalize_cc_model_id_maps_aliases() {
    assert_eq!(normalize_cc_model_id("sonnet"), "sonnet");
    assert_eq!(normalize_cc_model_id("opus"), "opus");
    assert_eq!(normalize_cc_model_id("haiku"), "haiku");
    assert_eq!(normalize_cc_model_id("claude-opus-4-7"), "claude-opus-4-7");
    assert_eq!(normalize_cc_model_id("claude-opus-4-1"), "claude-opus-4-1");
}

#[test]
fn normalize_cc_model_id_maps_full_ids() {
    assert_eq!(normalize_cc_model_id("claude-sonnet-4-6"), "sonnet");
    assert_eq!(normalize_cc_model_id("claude-sonnet-4-20250514"), "sonnet");
    assert_eq!(normalize_cc_model_id("claude-opus-4-6"), "opus");
    assert_eq!(normalize_cc_model_id("claude-haiku-4-5-20251001"), "haiku");
    assert_eq!(normalize_cc_model_id("claude-haiku-4-5@20251001"), "haiku");
    assert_eq!(normalize_cc_model_id("claude-haiku-4-5"), "haiku");
}

#[test]
fn normalize_cc_model_id_preserves_unknown() {
    assert_eq!(normalize_cc_model_id("gpt-4o"), "gpt-4o");
    assert_eq!(normalize_cc_model_id("custom-model"), "custom-model");
}

#[test]
fn fable_5_round_trips_through_cc_model_helpers() {
    // Fable 5 is a full model id present in cc_menu_options.json, so it passes
    // through normalize unchanged, and the 1M variant reconciles like the others.
    assert_eq!(normalize_cc_model_id("claude-fable-5"), "claude-fable-5");
    assert_eq!(
        normalize_cc_model_id("claude-fable-5[1m]"),
        "claude-fable-5[1m]"
    );
    assert_eq!(
        reconcile_cc_model(Some("claude-fable-5[1m]"), "claude-fable-5"),
        "claude-fable-5[1m]"
    );
    // The /model picker offers Fable 5 (and its 1M variant).
    let defs = cc_command_definitions();
    assert_command_options(
        &defs,
        "set_model",
        "model",
        &["claude-fable-5", "claude-fable-5[1m]"],
    );
}

#[test]
fn reconcile_cc_model_preserves_1m_suffix_when_cc_strips_it() {
    // CC strips the [1m] suffix when echoing the model in stream-json
    // (both Init and per-message Usage frames). The engine pinned the
    // 1M-context variant when invoking CC, so the reconciled name must
    // keep the [1m] marker — context_window_for needs it to return 1M.
    assert_eq!(
        reconcile_cc_model(Some("claude-opus-4-7[1m]"), "claude-opus-4-7"),
        "claude-opus-4-7[1m]"
    );
    assert_eq!(
        reconcile_cc_model(Some("opus[1m]"), "claude-opus-4-6"),
        "opus[1m]"
    );
    assert_eq!(
        reconcile_cc_model(Some("sonnet[1m]"), "claude-sonnet-4-6"),
        "sonnet[1m]"
    );
}

#[test]
fn reconcile_cc_model_drops_1m_when_user_switched_models() {
    // /model in CC can swap the active model mid-session. If the new model
    // doesn't share a base with the original [1m] alias, don't fabricate
    // a [1m] suffix on it.
    assert_eq!(
        reconcile_cc_model(Some("claude-opus-4-7[1m]"), "claude-sonnet-4-6"),
        "sonnet"
    );
    assert_eq!(
        reconcile_cc_model(Some("opus[1m]"), "claude-haiku-4-5"),
        "haiku"
    );
}

#[test]
fn reconcile_cc_model_passes_through_when_no_1m() {
    // No suffix on the original alias → behave exactly like normalize.
    assert_eq!(
        reconcile_cc_model(Some("claude-opus-4-7"), "claude-opus-4-7"),
        "claude-opus-4-7"
    );
    assert_eq!(
        reconcile_cc_model(Some("sonnet"), "claude-sonnet-4-6"),
        "sonnet"
    );
    assert_eq!(
        reconcile_cc_model(None, "claude-opus-4-7"),
        "claude-opus-4-7"
    );
}
