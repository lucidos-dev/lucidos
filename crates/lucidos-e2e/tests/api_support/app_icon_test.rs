use crate::support::{
    base_url, remove_data_fixtures, unique_marker, user_client, workspace_tree_lock,
    write_data_fixture,
};

fn manifest(app_id: &str, icon: &str) -> String {
    serde_json::json!({ "id": app_id, "name": "Icon probe", "icon": icon }).to_string()
}

async fn listed_icon(client: &reqwest::Client, app_id: &str) -> Option<String> {
    let apps: Vec<serde_json::Value> = client
        .get(format!("{}/api/v1/apps", base_url()))
        .send()
        .await
        .expect("apps list request failed")
        .json()
        .await
        .expect("apps list is JSON");
    let app = apps
        .iter()
        .find(|a| a["id"] == app_id)
        .unwrap_or_else(|| panic!("{app_id} is listed"));
    app["icon"].as_str().map(str::to_string)
}

#[tokio::test]
async fn a_listed_icon_loads_and_an_escaping_one_is_never_listed() {
    let client = user_client().await;
    let marker = unique_marker("appicon");
    let good = format!("e2e-{marker}-good");
    let escaping = format!("e2e-{marker}-out");
    let fixtures = [
        (
            format!("apps/{good}/manifest.json"),
            manifest(&good, "assets/icon.svg"),
        ),
        (format!("apps/{good}/assets/icon.svg"), "<svg/>".to_string()),
        (
            format!("apps/{escaping}/manifest.json"),
            manifest(&escaping, &format!("../{good}/assets/icon.svg")),
        ),
    ];
    let tree = workspace_tree_lock().read().await;
    for (rel, body) in &fixtures {
        write_data_fixture(&client, rel, body).await.unwrap();
    }

    assert_eq!(
        listed_icon(&client, &good).await.as_deref(),
        Some("assets/icon.svg")
    );
    let icon = client
        .get(format!("{}/app/{good}/assets/icon.svg", base_url()))
        .send()
        .await
        .expect("icon request failed");
    assert_eq!(icon.status(), 200);
    assert_eq!(
        icon.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/svg+xml")
    );

    assert_eq!(listed_icon(&client, &escaping).await, None);
    let escaped = client
        .get(format!(
            "{}/app/{escaping}/..%2F{good}%2Fassets%2Ficon.svg",
            base_url()
        ))
        .send()
        .await
        .expect("escape request failed");
    assert_ne!(escaped.status(), 200, "the route refuses a `..` path");

    drop(tree);
    let files: Vec<String> = fixtures.into_iter().map(|(rel, _)| rel).collect();
    remove_data_fixtures(&client, "apps", &files).await;
}
