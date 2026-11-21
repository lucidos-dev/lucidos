//! E2E coverage for `GET /api/v1/model-prices`: the shipped price cards, and
//! the workspace override merged over them card by card (ADR 0525).

use crate::support::{base_url, user_client, workspace_tree_lock, write_data_fixture};

const OVERRIDE: &str = "config/model-prices.json";

async fn price_table(client: &reqwest::Client) -> serde_json::Value {
    let resp = client
        .get(format!("{}/api/v1/model-prices", base_url()))
        .send()
        .await
        .expect("request failed");
    assert_eq!(resp.status(), 200);
    resp.json().await.expect("json body")
}

/// A newer override card joins the shipped one and the shipped card keeps
/// its span, so a past day keeps its price. Removing the file restores the
/// shipped table.
#[tokio::test]
async fn the_override_merges_card_by_card() {
    let _tree = workspace_tree_lock().read().await;
    let client = user_client().await;

    let shipped = price_table(&client).await;
    let opus = &shipped["models"]["claude-opus-5-5"];
    assert_eq!(opus[0]["origin"], "shipped", "{opus}");
    assert!(opus[0]["source"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(shipped["override"]["present"], false, "{shipped}");

    let overlay = serde_json::json!({
        "checked": "2026-11-02",
        "models": {"claude-opus-5-5": [{
            "from": "2026-11-01", "uncached_in": 3, "cache_write": 3.75,
            "cache_read": 0.15, "out": 15, "source": "an e2e price"
        }]}
    });
    write_data_fixture(&client, OVERRIDE, &overlay.to_string())
        .await
        .expect("the override is written");
    let merged = price_table(&client).await;
    let _ = client
        .delete(format!("{}/api/v1/data/{OVERRIDE}", base_url()))
        .send()
        .await;

    let cards = merged["models"]["claude-opus-5-5"]
        .as_array()
        .expect("a card list");
    assert_eq!(cards.len(), 2, "{merged}");
    assert_eq!(cards[0], opus[0], "the shipped card stands for the past");
    assert_eq!(cards[1]["origin"], "override");
    assert_eq!(cards[1]["from"], "2026-11-01");
    assert_eq!(merged["override"]["present"], true);
    assert_eq!(merged["override"]["checked"], "2026-11-02");
    assert_eq!(merged["override"]["error"], serde_json::Value::Null);

    let restored = price_table(&client).await;
    assert_eq!(restored["models"], shipped["models"], "the file is gone");
}
