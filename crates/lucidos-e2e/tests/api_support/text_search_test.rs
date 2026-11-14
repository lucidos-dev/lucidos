//! `GET /api/v1/search/text`: Search Everywhere's Text category, end to end
//! through the router, against a file written by the normal data-write path.

use serde_json::Value;

use crate::support::{
    base_url, remove_data_fixtures, unique_marker, user_client, workspace_tree_lock,
    write_data_fixture,
};

async fn search_text(client: &reqwest::Client, q: &str, mode: &str) -> Value {
    let response = client
        .get(format!("{}/api/v1/search/text", base_url()))
        .query(&[("q", q), ("mode", mode)])
        .send()
        .await
        .expect("GET /api/v1/search/text failed");
    assert_eq!(response.status(), 200, "search/text?q={q}&mode={mode}");
    response.json().await.expect("search/text body is JSON")
}

#[tokio::test]
async fn text_search_finds_a_phrase_at_its_line() {
    let client = user_client().await;
    let dir = unique_marker("text-search");
    let phrase = unique_marker("Phrase");
    let rel = format!("artifacts/{dir}/notes.md");
    {
        let _tree = workspace_tree_lock().read().await;
        write_data_fixture(
            &client,
            &rel,
            &format!("# Notes\n\nfirst line\n  see the {phrase} here\n"),
        )
        .await
        .expect("write the fixture");
    }

    for mode in ["preview", "all"] {
        let body = search_text(&client, &phrase.to_lowercase(), mode).await;
        assert_eq!(body["status"], "ok", "{mode}: {body}");
        assert_eq!(body["skipped_large_files"], 0, "{mode}: {body}");
        let hits = body["hits"].as_array().expect("hits array");
        assert_eq!(hits.len(), 1, "{mode}: {body}");
        let hit = &hits[0];
        assert_eq!(hit["path"], rel.as_str());
        assert_eq!(hit["line"], 4);
        assert_eq!(hit["before"], "see the ");
        assert_eq!(hit["matched"], phrase.as_str());
        assert_eq!(hit["after"], " here");
    }

    remove_data_fixtures(&client, &format!("artifacts/{dir}"), &[rel]).await;
}

#[tokio::test]
async fn text_search_refuses_a_short_query_with_the_minimum() {
    let client = user_client().await;
    let body = search_text(&client, "ab", "all").await;
    assert_eq!(body["status"], "query-too-short", "{body}");
    assert_eq!(body["min_query_chars"], 3, "pins the wire contract: {body}");
    assert!(body.get("hits").is_none(), "{body}");
}

#[tokio::test]
async fn text_search_rejects_an_unknown_mode() {
    let client = user_client().await;
    let response = client
        .get(format!("{}/api/v1/search/text", base_url()))
        .query(&[("q", "anything"), ("mode", "everything")])
        .send()
        .await
        .expect("GET /api/v1/search/text failed");
    assert_eq!(response.status(), 400);
}
