use super::*;

fn recorded_reply() -> serde_json::Value {
    serde_json::json!({
        "account": { "email": "someone@example.com", "organization": "Example Org" },
        "pid": 918273645,
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
        account: Some(account("a")),
        discovered_at,
        models: parse_initialize_models(&recorded_reply()).unwrap(),
    }
}

fn refused_by(cc_version: Option<&str>, model: &str) -> CcModelRefusals {
    let mut refusals = CcModelRefusals::default();
    assert!(refusals.refuse(model, cc_version, Some(&account("a"))));
    refusals
}

fn account(name: &str) -> AccountPin {
    AccountPin::ExplicitConfigDir {
        dir: format!("/home/user/.claude-{name}"),
    }
}

fn picker_offers(state: &Discovery, value: &str) -> bool {
    state
        .options
        .as_deref()
        .is_none_or(|options| options.iter().any(|o| o.value == value))
}

#[test]
fn a_refusal_on_a_new_version_survives_that_versions_probe() {
    let live = account("a");
    let mut state = Discovery::new();
    assert!(!state.install(cache_at(Utc::now(), Some("2.1.311"))));
    assert!(state.refuse(
        "claude-fable-5-2",
        Some("2.1.312"),
        Some(&live),
        Some(&live)
    ));
    assert!(state.refusals.holds("claude-fable-5-2"));
    assert!(!state.install(cache_at(Utc::now(), Some("2.1.312"))));
    assert!(state.refusals.holds("claude-fable-5-2"));
}

#[test]
fn a_probe_of_another_or_unknown_version_drops_the_refusals() {
    let live = account("a");
    for probed in [Some("2.1.313"), None] {
        let mut state = Discovery::new();
        assert!(state.refuse(
            "claude-fable-5-2",
            Some("2.1.312"),
            Some(&live),
            Some(&live)
        ));
        assert!(state.install(cache_at(Utc::now(), probed)), "{probed:?}");
        assert!(!state.refusals.holds("claude-fable-5-2"), "{probed:?}");
    }
    let undated = refused_by(None, "claude-fable-5-2");
    assert!(!undated.hold_for(None));
}

#[test]
fn only_a_session_on_the_live_account_learns() {
    let (live, other) = (account("a"), account("b"));
    let mut state = Discovery::new();
    assert!(!state.refuse(
        "claude-fable-5-2",
        Some("2.1.311"),
        Some(&other),
        Some(&live)
    ));
    assert!(!state.refusals.holds("claude-fable-5-2"));
    assert!(state.refuse(
        "claude-fable-5-2",
        Some("2.1.311"),
        Some(&live),
        Some(&live)
    ));
    assert!(state.refusals.holds("claude-fable-5-2[1m]"));
}

#[test]
fn a_new_live_account_drops_the_refusals() {
    let (a, b) = (account("a"), account("b"));
    let mut state = Discovery::new();
    assert!(state.refuse("claude-fable-5-2", Some("2.1.311"), Some(&a), Some(&a)));
    assert!(!state.observe_live_account(Some(&a)));
    assert!(state.refusals.holds("claude-fable-5-2"));
    assert!(state.observe_live_account(Some(&b)));
    assert!(!state.refusals.holds("claude-fable-5-2"));
}

#[test]
fn a_refusal_from_another_version_starts_over() {
    let a = account("a");
    let mut refusals = refused_by(Some("2.1.311"), "claude-fable-5-2[1m]");
    assert_eq!(refusals.models, ["claude-fable-5-2"]);
    assert!(!refusals.refuse("claude-fable-5-2", Some("2.1.311"), Some(&a)));
    assert!(refusals.refuse("claude-opus-5-5", Some("2.1.312"), Some(&a)));
    assert_eq!(refusals.models, ["claude-opus-5-5"]);
    assert_eq!(refusals.cc_version.as_deref(), Some("2.1.312"));
}

#[test]
fn a_refusal_keeps_the_model_offered() {
    let live = account("a");
    let mut state = Discovery::new();
    assert!(state.refuse(
        "claude-fable-5-2",
        Some("2.1.311"),
        Some(&live),
        Some(&live)
    ));
    assert!(
        state.options.is_none(),
        "the curated fallback still applies"
    );
    assert!(!state.install(cache_at(Utc::now(), Some("2.1.311"))));
    assert!(state.refusals.holds("claude-fable-5-2"));
    assert!(picker_offers(&state, "claude-fable-5-2"));
}

#[test]
fn a_clean_serve_on_the_live_account_clears_the_refusal() {
    let (live, other) = (account("a"), account("b"));
    let mut state = Discovery::new();
    assert!(state.refuse(
        "claude-fable-5-2",
        Some("2.1.311"),
        Some(&live),
        Some(&live)
    ));
    assert!(!state.serve("claude-fable-5-2", Some(&other), Some(&live)));
    assert!(state.refusals.holds("claude-fable-5-2"));
    assert!(state.serve("claude-fable-5-2[1m]", Some(&live), Some(&live)));
    assert!(!state.refusals.holds("claude-fable-5-2"));
    assert!(!state.serve("claude-fable-5-2", Some(&live), Some(&live)));
}

#[test]
fn the_refusals_round_trip_through_their_file() {
    let refusals = refused_by(Some("2.1.311"), "claude-fable-5-2");
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".lucidos")).unwrap();
    assert_eq!(load_refusals(dir.path()), CcModelRefusals::default());
    save_refusals(dir.path(), &refusals);
    assert_eq!(load_refusals(dir.path()), refusals);
    std::fs::write(refusals_path(dir.path()), b"{ not json").unwrap();
    assert_eq!(load_refusals(dir.path()), CcModelRefusals::default());
}

#[test]
fn a_told_refusal_stays_told_until_it_clears() {
    let mut refusals = refused_by(Some("2.1.311"), "claude-fable-5-2");
    assert!(refusals.untold("claude-fable-5-2[1m]"));
    assert!(refusals.tell("claude-fable-5-2"));
    assert!(!refusals.untold("claude-fable-5-2"));
    assert!(!refusals.tell("claude-fable-5-2"), "told once");
    assert!(!refusals.untold("claude-opus-5-5"), "never refused");

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".lucidos")).unwrap();
    save_refusals(dir.path(), &refusals);
    assert_eq!(load_refusals(dir.path()), refusals);

    assert!(refusals.forget("claude-fable-5-2"));
    assert!(refusals.refuse("claude-fable-5-2", Some("2.1.311"), Some(&account("a"))));
    assert!(refusals.untold("claude-fable-5-2"));
}

#[test]
fn only_the_model_list_reaches_the_cache() {
    let discovered_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let cache = cache_at(discovered_at, Some("2.1.280"));
    assert_eq!(cache.account, Some(account("a")));
    let bytes = serde_json::to_string(&cache).unwrap();
    for leaked in [
        "someone@example.com",
        "Example Org",
        "email",
        "organization",
        "918273645",
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

#[test]
fn a_reply_without_models_is_refused() {
    let missing = parse_initialize_models(&serde_json::json!({ "account": {} }));
    assert!(missing.unwrap_err().contains("no models field"));
    let empty = parse_initialize_models(&serde_json::json!({ "models": [] }));
    assert!(empty.unwrap_err().contains("listed no models"));
    let malformed = parse_initialize_models(&serde_json::json!({ "models": [{ "value": 1 }] }));
    assert!(malformed.unwrap_err().contains("did not parse"));
}

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
    assert!(needs_refresh(None, None, now, None, None));
    let fresh = cache_at(now - chrono::Duration::hours(1), Some("2.1.280"));
    assert!(!needs_refresh(Some(&fresh), None, now, None, None));
    assert!(!needs_refresh(
        Some(&fresh),
        None,
        now,
        Some("2.1.280"),
        None
    ));
    assert!(needs_refresh(
        Some(&fresh),
        None,
        now,
        Some("2.1.281"),
        None
    ));
    let stale = cache_at(
        now - CACHE_TTL - chrono::Duration::minutes(1),
        Some("2.1.280"),
    );
    assert!(needs_refresh(Some(&stale), None, now, None, None));
}

#[test]
fn an_unversioned_cache_refreshes_on_age_only() {
    let now = Utc::now();
    let unversioned = cache_at(now, None);
    assert!(!needs_refresh(
        Some(&unversioned),
        None,
        now,
        Some("2.1.280"),
        None
    ));
    let old = cache_at(now - CACHE_TTL - chrono::Duration::minutes(1), None);
    assert!(needs_refresh(Some(&old), None, now, Some("2.1.280"), None));
}

#[test]
fn a_cache_from_another_account_is_refreshed() {
    let now = Utc::now();
    let fresh = cache_at(now, Some("2.1.280"));
    let probed_under = account("a");
    let live = Some(&probed_under);
    assert!(!needs_refresh(
        Some(&fresh),
        None,
        now,
        Some("2.1.280"),
        live
    ));
    let switched = account("b");
    assert!(needs_refresh(
        Some(&fresh),
        None,
        now,
        Some("2.1.280"),
        Some(&switched)
    ));
    let unrecorded = CcModelCache {
        account: None,
        ..fresh
    };
    assert!(needs_refresh(Some(&unrecorded), None, now, None, live));
}

#[test]
fn a_failed_probe_waits_before_the_next() {
    let now = Utc::now();
    let just_failed = Some(now - chrono::Duration::minutes(5));
    assert!(!needs_refresh(None, just_failed, now, None, None));
    assert!(!needs_refresh(
        None,
        just_failed,
        now,
        Some("2.1.281"),
        None
    ));
    let failed_long_ago = Some(now - RETRY_AFTER_FAILURE - chrono::Duration::minutes(1));
    assert!(needs_refresh(None, failed_long_ago, now, None, None));
}

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

#[test]
fn an_unreadable_cache_reads_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".lucidos")).unwrap();
    std::fs::write(cache_path(dir.path()), b"{ not json").unwrap();
    assert_eq!(load_cache(dir.path()), None);
}
