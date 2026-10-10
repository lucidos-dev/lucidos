//! E2E coverage for `GET /api/v1/plugins/catalog`.
//!
//! Why over HTTP rather than in-engine: the invariant is about what the
//! REQUEST does, and the thing it must not do is clone a git repository. Only a
//! real handler behind a real router can prove that, since the engine-side
//! tests call the cache helpers directly and never reach the route.
//!
//! The route used to run a full marketplace scan per request, cloning every
//! registered repo. It now reads the *plugin catalog cache*
//! (`docs/plans/2026-09-22-plugin-catalog-served-from-cache.md`).

use crate::support::{base_url, unique_marker, user_client};
use serde_json::json;
use std::time::{Duration, Instant};

/// A source that parses as a git URL and can never be reached. Registering it
/// leaves the background scan failing, which is exactly the state under which
/// the page must still paint.
fn unreachable_source() -> String {
    format!("https://example.invalid/{}.git", unique_marker("e2e-mkt"))
}

/// A clone against an unreachable host takes seconds to give up, or hangs.
/// `shallow_clone` has no timeout. So a request that stays well inside this
/// bound is a request that did not clone.
const NO_CLONE_BUDGET: Duration = Duration::from_secs(3);

async fn get_catalog() -> (Duration, serde_json::Value) {
    let client = user_client().await;
    let started = Instant::now();
    let resp = client
        .get(format!("{}/api/v1/plugins/catalog", base_url()))
        .send()
        .await
        .expect("catalog request failed");
    assert_eq!(resp.status(), 200, "catalog must answer 200");
    let body = resp.json().await.expect("catalog body is not json");
    (started.elapsed(), body)
}

#[tokio::test]
async fn the_catalog_answers_without_cloning_anything() {
    let client = user_client().await;
    let api = base_url();
    let source = unreachable_source();

    let resp = client
        .post(format!("{api}/api/v1/plugins/marketplaces"))
        .json(&json!({ "source": source, "name": "E2E unreachable" }))
        .send()
        .await
        .expect("register failed");
    assert_eq!(resp.status(), 200, "registering a marketplace must succeed");
    let registered: serde_json::Value = resp.json().await.expect("register body is not json");
    let id = registered["marketplace"]["id"]
        .as_str()
        .expect("register response names the new id")
        .to_string();

    // Twice: the first call may be the one that starts the background scan, and
    // the second lands while that scan is still cloning. Neither may wait on it.
    for attempt in 1..=2 {
        let (elapsed, body) = get_catalog().await;
        assert!(
            elapsed < NO_CLONE_BUDGET,
            "attempt {attempt}: catalog took {elapsed:?}, which means it cloned",
        );
        let sources: Vec<&str> = body["marketplaces"]
            .as_array()
            .expect("marketplaces array")
            .iter()
            .filter_map(|m| m["source"].as_str())
            .collect();
        assert!(
            sources.contains(&source.as_str()),
            "attempt {attempt}: the live registry must be in the response: {body}",
        );
    }

    // Cleanup is best effort: the workspace is disposable, and a failure here
    // must not mask the assertions above.
    let _ = client
        .delete(format!("{api}/api/v1/plugins/marketplaces/{id}"))
        .send()
        .await;
}

/// The three freshness fields replace the skeleton on the panel. A missing one
/// is a silently broken cue rather than a visible error, so pin them here.
#[tokio::test]
async fn the_catalog_reports_its_own_freshness() {
    let (_, body) = get_catalog().await;
    let obj = body.as_object().expect("catalog body is an object");

    assert!(
        obj.get("scanning").and_then(|v| v.as_bool()).is_some(),
        "scanning must be a bool: {body}",
    );
    assert!(obj.contains_key("scanned_at"), "missing scanned_at: {body}");
    assert!(obj.contains_key("scan_error"), "missing scan_error: {body}");
}

/// The manual refresh returns as soon as the scan is queued. Waiting for the
/// scan would put the clone back on a request, which is the whole bug.
#[tokio::test]
async fn a_manual_rescan_returns_immediately() {
    let client = user_client().await;
    let started = Instant::now();

    let resp = client
        .post(format!("{}/api/v1/plugins/catalog/rescan", base_url()))
        .json(&json!({}))
        .send()
        .await
        .expect("rescan failed");

    assert_eq!(resp.status(), 200);
    assert!(
        started.elapsed() < NO_CLONE_BUDGET,
        "rescan waited for the scan it queued",
    );
}
