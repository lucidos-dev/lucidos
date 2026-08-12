//! `GET /api/v1/engine/changelog`: the notes behind Settings > System > What's
//! New.

use crate::support::{base_url, http_client};

#[tokio::test]
async fn changelog_returns_every_release_newest_first() {
    let client = http_client();
    let url = format!("{}/api/v1/engine/changelog", base_url());

    let resp = client
        .get(&url)
        .send()
        .await
        .expect("Changelog request failed");
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    let releases = body["releases"]
        .as_array()
        .expect("`releases` must be an array");
    assert!(
        releases.len() > 1,
        "the changelog carries many releases; got {}",
        releases.len()
    );

    // The panel marks "you are running this" by matching /health's `release`
    // against a version in this list. They come from two different baked
    // constants (RELEASE and CHANGELOG.md), so nothing but this holds them
    // together, and a drift would leave the panel silently marking nothing.
    assert_eq!(
        releases[0]["version"],
        lucidos_engine::LUCIDOS_RELEASE,
        "the newest release should be the one this engine reports running"
    );

    for release in releases {
        assert!(
            release["version"].is_string(),
            "every release needs a version: {release}"
        );
        // Raw markdown, per `.claude/rules/rust.md`: the frontend converts.
        let notes = release["notes"]
            .as_str()
            .unwrap_or_else(|| panic!("every release needs notes: {release}"));
        assert!(!notes.trim().is_empty(), "empty notes for {release}");
        assert!(
            !notes.starts_with("## v"),
            "notes must exclude their own heading: {release}"
        );
    }
}

/// The endpoint reads a compile-time constant, so it must answer identically
/// whatever the workspace, the checkout or the database are doing. A handler
/// that grew a filesystem or state dependency would show up here as a difference
/// between two calls, and on a packaged install as an empty panel.
#[tokio::test]
async fn changelog_is_stateless_and_repeatable() {
    let client = http_client();
    let url = format!("{}/api/v1/engine/changelog", base_url());

    let first: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("Changelog request failed")
        .json()
        .await
        .expect("Invalid JSON");
    let second: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("Changelog request failed")
        .json()
        .await
        .expect("Invalid JSON");

    assert_eq!(first, second);
}
