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

/// Claude Code's JSON declares no per-model effort restriction, so the
/// transpose must offer every model every tier. The picker then reads one rule
/// on both backends rather than special-casing the absent matrix.
#[test]
fn every_claude_code_model_offers_every_tier() {
    let defs = cc_command_definitions();
    let all: Vec<&str> = cc_reasoning_effort_options()
        .iter()
        .map(|e| e.value.as_str())
        .collect();
    let options = defs.as_array().expect("array")[0]["params"][0]["options"]
        .as_array()
        .expect("options")
        .clone();
    for option in &options {
        let offered: Vec<&str> = option["reasoning_efforts"]
            .as_array()
            .unwrap_or_else(|| panic!("{} must declare its tiers", option["value"]))
            .iter()
            .map(|e| e.as_str().expect("tier"))
            .collect();
        assert_eq!(offered, all, "{} must offer every tier", option["value"]);
    }
}

/// The `major.minor` a pinned model id spells, or `None` for an alias.
///
/// Reads the id, not the label. `Opus (latest, 1M)` carries a digit that is not
/// a version, and `Sonnet (latest)` carries none at all.
///
/// Strips BOTH decorations, which `likely_intended_model` refuses to do. That
/// rule tells two rows apart; this one places them. `claude-opus-5@default` and
/// `claude-opus-5[1m]` are both Opus 5, so they share a slot.
fn pinned_model_version(value: &str) -> Option<(u32, u32)> {
    let id = crate::runtime::strip_version_pin(value);
    let id = id.strip_suffix("[1m]").unwrap_or(&id);
    let (_family, version) = id.strip_prefix("claude-")?.split_once('-')?;
    let mut parts = version.split('-');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((major, minor))
}

/// Where a new model row goes: newest version first.
///
/// Ordering by capability tier is what put Opus 5.5 below Fable 5.1, Fable 5
/// and Sonnet 5. The panel shows about three rows at phone width, so the picker
/// opened on three older models and the newest one needed a scroll.
///
/// Only a pinned id carries a version. `default` heads the list, because that
/// is the row a thread with no pick already sits on. The version-free aliases
/// trail, because there is no version to place them by.
#[test]
fn the_model_rows_run_newest_version_first() {
    let values: Vec<&str> = cc_model_options()
        .iter()
        .map(|m| m.value.as_str())
        .collect();
    assert_eq!(
        values.first(),
        Some(&"default"),
        "the Default row heads the list"
    );

    let mut previous: Option<(u32, u32)> = None;
    let mut alias_seen = false;
    for value in &values[1..] {
        let Some(version) = pinned_model_version(value) else {
            alias_seen = true;
            continue;
        };
        assert!(
            !alias_seen,
            "{value} carries a version, so it belongs above every alias row"
        );
        if let Some(above) = previous {
            assert!(
                version <= above,
                "{value} is newer than the row above it: {version:?} under {above:?}"
            );
        }
        previous = Some(version);
    }
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

fn write_settings(dir: &Path, body: serde_json::Value) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("settings.json"), body.to_string()).unwrap();
}

fn effort_env(value: &str) -> Vec<(String, String)> {
    vec![("CLAUDE_CODE_EFFORT_LEVEL".to_string(), value.to_string())]
}

/// Regression: the label read a value cached at engine startup, so it kept
/// saying `xhigh` after the workspace var changed to `high`.
#[test]
fn default_effort_follows_a_changed_workspace_env_var() {
    let project = tempfile::TempDir::new().unwrap();
    let resolve = |env: &[(String, String)]| {
        CcSettingsScope {
            env,
            inherited: |_| None,
            config_dir: None,
            project_dir: project.path(),
        }
        .default_effort()
    };
    assert_eq!(resolve(&effort_env("xhigh")).as_deref(), Some("xhigh"));
    assert_eq!(resolve(&effort_env("high")).as_deref(), Some("high"));
}

/// The user settings live under the session's `CLAUDE_CONFIG_DIR`, not `$HOME`,
/// and the project settings under its worktree, not the engine's cwd.
#[test]
fn default_effort_reads_the_sessions_own_settings_files() {
    let project = tempfile::TempDir::new().unwrap();
    let config = tempfile::TempDir::new().unwrap();
    write_settings(config.path(), serde_json::json!({ "effortLevel": "low" }));
    let scope = CcSettingsScope {
        env: &[],
        inherited: |_| None,
        config_dir: Some(config.path()),
        project_dir: project.path(),
    };
    assert_eq!(scope.default_effort().as_deref(), Some("low"));

    write_settings(
        &project.path().join(".claude"),
        serde_json::json!({ "effortLevel": "medium" }),
    );
    assert_eq!(
        scope.default_effort().as_deref(),
        Some("medium"),
        "project settings outrank the user's"
    );
    assert_eq!(
        CcSettingsScope {
            env: &effort_env("max"),
            ..scope
        }
        .default_effort()
        .as_deref(),
        Some("max"),
        "the env var outranks every settings file"
    );
}

/// A level from the engine's own launch env reaches CC, so the label reads it
/// too. A workspace var still outranks it, as it does in the spawned env.
#[test]
fn default_effort_reads_what_cc_inherits_below_the_workspace_var() {
    let project = tempfile::TempDir::new().unwrap();
    let mut scope = CcSettingsScope {
        env: &[],
        inherited: |name| (name == "CLAUDE_CODE_EFFORT_LEVEL").then(|| "low".to_string()),
        config_dir: None,
        project_dir: project.path(),
    };
    assert_eq!(scope.default_effort().as_deref(), Some("low"));
    let env = effort_env("high");
    scope.env = &env;
    assert_eq!(scope.default_effort().as_deref(), Some("high"));
}

#[test]
fn default_effort_skips_an_unknown_level() {
    let project = tempfile::TempDir::new().unwrap();
    let config = tempfile::TempDir::new().unwrap();
    write_settings(config.path(), serde_json::json!({ "effortLevel": "high" }));
    let scope = CcSettingsScope {
        env: &effort_env("turbo"),
        inherited: |_| None,
        config_dir: Some(config.path()),
        project_dir: project.path(),
    };
    assert_eq!(scope.default_effort().as_deref(), Some("high"));
}

#[test]
fn default_model_prefers_anthropic_model_over_the_user_settings() {
    let project = tempfile::TempDir::new().unwrap();
    let config = tempfile::TempDir::new().unwrap();
    write_settings(
        config.path(),
        serde_json::json!({ "model": "claude-opus-5-5[1m]" }),
    );
    let mut scope = CcSettingsScope {
        env: &[],
        inherited: |_| None,
        config_dir: Some(config.path()),
        project_dir: project.path(),
    };
    assert_eq!(
        scope.default_model().as_deref(),
        Some("claude-opus-5-5[1m]")
    );

    let env = vec![("ANTHROPIC_MODEL".to_string(), "claude-sonnet-5".to_string())];
    scope.env = &env;
    assert_eq!(scope.default_model().as_deref(), Some("claude-sonnet-5"));
}

/// With no pin, the Init handler reconciles CC's echo against the user's own
/// default, so a `[1m]` they configured survives into the label.
#[test]
fn an_unpinned_session_keeps_the_1m_suffix_of_the_users_default() {
    let project = tempfile::TempDir::new().unwrap();
    let config = tempfile::TempDir::new().unwrap();
    write_settings(
        config.path(),
        serde_json::json!({ "model": "claude-opus-5-5[1m]" }),
    );
    let default_model = CcSettingsScope {
        env: &[],
        inherited: |_| None,
        config_dir: Some(config.path()),
        project_dir: project.path(),
    }
    .default_model();
    assert_eq!(
        reconcile_cc_model(default_model.as_deref(), "claude-opus-5-5"),
        "claude-opus-5-5[1m]"
    );
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
fn every_fable_generation_round_trips_through_cc_model_helpers() {
    // Each Fable id is a full model id present in cc_menu_options.json, so it
    // passes through normalize unchanged, and the 1M variant reconciles like
    // the others. Fable 5.1 is the trap: `claude-fable-5` is a prefix of it,
    // so a prefix-shaped fold would rewrite one generation into the other.
    for base in ["claude-fable-5", "claude-fable-5-1"] {
        let one_m = format!("{base}[1m]");
        assert_eq!(normalize_cc_model_id(base), base);
        assert_eq!(normalize_cc_model_id(&one_m), one_m);
        assert_eq!(reconcile_cc_model(Some(&one_m), base), one_m);
    }
    // The /model picker offers both generations, each with its 1M variant.
    let defs = cc_command_definitions();
    assert_command_options(
        &defs,
        "set_model",
        "model",
        &[
            "claude-fable-5-1",
            "claude-fable-5-1[1m]",
            "claude-fable-5",
            "claude-fable-5[1m]",
        ],
    );
}

#[test]
fn sonnet_5_round_trips_through_cc_model_helpers() {
    // Sonnet 5 is pinned as a full model id in cc_menu_options.json, so CC
    // echoing `claude-sonnet-5` must normalize to itself rather than being
    // rewritten to the `sonnet` alias by the `claude-sonnet-4` rule below it.
    assert_eq!(normalize_cc_model_id("claude-sonnet-5"), "claude-sonnet-5");
    assert_eq!(
        reconcile_cc_model(Some("claude-sonnet-5"), "claude-sonnet-5"),
        "claude-sonnet-5"
    );
    // Picking the `sonnet` alias also lands on Sonnet 5: CC resolves the alias
    // and reports the concrete id, which is a picker value.
    assert_eq!(
        reconcile_cc_model(Some("sonnet"), "claude-sonnet-5"),
        "claude-sonnet-5"
    );
    // Sonnet 4.6 still folds back to the alias (unchanged behaviour).
    assert_eq!(normalize_cc_model_id("claude-sonnet-4-6"), "sonnet");
    // The /model picker offers Sonnet 5 alongside the alias.
    let defs = cc_command_definitions();
    assert_command_options(&defs, "set_model", "model", &["claude-sonnet-5", "sonnet"]);
}

#[test]
fn every_opus_5_generation_round_trips_through_cc_model_helpers() {
    // Opus 5.5 is the trap Fable 5.1 was: `claude-opus-5` is a prefix of
    // `claude-opus-5-5`, so a prefix-shaped fold would rewrite one generation
    // into the other. Both are pinned picker values, so both normalize to
    // themselves and neither collapses onto the `opus` alias.
    for base in ["claude-opus-5-5", "claude-opus-5@default"] {
        assert_eq!(normalize_cc_model_id(base), base);
    }
    for one_m in ["claude-opus-5-5[1m]", "claude-opus-5[1m]"] {
        assert_eq!(normalize_cc_model_id(one_m), one_m);
    }
    // CC strips the suffix when it echoes the model, so reconcile re-attaches it.
    assert_eq!(
        reconcile_cc_model(Some("claude-opus-5-5[1m]"), "claude-opus-5-5"),
        "claude-opus-5-5[1m]"
    );
    // Picking the `opus` alias lands on Opus 5.5: CC resolves the alias and
    // reports the concrete id, which is a picker value.
    assert_eq!(
        reconcile_cc_model(Some("opus"), "claude-opus-5-5"),
        "claude-opus-5-5"
    );
    // Opus 4.6 still folds back to the alias (unchanged behaviour).
    assert_eq!(normalize_cc_model_id("claude-opus-4-6"), "opus");
    // The /model picker offers both generations, each with its 1M variant.
    let defs = cc_command_definitions();
    assert_command_options(
        &defs,
        "set_model",
        "model",
        &[
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-opus-5@default",
            "claude-opus-5[1m]",
            "opus",
        ],
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
