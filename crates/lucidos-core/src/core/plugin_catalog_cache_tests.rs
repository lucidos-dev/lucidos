use super::*;
use crate::core::plugin_marketplaces::{MarketplacePluginStatus, PluginMarketplace};

fn marketplace(id: &str) -> PluginMarketplace {
    PluginMarketplace {
        id: id.to_string(),
        name: format!("{id} marketplace"),
        source: format!("https://example.test/{id}"),
    }
}

fn registry(ids: &[&str]) -> PluginMarketplaceRegistry {
    PluginMarketplaceRegistry {
        marketplaces: ids.iter().map(|id| marketplace(id)).collect(),
    }
}

fn plugin(marketplace_id: &str, id: &str) -> MarketplacePlugin {
    MarketplacePlugin {
        marketplace_id: marketplace_id.to_string(),
        marketplace_name: format!("{marketplace_id} marketplace"),
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        source: format!("https://example.test/{marketplace_id}"),
        manifest: serde_json::json!({}),
        content: vec![],
        categories: vec![],
        files_count: 1,
        status: MarketplacePluginStatus::Available,
        installed_version: None,
        setup_thread_id: None,
        setup_complete: false,
        app_id: None,
        modified: false,
        modified_paths: vec![],
        engine_requirement: None,
        engine_compatible: true,
        engine_incompatible_reason: None,
        media: Default::default(),
    }
}

fn scan_error(marketplace_id: &str) -> MarketplaceScanError {
    MarketplaceScanError {
        marketplace_id: marketplace_id.to_string(),
        marketplace_name: format!("{marketplace_id} marketplace"),
        source: format!("https://example.test/{marketplace_id}"),
        error: "clone failed".to_string(),
    }
}

#[test]
fn the_marketplace_list_comes_from_the_live_registry() {
    let cached = CachedCatalog {
        plugins: vec![plugin("alpha", "one")],
        ..Default::default()
    };
    let mut renamed = registry(&["alpha"]);
    renamed.marketplaces[0].name = "Renamed".to_string();

    let merged = merge_with_registry(&cached, &renamed);

    assert_eq!(merged.marketplaces.len(), 1);
    assert_eq!(merged.marketplaces[0].name, "Renamed");
}

#[test]
fn a_marketplace_no_longer_registered_contributes_nothing() {
    let cached = CachedCatalog {
        plugins: vec![plugin("alpha", "one"), plugin("beta", "two")],
        errors: vec![scan_error("alpha"), scan_error("beta")],
        ..Default::default()
    };

    let merged = merge_with_registry(&cached, &registry(&["alpha"]));

    assert_eq!(
        merged
            .plugins
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        vec!["one"],
    );
    assert_eq!(
        merged
            .errors
            .iter()
            .map(|e| e.marketplace_id.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha"],
    );
}

#[test]
fn a_marketplace_registered_since_the_scan_lists_with_no_plugins() {
    let cached = CachedCatalog {
        plugins: vec![plugin("alpha", "one")],
        ..Default::default()
    };

    let merged = merge_with_registry(&cached, &registry(&["alpha", "beta"]));

    assert_eq!(merged.marketplaces.len(), 2);
    assert!(merged.plugins.iter().all(|p| p.marketplace_id == "alpha"));
}

#[test]
fn a_running_scan_is_one_that_started_inside_the_cutoff() {
    let now = Utc::now();
    assert!(scan_is_running(
        Some(now - Duration::seconds(SCAN_STALL_CUTOFF_SECS - 1)),
        now,
    ));
    assert!(!scan_is_running(None, now));
}

#[test]
fn a_wedged_scan_stops_counting_as_running() {
    let now = Utc::now();
    let started = now - Duration::seconds(SCAN_STALL_CUTOFF_SECS + 1);
    assert!(!scan_is_running(Some(started), now));
}

#[test]
fn a_stamp_from_the_future_reads_as_ancient() {
    let now = Utc::now();
    let ahead = now + Duration::seconds(3600);

    assert!(!scan_is_running(Some(ahead), now));
    assert!(needs_rescan(
        &CachedCatalog {
            scanned_at: Some(ahead),
            ..Default::default()
        },
        now,
    ));
}

#[test]
fn a_workspace_that_never_scanned_needs_one() {
    assert!(needs_rescan(&CachedCatalog::default(), Utc::now()));
}

#[test]
fn a_fresh_cache_needs_no_rescan_and_a_stale_one_does() {
    let now = Utc::now();
    let fresh = CachedCatalog {
        scanned_at: Some(now - Duration::seconds(CACHE_TTL_SECS - 1)),
        ..Default::default()
    };
    let stale = CachedCatalog {
        scanned_at: Some(now - Duration::seconds(CACHE_TTL_SECS + 1)),
        ..Default::default()
    };
    assert!(!needs_rescan(&fresh, now));
    assert!(needs_rescan(&stale, now));
}

#[test]
fn a_running_scan_is_never_doubled_up() {
    let now = Utc::now();
    let scanning = CachedCatalog {
        scanned_at: None,
        scan_started_at: Some(now),
        ..Default::default()
    };
    assert!(!needs_rescan(&scanning, now));
}

#[test]
fn a_missing_cache_reads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let cached = load(dir.path());
    assert!(cached.plugins.is_empty());
    assert!(cached.scanned_at.is_none());
}

#[test]
fn a_truncated_or_invalid_cache_reads_as_empty() {
    for body in ["{\"plugins\": [", "not json at all", ""] {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        assert!(load(dir.path()).plugins.is_empty(), "body: {body}");
    }
}

#[test]
fn a_recorded_scan_reads_back_whole() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc::now();
    let catalog = MarketplaceCatalog {
        marketplaces: registry(&["alpha"]).marketplaces,
        plugins: vec![plugin("alpha", "one")],
        errors: vec![scan_error("alpha")],
    };

    record_scan(dir.path(), &catalog, now);
    let cached = load(dir.path());

    assert_eq!(cached.plugins.len(), 1);
    assert_eq!(cached.plugins[0].id, "one");
    assert_eq!(cached.errors.len(), 1);
    assert_eq!(cached.scanned_at, Some(now));
    assert_eq!(cached.scan_started_at, None);
    assert_eq!(cached.scan_error, None);
}

#[test]
fn the_cache_stores_no_marketplace_list() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = MarketplaceCatalog {
        marketplaces: registry(&["alpha"]).marketplaces,
        plugins: vec![],
        errors: vec![],
    };

    record_scan(dir.path(), &catalog, Utc::now());
    let text = std::fs::read_to_string(cache_path(dir.path())).unwrap();

    assert!(!text.contains("marketplaces"), "cache body: {text}");
}

#[test]
fn a_failed_scan_keeps_the_last_good_plugins() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc::now();
    let catalog = MarketplaceCatalog {
        marketplaces: vec![],
        plugins: vec![plugin("alpha", "one")],
        errors: vec![],
    };
    record_scan(dir.path(), &catalog, now);

    mark_scan_started(dir.path(), now);
    record_scan_failure(dir.path(), "registry unreadable");
    let cached = load(dir.path());

    assert_eq!(cached.plugins.len(), 1);
    assert_eq!(cached.scanned_at, Some(now));
    assert_eq!(cached.scan_started_at, None);
    assert_eq!(cached.scan_error.as_deref(), Some("registry unreadable"));
}

#[test]
fn marking_a_scan_started_keeps_the_cached_plugins() {
    let dir = tempfile::tempdir().unwrap();
    let scanned = Utc::now() - Duration::seconds(60);
    let catalog = MarketplaceCatalog {
        marketplaces: vec![],
        plugins: vec![plugin("alpha", "one")],
        errors: vec![],
    };
    record_scan(dir.path(), &catalog, scanned);

    let started = Utc::now();
    mark_scan_started(dir.path(), started);
    let cached = load(dir.path());

    assert_eq!(cached.plugins.len(), 1);
    assert_eq!(cached.scanned_at, Some(scanned));
    assert_eq!(cached.scan_started_at, Some(started));
}

#[test]
fn a_recorded_scan_survives_a_concurrent_start_stamp() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let catalog = MarketplaceCatalog {
        marketplaces: vec![],
        plugins: vec![plugin("alpha", "one")],
        errors: vec![],
    };

    let stamping = {
        let root = root.clone();
        std::thread::spawn(move || {
            for _ in 0..200 {
                mark_scan_started(&root, Utc::now());
            }
        })
    };
    let base = Utc::now();
    for round in 0..200 {
        let stamped = base + Duration::seconds(round);
        record_scan(&root, &catalog, stamped);
        let seen = load(&root);
        assert_eq!(seen.plugins.len(), 1, "round {round} lost the scanned rows");
        assert!(
            seen.scanned_at.is_some_and(|at| at >= stamped),
            "round {round} rolled scanned_at back to {:?}",
            seen.scanned_at,
        );
    }
    stamping.join().unwrap();
}
