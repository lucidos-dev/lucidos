use reqwest::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, ETAG, IF_NONE_MATCH, VARY,
};

use crate::support::{base_url, user_client};

const SLICE_COUNT: usize = 25;

fn bootstrap_url() -> String {
    format!("{}/api/v1/bootstrap", base_url())
}

#[tokio::test]
async fn the_snapshot_answers_every_slice_as_data_or_as_an_error() {
    let client = user_client().await;
    let response = client.get(bootstrap_url()).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()[CACHE_CONTROL], "private, no-cache");
    let vary: Vec<&str> = response
        .headers()
        .get_all(VARY)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    assert!(vary.join(",").contains("x-lucidos-device-id"), "{vary:?}");
    let etag = response.headers()[ETAG].to_str().unwrap().to_owned();
    assert!(etag.starts_with("W/\""), "a weak validator: {etag}");

    let body: serde_json::Map<String, serde_json::Value> = response.json().await.unwrap();
    assert_eq!(body.len(), SLICE_COUNT);
    for (key, slice) in &body {
        let error = &slice["error"];
        let is_error = error["message"].is_string() && error["http_code"].is_u64();
        assert!(
            slice.get("data").is_some() != is_error,
            "{key} is data or an error: {slice}"
        );
    }
}

#[tokio::test]
async fn a_slice_is_the_body_its_narrow_route_answers() {
    let client = user_client().await;
    let snapshot: serde_json::Value = client
        .get(bootstrap_url())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for (key, path) in [("users", "/users"), ("reader_devices", "/users/me")] {
        let route: serde_json::Value = client
            .get(format!("{}/api/v1{path}", base_url()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(snapshot[key]["data"], route, "{key}");
    }
}

#[tokio::test]
async fn a_held_validator_gets_a_bodiless_not_modified_even_compressed() {
    let client = user_client().await;
    let full = client
        .get(bootstrap_url())
        .header(ACCEPT_ENCODING, "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(full.status(), 200);
    assert_eq!(full.headers()[CONTENT_ENCODING], "gzip");
    assert!(full.headers()[ETAG].to_str().unwrap().starts_with("W/\""));

    let held = client
        .get(bootstrap_url())
        .header(ACCEPT_ENCODING, "gzip")
        .header(IF_NONE_MATCH, "*")
        .send()
        .await
        .unwrap();
    assert_eq!(held.status(), 304);
    assert!(held.headers()[ETAG].to_str().unwrap().starts_with("W/\""));
    assert_eq!(held.headers()[CACHE_CONTROL], "private, no-cache");
    assert!(held.bytes().await.unwrap().is_empty());
}
