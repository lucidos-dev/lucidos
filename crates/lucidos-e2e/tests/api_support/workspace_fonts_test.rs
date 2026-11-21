//! Workspace fonts over HTTP (ADR 0308): the data route's gate, the listing,
//! the `/data` mount serving the face, and a theme naming the font.
//!
//! `core/workspace_fonts` unit tests cover every rule. Only a booted engine
//! shows that the routes agree: a font written through `PUT /api/v1/data` is
//! listed by `GET /api/v1/fonts`, and its face path loads from `/data`.

use crate::support::{
    base_url, e2e_font_slug, http_client, user_client, workspace_path, workspace_tree_lock,
};

fn api(path: &str) -> String {
    format!("{}/api/v1{}", base_url(), path)
}

/// Enough bytes to pass the engine's check. No browser parses them here.
const FAKE_WOFF2: &[u8] = b"wOF2 not a real font, only its magic number";

#[tokio::test]
async fn a_workspace_font_is_gated_listed_served_and_nameable_from_a_theme() {
    let _tree = workspace_tree_lock().read().await;
    let client = user_client().await;
    let slug = e2e_font_slug("e2e-font");
    let id = format!("ws-{slug}");
    let dir = workspace_path().join("data/fonts").join(&slug);

    // Refused before anything reaches disk.
    for (file, body) in [
        ("index.html", b"<script>alert(1)</script>".as_slice()),
        ("a.woff2", b"<html>not a font".as_slice()),
        (
            "font.json",
            br#"{"label":"x","group":"sans","faces":[{"file":"https://cdn.example.com/a.woff2"}]}"#
                .as_slice(),
        ),
    ] {
        let resp = client
            .put(api(&format!("/data/fonts/{slug}/{file}")))
            .body(body.to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 422, "{file} must be refused");
        assert!(!dir.join(file).exists(), "{file} reached disk");
    }

    // The face first, then the manifest.
    let face = client
        .put(api(&format!("/data/fonts/{slug}/Brand.woff2")))
        .body(FAKE_WOFF2.to_vec())
        .send()
        .await
        .unwrap();
    assert!(face.status().is_success(), "face: {}", face.status());
    let manifest = client
        .put(api(&format!("/data/fonts/{slug}/font.json")))
        .body(r#"{"label":"Brand Mono","group":"mono","faces":[{"file":"Brand.woff2","weight":"100 900"}]}"#)
        .send()
        .await
        .unwrap();
    assert!(
        manifest.status().is_success(),
        "manifest: {}",
        manifest.status()
    );

    let listing: serde_json::Value = client
        .get(api("/fonts"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let font = listing["fonts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == id.as_str())
        .unwrap_or_else(|| panic!("{id} is not listed: {listing}"));
    assert_eq!(font["source"], "workspace");
    assert_eq!(font["kind"], "both");
    assert_eq!(font["family"], id.as_str());
    let path = font["faces"][0]["path"].as_str().unwrap().to_string();
    assert_eq!(path, format!("fonts/{slug}/Brand.woff2"));

    // The face loads from the workspace, as an app frame's font load asks.
    let resp = http_client()
        .get(format!("{}/data/{path}", base_url()))
        .header("origin", "null")
        .header("sec-fetch-site", "cross-site")
        .header("sec-fetch-mode", "cors")
        .header("sec-fetch-dest", "font")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("*"),
        "a font load across the frame's opaque origin needs the grant"
    );
    assert_eq!(resp.bytes().await.unwrap().as_ref(), FAKE_WOFF2);

    // A theme may name it in both slots, and never a font that is not installed.
    let theme_id = format!("e2e-theme-{}", uuid::Uuid::new_v4().simple());
    let theme_path = api(&format!("/data/themes/{theme_id}.json"));
    let missing = client
        .put(&theme_path)
        .body(r#"{"name":"Branded","fonts":{"ui":"ws-e2e-no-such-font"}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 422);
    let saved = client
        .put(&theme_path)
        .body(format!(
            r#"{{"name":"Branded","fonts":{{"ui":"{id}","mono":"{id}"}}}}"#
        ))
        .send()
        .await
        .unwrap();
    assert!(saved.status().is_success(), "theme: {}", saved.status());
    let theme: serde_json::Value = client
        .get(api(&format!("/theme?id={theme_id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(theme["resolved"]["fonts"]["mono"], id.as_str());
    assert_eq!(theme["resolved"]["workspace_fonts"][0]["id"], id.as_str());
    assert!(theme["resolved"]["dark"]["--font-mono"]
        .as_str()
        .unwrap()
        .starts_with(&format!("'{id}'")));

    // Removing the font drops it from the list and from the theme.
    for file in ["font.json", "Brand.woff2"] {
        let resp = client
            .delete(api(&format!("/data/fonts/{slug}/{file}")))
            .send()
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "delete {file}: {}",
            resp.status()
        );
    }
    let theme: serde_json::Value = client
        .get(api(&format!("/theme?id={theme_id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        theme["resolved"].get("workspace_fonts").is_none(),
        "{theme}"
    );
    assert!(
        theme["resolved"]["dark"].get("--font-mono").is_none(),
        "{theme}"
    );
    client.delete(&theme_path).send().await.unwrap();
}
