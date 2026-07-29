//! Tests for the plugin update-check notification helpers — notification
//! phrasing and the dedup marker that stops the 5-minute re-scan from
//! re-notifying about updates the user has already seen.

use super::*;
use crate::core::plugin_marketplaces::{MarketplacePlugin, MarketplacePluginStatus};

fn candidate(id: &str, name: &str, version: &str) -> MarketplacePlugin {
    MarketplacePlugin {
        marketplace_id: "mkt".to_string(),
        marketplace_name: "Test Marketplace".to_string(),
        id: id.to_string(),
        name: name.to_string(),
        description: String::new(),
        version: version.to_string(),
        source: format!("https://example.com/{id}"),
        manifest: serde_json::json!({}),
        content: vec!["apps".to_string()],
        categories: vec![],
        files_count: 1,
        status: MarketplacePluginStatus::UpdateAvailable,
        installed_version: Some("0.0.1".to_string()),
        setup_thread_id: None,
        setup_complete: false,
        app_id: Some(id.to_string()),
        modified: false,
        modified_paths: vec![],
    }
}

#[test]
fn single_candidate_uses_singular_phrasing_with_name_and_version() {
    let (title, message) = build_update_notification(&[candidate("weather", "Weather", "1.2.0")]);
    assert_eq!(title, "Plugin update available");
    assert!(message.contains("Weather"), "message: {message}");
    assert!(message.contains("1.2.0"), "message: {message}");
    // Points at Plugins, not the old "Apps" / app store wording.
    assert!(message.contains("Open Plugins to review."), "message: {message}");
    assert!(!message.contains("Apps"), "should not mention Apps: {message}");
}

#[test]
fn multiple_candidates_use_plural_phrasing_with_sorted_names_and_count() {
    let (title, message) = build_update_notification(&[
        candidate("weather", "Weather", "1.2.0"),
        candidate("habit", "Habit Tracker", "2.0.0"),
    ]);
    assert_eq!(title, "Plugin updates available");
    assert!(message.contains('2'), "expected count in: {message}");
    // Names are listed alphabetically regardless of candidate order.
    let habit = message.find("Habit Tracker").expect("habit listed");
    let weather = message.find("Weather").expect("weather listed");
    assert!(habit < weather, "names should be sorted: {message}");
    assert!(message.contains("Open Plugins to review."), "message: {message}");
}

#[test]
fn single_candidate_navigation_focuses_that_plugin_in_the_installed_tab() {
    let nav = build_update_navigation(&[candidate("weather", "Weather", "1.2.0")]);
    assert_eq!(nav.target, NavigateTarget::Plugins);
    assert_eq!(nav.id.as_deref(), Some("weather"));
}

#[test]
fn multiple_candidates_navigation_focuses_alphabetically_first_by_name() {
    // Candidate order is weather-then-habit; the focus is the name-first one
    // (Habit Tracker), matching the plural body's name ordering.
    let nav = build_update_navigation(&[
        candidate("weather", "Weather", "1.2.0"),
        candidate("habit", "Habit Tracker", "2.0.0"),
    ]);
    assert_eq!(nav.target, NavigateTarget::Plugins);
    assert_eq!(nav.id.as_deref(), Some("habit"));
}

#[test]
fn marker_roundtrips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut sig = BTreeSet::new();
    sig.insert("weather@1.2.0".to_string());
    sig.insert("habit@2.0.0".to_string());

    write_notified_signature(dir.path(), &sig);
    let read_back = read_notified_signature(dir.path());

    assert_eq!(read_back, sig);
}

#[test]
fn missing_marker_reads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_notified_signature(dir.path()).is_empty());
}

#[test]
fn new_update_is_detected_against_an_existing_marker() {
    let dir = tempfile::tempdir().unwrap();
    let mut notified = BTreeSet::new();
    notified.insert("weather@1.2.0".to_string());
    write_notified_signature(dir.path(), &notified);

    // A freshly bumped version is "new" relative to what was already notified.
    let current: BTreeSet<String> =
        ["weather@1.2.0".to_string(), "weather@1.3.0".to_string()]
            .into_iter()
            .collect();
    let already = read_notified_signature(dir.path());
    assert!(current.difference(&already).next().is_some());
}

#[test]
fn shrinking_set_after_an_apply_is_not_treated_as_new() {
    let dir = tempfile::tempdir().unwrap();
    let notified: BTreeSet<String> =
        ["weather@1.2.0".to_string(), "habit@2.0.0".to_string()]
            .into_iter()
            .collect();
    write_notified_signature(dir.path(), &notified);

    // User applied "habit"; only "weather" remains — no new entry, so no
    // re-notification.
    let current: BTreeSet<String> = ["weather@1.2.0".to_string()].into_iter().collect();
    let already = read_notified_signature(dir.path());
    assert!(current.difference(&already).next().is_none());
}
