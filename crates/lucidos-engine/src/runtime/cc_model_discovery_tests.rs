use super::*;

/// The shape Claude Code 2.1.280 returned, trimmed to three rows, with a fake
/// account where the real reply names the signed-in user.
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

fn offers(cache: Option<&CcModelCache>, refusals: &CcModelRefusals, value: &str) -> bool {
    offered_list(cache, refusals)
        .iter()
        .any(|o| o.value == value)
}

/// What the engine's picker offers right now.
fn picker_offers(state: &Discovery, value: &str) -> bool {
    state
        .options
        .as_deref()
        .is_none_or(|options| options.iter().any(|o| o.value == value))
}

/// A session on a just-updated Claude Code refuses before the probe of that
/// version lands. The refusal carries the session's version, so that probe
/// keeps it, whatever version the cache it replaces came from.
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
    assert!(!picker_offers(&state, "claude-fable-5-2"));
    assert!(!state.install(cache_at(Utc::now(), Some("2.1.312"))));
    assert!(!picker_offers(&state, "claude-fable-5-2"));
}

/// A probe of another version drops the refusals, and so does one whose
/// version is unknown: an update is when the account's list can move.
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
        assert!(picker_offers(&state, "claude-fable-5-2"), "{probed:?}");
    }
    let undated = refused_by(None, "claude-fable-5-2");
    assert!(!undated.hold_for(None));
}

/// The probe runs under the live account, so only a session on it changes the
/// picker. A session pinned to another account refuses for itself alone.
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
    assert!(picker_offers(&state, "claude-fable-5-2"));
    assert!(state.refuse(
        "claude-fable-5-2",
        Some("2.1.311"),
        Some(&live),
        Some(&live)
    ));
    assert!(!picker_offers(&state, "claude-fable-5-2"));
}

/// A switch of the live account drops the old account's refusals, so the new
/// account is offered the models it may run.
#[test]
fn a_new_live_account_drops_the_refusals() {
    let (a, b) = (account("a"), account("b"));
    let mut state = Discovery::new();
    assert!(state.refuse("claude-fable-5-2", Some("2.1.311"), Some(&a), Some(&a)));
    assert!(!state.observe_live_account(Some(&a)));
    assert!(!picker_offers(&state, "claude-fable-5-2"));
    assert!(state.observe_live_account(Some(&b)));
    assert!(picker_offers(&state, "claude-fable-5-2"));
}

/// A refusal from another version replaces the older ones rather than adding
/// to them, and a known refusal records nothing new.
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

/// With no probe result the picker is the curated fallback, and a refusal
/// still takes its row off: a workspace whose probe fails learns too.
#[test]
fn a_refusal_drops_the_curated_row_from_the_fallback() {
    let none = CcModelRefusals::default();
    assert!(offers(None, &none, "claude-fable-5-2"));
    let refusals = refused_by(Some("2.1.311"), "claude-fable-5-2");
    assert!(!offers(None, &refusals, "claude-fable-5-2"));
}

/// A refusal drops the curated row from a discovered list, and survives a
/// restart through its file. A discovered row is never dropped: Claude Code
/// listing it says it may run.
#[test]
fn a_refusal_drops_the_curated_row_and_round_trips() {
    let cache = cache_at(Utc::now(), Some("2.1.311"));
    let mut refusals = refused_by(Some("2.1.311"), "claude-fable-5-2");
    assert!(offers(
        Some(&cache),
        &CcModelRefusals::default(),
        "claude-fable-5-2"
    ));
    assert!(!offers(Some(&cache), &refusals, "claude-fable-5-2"));
    assert!(refusals.refuse("sonnet", Some("2.1.311"), Some(&account("a"))));
    assert!(
        offers(Some(&cache), &refusals, "sonnet"),
        "a discovered row stays offered"
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".lucidos")).unwrap();
    assert_eq!(load_refusals(dir.path()), CcModelRefusals::default());
    save_refusals(dir.path(), &refusals);
    assert_eq!(load_refusals(dir.path()), refusals);
    std::fs::write(refusals_path(dir.path()), b"{ not json").unwrap();
    assert_eq!(load_refusals(dir.path()), CcModelRefusals::default());
}

/// The reply names the signed-in account. Nothing of it may reach the cache:
/// its `account` field is the engine's own account pin, never the reply's.
/// The timestamp is fixed: a live one carries random digits that can spell a
/// probe string.
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
        Some("2.1.280"),
        None
    ));
    let old = cache_at(now - CACHE_TTL - chrono::Duration::minutes(1), None);
    assert!(needs_refresh(Some(&old), None, now, Some("2.1.280"), None));
}

/// Another account offers other models, so a switch of account re-probes. A
/// cache written before the engine recorded the account re-probes once.
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

/// A failed probe holds off the next one, so a lasting failure costs one cold
/// Claude Code an hour rather than one per session start.
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
