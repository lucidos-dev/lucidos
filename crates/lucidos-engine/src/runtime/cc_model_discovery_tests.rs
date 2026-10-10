use super::*;

/// The shape Claude Code 2.1.280 returned, trimmed to three rows, with a fake
/// account where the real reply names the signed-in user.
fn recorded_reply() -> serde_json::Value {
    serde_json::json!({
        "account": { "email": "someone@example.com", "organization": "Example Org" },
        "pid": 4242,
        "commands": [{ "name": "compact" }],
        "models": [
            {
                "value": "default",
                "resolvedModel": "claude-opus-5-5[1m]",
                "displayName": "Default",
                "description": "Use the default model (currently Opus 5.5 (1M context))",
                "supportsEffort": true,
                "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"],
                "supportsAdaptiveThinking": true
            },
            {
                "value": "sonnet",
                "resolvedModel": "claude-sonnet-5[1m]",
                "displayName": "Sonnet 5",
                "description": "Custom Sonnet model (claude-sonnet-5[1m])",
                "supportsEffort": true,
                "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]
            },
            {
                "value": "haiku",
                "resolvedModel": "claude-haiku-4-5",
                "displayName": "Haiku 4.5",
                "description": "Custom Haiku model (claude-haiku-4-5)"
            }
        ]
    })
}

fn cache_at(discovered_at: DateTime<Utc>, cc_version: Option<&str>) -> CcModelCache {
    CcModelCache {
        cc_version: cc_version.map(str::to_string),
        discovered_at,
        models: parse_initialize_models(&recorded_reply()).unwrap(),
    }
}

/// The reply names the signed-in account. Nothing of it may reach the cache.
#[test]
fn only_the_model_list_reaches_the_cache() {
    let cache = cache_at(Utc::now(), Some("2.1.280"));
    let bytes = serde_json::to_string(&cache).unwrap();
    for leaked in [
        "someone@example.com",
        "Example Org",
        "account",
        "4242",
        "compact",
    ] {
        assert!(
            !bytes.contains(leaked),
            "{leaked} reached the cache: {bytes}"
        );
    }
    assert_eq!(cache.models.len(), 3);
    assert_eq!(cache.models[1].value, "sonnet");
    assert_eq!(
        cache.models[1].resolved_model.as_deref(),
        Some("claude-sonnet-5[1m]")
    );
}

/// A reply with no usable list is an error, so a broken probe never empties
/// the picker.
#[test]
fn a_reply_without_models_is_refused() {
    let missing = parse_initialize_models(&serde_json::json!({ "account": {} }));
    assert!(missing.unwrap_err().contains("no models field"));
    let empty = parse_initialize_models(&serde_json::json!({ "models": [] }));
    assert!(empty.unwrap_err().contains("listed no models"));
    let malformed = parse_initialize_models(&serde_json::json!({ "models": [{ "value": 1 }] }));
    assert!(malformed.unwrap_err().contains("did not parse"));
}

/// The picker shows Claude Code's own labels, in its order, and a model that
/// takes no effort offers no tiers.
#[test]
fn discovered_rows_keep_claude_codes_labels_order_and_tiers() {
    let options = menu_options(&parse_initialize_models(&recorded_reply()).unwrap());
    let values: Vec<&str> = options.iter().map(|o| o.value.as_str()).collect();
    assert_eq!(values, ["default", "sonnet", "haiku"]);
    assert_eq!(options[1].label, "Sonnet 5");
    assert_eq!(
        options[0].reasoning_efforts.as_deref(),
        Some(&["low", "medium", "high", "xhigh", "max"].map(String::from)[..])
    );
    assert_eq!(options[2].reasoning_efforts.as_deref(), Some(&[][..]));
    assert!(options.iter().all(|o| o.context_window.is_none()));
}

#[test]
fn a_cache_is_refreshed_when_missing_stale_or_from_another_version() {
    let now = Utc::now();
    assert!(needs_refresh(None, None, now, None));
    let fresh = cache_at(now - chrono::Duration::hours(1), Some("2.1.280"));
    assert!(!needs_refresh(Some(&fresh), None, now, None));
    assert!(!needs_refresh(Some(&fresh), None, now, Some("2.1.280")));
    assert!(needs_refresh(Some(&fresh), None, now, Some("2.1.281")));
    let stale = cache_at(
        now - CACHE_TTL - chrono::Duration::minutes(1),
        Some("2.1.280"),
    );
    assert!(needs_refresh(Some(&stale), None, now, None));
}

/// A probe whose `--version` did not answer has nothing to compare, so every
/// session start reporting a version must not re-probe.
#[test]
fn an_unversioned_cache_refreshes_on_age_only() {
    let now = Utc::now();
    let unversioned = cache_at(now, None);
    assert!(!needs_refresh(
        Some(&unversioned),
        None,
        now,
        Some("2.1.280")
    ));
    let old = cache_at(now - CACHE_TTL - chrono::Duration::minutes(1), None);
    assert!(needs_refresh(Some(&old), None, now, Some("2.1.280")));
}

/// A failed probe holds off the next one, so a lasting failure costs one cold
/// Claude Code an hour rather than one per session start.
#[test]
fn a_failed_probe_waits_before_the_next() {
    let now = Utc::now();
    let just_failed = Some(now - chrono::Duration::minutes(5));
    assert!(!needs_refresh(None, just_failed, now, None));
    assert!(!needs_refresh(None, just_failed, now, Some("2.1.281")));
    let failed_long_ago = Some(now - RETRY_AFTER_FAILURE - chrono::Duration::minutes(1));
    assert!(needs_refresh(None, failed_long_ago, now, None));
}

/// A model that supports effort but names no tiers falls back to the effort
/// table, rather than offering none.
#[test]
fn effort_support_without_named_tiers_defers_to_the_table() {
    let unnamed = DiscoveredModel {
        value: "opus".to_string(),
        resolved_model: None,
        display_name: "Opus".to_string(),
        description: String::new(),
        supports_effort: Some(true),
        supported_effort_levels: None,
    };
    assert_eq!(menu_options(&[unnamed])[0].reasoning_efforts, None);
}

#[test]
fn the_cache_round_trips_through_its_file() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(load_cache(dir.path()), None);
    let cache = cache_at(Utc::now(), Some("2.1.280"));
    save_cache(dir.path(), &cache).unwrap();
    assert_eq!(load_cache(dir.path()), Some(cache));
}

/// An unreadable file is treated as absent, so the next probe replaces it.
#[test]
fn an_unreadable_cache_reads_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".lucidos")).unwrap();
    std::fs::write(cache_path(dir.path()), b"{ not json").unwrap();
    assert_eq!(load_cache(dir.path()), None);
}
