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
fn command_definitions_offer_no_permission_mode() {
    let defs = cc_command_definitions();
    let subtypes: Vec<&str> = defs
        .as_array()
        .expect("array")
        .iter()
        .map(|d| d["subtype"].as_str().expect("subtype"))
        .collect();
    assert_eq!(subtypes, ["set_model", "set_reasoning_effort"]);
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

fn pinned_model_version(value: &str) -> Option<(u32, u32)> {
    let id = crate::runtime::strip_version_pin(value);
    let id = id.strip_suffix("[1m]").unwrap_or(&id);
    let (_family, version) = id.strip_prefix("claude-")?.split_once('-')?;
    let mut parts = version.split('-');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((major, minor))
}

#[test]
fn the_model_rows_run_newest_version_first() {
    let values: Vec<&str> = curated_cc_model_options()
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

fn write_settings(dir: &Path, body: serde_json::Value) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("settings.json"), body.to_string()).unwrap();
}

fn effort_env(value: &str) -> Vec<(String, String)> {
    vec![("CLAUDE_CODE_EFFORT_LEVEL".to_string(), value.to_string())]
}

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
    for base in ["claude-fable-5", "claude-fable-5-1"] {
        let one_m = format!("{base}[1m]");
        assert_eq!(normalize_cc_model_id(base), base);
        assert_eq!(normalize_cc_model_id(&one_m), one_m);
        assert_eq!(reconcile_cc_model(Some(&one_m), base), one_m);
    }
}

#[test]
fn every_sonnet_5_generation_round_trips_through_cc_model_helpers() {
    for id in [
        "claude-sonnet-5-5",
        "claude-sonnet-5",
        "claude-sonnet-5-5[1m]",
    ] {
        assert_eq!(normalize_cc_model_id(id), id);
    }
    assert_eq!(
        reconcile_cc_model(Some("claude-sonnet-5"), "claude-sonnet-5"),
        "claude-sonnet-5"
    );
    assert_eq!(
        reconcile_cc_model(Some("claude-sonnet-5-5[1m]"), "claude-sonnet-5-5"),
        "claude-sonnet-5-5[1m]"
    );
    assert_eq!(
        reconcile_cc_model(Some("sonnet"), "claude-sonnet-5-5"),
        "claude-sonnet-5-5"
    );
    assert_eq!(normalize_cc_model_id("claude-sonnet-4-6"), "sonnet");
    let defs = cc_command_definitions();
    assert_command_options(
        &defs,
        "set_model",
        "model",
        &[
            "claude-sonnet-5-5",
            "claude-sonnet-5-5[1m]",
            "claude-sonnet-5",
            "sonnet",
        ],
    );
}

#[test]
fn every_opus_5_generation_round_trips_through_cc_model_helpers() {
    for base in ["claude-opus-5-5", "claude-opus-5@default"] {
        assert_eq!(normalize_cc_model_id(base), base);
    }
    for one_m in ["claude-opus-5-5[1m]", "claude-opus-5[1m]"] {
        assert_eq!(normalize_cc_model_id(one_m), one_m);
    }
    assert_eq!(
        reconcile_cc_model(Some("claude-opus-5-5[1m]"), "claude-opus-5-5"),
        "claude-opus-5-5[1m]"
    );
    assert_eq!(
        reconcile_cc_model(Some("opus"), "claude-opus-5-5"),
        "claude-opus-5-5"
    );
    assert_eq!(normalize_cc_model_id("claude-opus-4-6"), "opus");
    let defs = cc_command_definitions();
    assert_command_options(
        &defs,
        "set_model",
        "model",
        &["claude-opus-5-5", "claude-opus-5-5[1m]", "opus"],
    );
}

#[test]
fn reconcile_cc_model_preserves_1m_suffix_when_cc_strips_it() {
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

#[test]
fn a_pinned_id_served_as_another_model_is_a_substitution() {
    assert_eq!(
        substituted_cc_model(Some("claude-fable-5-2"), "claude-opus-5-5").as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(
        substituted_cc_model(Some("claude-fable-5-2[1m]"), "claude-opus-5-5").as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(
        substituted_cc_model(Some("claude-opus-5@default"), "claude-sonnet-5-5").as_deref(),
        Some("claude-sonnet-5-5")
    );
}

#[test]
fn an_echo_of_the_requested_model_is_not_a_substitution() {
    for (requested, reported) in [
        ("claude-opus-5-5", "claude-opus-5-5"),
        ("claude-opus-5-5[1m]", "claude-opus-5-5"),
        ("claude-opus-5@default", "claude-opus-5"),
        ("claude-sonnet-4-5", "claude-sonnet-4-5-20250929"),
        ("claude-sonnet-4-5-20250929", "claude-sonnet-4-5"),
    ] {
        assert_eq!(
            substituted_cc_model(Some(requested), reported),
            None,
            "{requested} reported as {reported}"
        );
    }
}

#[test]
fn an_alias_or_no_request_is_never_a_substitution() {
    for requested in ["default", "opus", "opus[1m]", "sonnet", "haiku"] {
        assert_eq!(
            substituted_cc_model(Some(requested), "claude-opus-5-5"),
            None,
            "{requested}"
        );
    }
    assert_eq!(substituted_cc_model(None, "claude-opus-5-5"), None);
    assert_eq!(
        substituted_cc_model(Some("claude-opus-5"), "claude-opus-5-5").as_deref(),
        Some("claude-opus-5-5")
    );
}

#[test]
fn the_offered_list_is_the_discovered_rows_then_the_curated_rest() {
    let offered_curated: Vec<&CcMenuOption> = curated_cc_model_options()
        .iter()
        .filter(|o| o.offered)
        .collect();
    assert_eq!(
        offered_cc_model_options(None).iter().collect::<Vec<_>>(),
        offered_curated
    );
    let discovered = vec![CcMenuOption {
        value: "sonnet".to_string(),
        resolved_model: None,
        label: "Sonnet 5".to_string(),
        description: String::new(),
        supported_models: None,
        context_window: None,
        reasoning_efforts: Some(vec!["low".to_string()]),
        default_effort: None,
        offered: true,
    }];
    let merged = offered_cc_model_options(Some(&discovered));
    assert_eq!(merged[0], discovered[0], "a discovered row leads and wins");
    let expected_rest: Vec<&str> = offered_curated
        .iter()
        .map(|o| o.value.as_str())
        .filter(|v| *v != "sonnet")
        .collect();
    let rest: Vec<&str> = merged[1..].iter().map(|o| o.value.as_str()).collect();
    assert_eq!(rest, expected_rest);
}

#[test]
fn a_discovered_row_offers_its_own_tiers() {
    let row = CcMenuOption {
        value: "haiku".to_string(),
        resolved_model: None,
        label: "Haiku 4.5".to_string(),
        description: String::new(),
        supported_models: None,
        context_window: None,
        reasoning_efforts: Some(Vec::new()),
        default_effort: None,
        offered: true,
    };
    let (models, _) = model_and_effort_options(&[row], cc_reasoning_effort_options());
    assert_eq!(models[0]["reasoning_efforts"], serde_json::json!([]));
}

fn discovered_row(value: &str, runs: &str) -> CcMenuOption {
    CcMenuOption {
        value: value.to_string(),
        resolved_model: Some(runs.to_string()),
        label: value.to_string(),
        description: String::new(),
        supported_models: None,
        context_window: None,
        reasoning_efforts: None,
        default_effort: None,
        offered: true,
    }
}

fn reported_claude_code_list() -> Vec<CcMenuOption> {
    vec![
        discovered_row("default", "claude-opus-5-5[1m]"),
        discovered_row("sonnet", "claude-sonnet-5[1m]"),
        discovered_row("claude-fable-5-1", "claude-fable-5-1"),
        discovered_row("opus", "claude-opus-5-5[1m]"),
        discovered_row("haiku", "claude-haiku-4-5"),
        discovered_row("claude-opus-5-5[1m]", "claude-opus-5-5[1m]"),
    ]
}

fn values(rows: &[CcMenuOption]) -> Vec<&str> {
    rows.iter().map(|r| r.value.as_str()).collect()
}

#[test]
fn the_picker_runs_strongest_first_and_shows_each_model_once() {
    let rows = cc_picker_rows(&reported_claude_code_list());
    assert_eq!(
        values(&rows),
        ["default", "claude-fable-5-1", "opus", "sonnet", "haiku"]
    );
}

#[test]
fn the_fallback_picker_groups_by_family_newest_first() {
    let rows = cc_picker_rows(&offered_cc_model_options(None));
    assert_eq!(
        values(&rows),
        [
            "default",
            "claude-fable-5-2",
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-opus-4-8@default",
            "claude-opus-4-8[1m]",
            "claude-opus-4-7",
            "claude-opus-4-1",
            "opus",
            "opus[1m]",
            "claude-sonnet-5-5",
            "claude-sonnet-5-5[1m]",
            "claude-sonnet-5",
            "sonnet",
            "claude-haiku-5-5",
            "haiku",
        ]
    );
}

#[test]
fn the_served_picker_names_the_model_each_row_runs() {
    let rows = cc_picker_rows(&reported_claude_code_list());
    let (served, _) = model_and_effort_options(&rows, cc_reasoning_effort_options());
    assert_eq!(served[2]["value"], "opus");
    assert_eq!(served[2]["resolved_model"], "claude-opus-5-5[1m]");
}

#[test]
fn fable_52_declares_its_one_million_window() {
    assert_eq!(
        crate::runtime::coding_agent_context_window(
            crate::runtime::CodingAgent::ClaudeCode,
            "claude-fable-5-2"
        ),
        Some(1_000_000)
    );
}
