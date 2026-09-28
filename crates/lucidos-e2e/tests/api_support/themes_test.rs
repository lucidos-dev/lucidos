//! Themes over HTTP: the three read routes, and the data route's theme gate.
//!
//! `core/themes` unit tests cover validation and derivation. Only a booted
//! engine shows three more things:
//!
//! - the routes are mounted;
//! - a theme written through `PUT /api/v1/data/themes/<id>.json` is served back
//!   resolved;
//! - the data route refuses a bad theme before it reaches disk.

use crate::support::{base_url, user_client, workspace_path, workspace_tree_lock};

fn api(path: &str) -> String {
    format!("{}/api/v1{}", base_url(), path)
}

#[tokio::test]
async fn built_in_themes_and_the_token_catalog_are_served() {
    let client = user_client().await;

    let themes: serde_json::Value = client
        .get(api("/themes"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = themes["themes"]
        .as_array()
        .expect("a themes array")
        .iter()
        .filter_map(|l| l["id"].as_str())
        .collect();
    assert_eq!(
        ids.first(),
        Some(&"lucidos"),
        "the default theme leads: {ids:?}"
    );
    assert!(ids.contains(&"nord"), "{ids:?}");

    let nord: serde_json::Value = client
        .get(api("/theme?id=nord"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(nord["source"], "built-in");
    assert!(nord["resolved"]["dark"]["--bg-primary"].is_string());

    let catalog: serde_json::Value = client
        .get(api("/themes/tokens"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = catalog["tokens"]
        .as_array()
        .expect("a tokens array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for header_token in [
        "--header-fg",
        "--focus-header-tint",
        "--focus-header-underline",
    ] {
        assert!(
            names.contains(&header_token),
            "{header_token} is not listed"
        );
    }

    let missing = client
        .get(api("/theme?id=no-such-theme"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    let bad_id = client.get(api("/theme?id=Bad%20Id")).send().await.unwrap();
    assert_eq!(bad_id.status(), 400);
}

#[tokio::test]
async fn a_theme_written_through_the_data_route_is_served_resolved() {
    let _tree = workspace_tree_lock().read().await;
    let client = user_client().await;
    let id = format!("e2e-theme-{}", uuid::Uuid::new_v4().simple());
    let path = api(&format!("/data/themes/{id}.json"));

    let refused = client
        .put(&path)
        .body(r#"{"name":"Leaky","dark":{"--bg-primary":"url(https://example.com)"}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 422, "a banned value must not reach disk");
    assert!(!workspace_path()
        .join(format!("data/themes/{id}.json"))
        .exists());

    let shadow = client
        .put(api("/data/themes/nord.json"))
        .body(r#"{"name":"Shadow"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(
        shadow.status(),
        422,
        "a workspace theme cannot take a built-in id"
    );

    let saved = client
        .put(&path)
        .body(r##"{"name":"Harbour","dark":{"--bg-primary":"#0f2327","--accent":"#d4a650"}}"##)
        .send()
        .await
        .unwrap();
    assert!(
        saved.status().is_success(),
        "save failed: {}",
        saved.status()
    );

    let theme: serde_json::Value = client
        .get(api(&format!("/theme?id={id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(theme["source"], "workspace");
    assert_eq!(theme["resolved"]["dark"]["--accent"], "#d4a650");
    assert_eq!(
        theme["resolved"]["dark"]["--accent-action"], "var(--accent)",
        "the engine fills in what the accent seed implies"
    );

    let edited = client
        .post(api("/data/edit"))
        .json(&serde_json::json!({
            "path": format!("themes/{id}.json"),
            "operations": [{ "json_path": "dark.--accent", "json_value": "url(x)" }],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        edited.status(),
        400,
        "a theme is written whole, never edited in place"
    );

    let deleted = client.delete(&path).send().await.unwrap();
    assert!(
        deleted.status().is_success(),
        "delete failed: {}",
        deleted.status()
    );
    let gone = client
        .get(api(&format!("/theme?id={id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 404);
}

/// ADR 0309: the data route refuses a hostile theme with a reason its author
/// can act on. A fair theme comes back with the clamped palette.
#[tokio::test]
async fn a_hostile_theme_is_refused_and_a_fair_one_carries_the_protected_palette() {
    let _tree = workspace_tree_lock().read().await;
    let client = user_client().await;
    let id = format!("e2e-theme-{}", uuid::Uuid::new_v4().simple());
    let path = api(&format!("/data/themes/{id}.json"));

    for (body, reason) in [
        (
            r##"{"name":"Blank","dark":{"--bg-primary":"#101010","--text-primary":"#101010"}}"##,
            "--text-primary on --bg-primary is 1.0:1",
        ),
        (
            r##"{"name":"Swapped","dark":{"--accent-green":"#f85149","--accent-red":"#3fb950"}}"##,
            "--accent-green reads as red",
        ),
        (
            r#"{"name":"Stacked","tokens":{"--z-modal":"0"}}"#,
            "--z-modal is not a theme token",
        ),
        (
            r#"{"name":"Pinned","tokens":{"--protected-text":"transparent"}}"#,
            "Lucidos computes --protected-text",
        ),
        (
            r#"{"name":"Cover","tokens":{"--shadow-lg":"0 0 0 100vmax #000"}}"#,
            "the shadow --shadow-lg reaches 100vmax",
        ),
    ] {
        let refused = client.put(&path).body(body).send().await.unwrap();
        assert_eq!(refused.status(), 422, "{reason}");
        let error: serde_json::Value = refused.json().await.unwrap();
        let message = error["error"].as_str().unwrap_or_default();
        assert!(message.contains(reason), "{reason}: {message}");
        assert!(!workspace_path()
            .join(format!("data/themes/{id}.json"))
            .exists());
    }

    let saved = client
        .put(&path)
        .body(r#"{"name":"Veiled","tokens":{"--scrim":"transparent"}}"#)
        .send()
        .await
        .unwrap();
    assert!(saved.status().is_success(), "{}", saved.status());
    let theme: serde_json::Value = client
        .get(api(&format!("/theme?id={id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for mode in ["dark", "light"] {
        let scrim = theme["resolved"][mode]["--protected-scrim"]
            .as_str()
            .unwrap_or_default();
        assert!(
            scrim.starts_with("rgba(0, 0, 0, 0.4"),
            "{mode}: the protected scrim still dims: {scrim}"
        );
    }
    client.delete(&path).send().await.unwrap();
}

/// ADR 0307: the part catalog is served, the data route refuses a bad part
/// value with its reason, and a good one is served as part tokens.
#[tokio::test]
async fn theme_parts_are_served_refused_and_resolved() {
    let _tree = workspace_tree_lock().read().await;
    let client = user_client().await;

    let catalog: serde_json::Value = client
        .get(api("/themes/parts"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let chat_text = catalog["parts"]
        .as_array()
        .expect("a parts array")
        .iter()
        .find(|p| p["id"] == "chat-text")
        .expect("chat-text is a part");
    assert!(chat_text["properties"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["token"] == "--part-chat-text-text-shadow"));

    let id = format!("e2e-theme-{}", uuid::Uuid::new_v4().simple());
    let path = api(&format!("/data/themes/{id}.json"));
    let refused = client
        .put(&path)
        .body(r#"{"name":"Blurry","light":{"parts":{"chat-text":{"text-shadow":"0 0 2em red"}}}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 422);
    let error: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(
        error["error"],
        "light.parts.chat-text.text-shadow: blur 2em is over the 0.6em cap."
    );
    assert!(!workspace_path()
        .join(format!("data/themes/{id}.json"))
        .exists());

    let saved = client
        .put(&path)
        .body(r#"{"name":"Glow","parts":{"chat-text":{"text-shadow":"0 0 0.3EM var(--accent)"}}}"#)
        .send()
        .await
        .unwrap();
    assert!(saved.status().is_success(), "{}", saved.status());
    let theme: serde_json::Value = client
        .get(api(&format!("/theme?id={id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for mode in ["dark", "light"] {
        assert_eq!(
            theme["resolved"][mode]["--part-chat-text-text-shadow"],
            "0 0 0.3em var(--accent)"
        );
    }
    assert_eq!(theme["modes"], serde_json::json!([]));
    client.delete(&path).send().await.unwrap();
}

/// Any app can write `style_overrides`, so the preference write refuses a
/// part token that breaks the part grammar (ADR 0307).
#[tokio::test]
async fn a_style_override_part_token_past_a_cap_is_refused() {
    let client = user_client().await;
    let url = api("/preferences?key=style_overrides");
    let verdict: serde_json::Value = client
        .put(&url)
        .json(&serde_json::json!({
            "value": r#"{"--part-chat-text-text-shadow":"0 0 9em red"}"#
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(verdict["success"], false, "{verdict}");
    assert!(
        verdict["error"]
            .as_str()
            .unwrap_or_default()
            .contains("--part-chat-text-text-shadow: blur 9em is over the 0.6em cap."),
        "{verdict}"
    );
}

/// ADR 0307: a frame gets part tokens only through SDK tags the app loads
/// itself. The engine injects nothing into an app page that has none, so a
/// plain app comes back as written, whatever theme is active.
#[tokio::test]
async fn a_plain_app_page_gets_no_part_tokens_and_no_injected_styling() {
    let _tree = workspace_tree_lock().read().await;
    let client = user_client().await;
    let app_id = format!("e2e-plain-{}", uuid::Uuid::new_v4().simple());
    let dir = workspace_path().join("data/apps").join(&app_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        format!(r#"{{"id":"{app_id}","name":"Plain","description":"e2e"}}"#),
    )
    .unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><head><title>Plain</title></head><body><p>Hello</p></body></html>",
    )
    .unwrap();

    let page = client
        .get(format!(
            "{}/app/{app_id}/?device=e2eplaindevice",
            base_url()
        ))
        .send()
        .await
        .unwrap();
    assert!(page.status().is_success(), "{}", page.status());
    let html = page.text().await.unwrap();
    for injected in [
        "--part-",
        "sdk-prefs.js",
        "sdk-iframe.css",
        "sdk.js",
        "<style",
    ] {
        assert!(
            !html.contains(injected),
            "the engine injected {injected}: {html}"
        );
    }
    assert!(html.contains("<p>Hello</p>"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// POST /api/v1/themes/resolve answers what a draft would paint, or the
/// refusal a save would give, and writes nothing (ADR 0307).
#[tokio::test]
async fn a_draft_theme_resolves_without_being_saved() {
    let client = user_client().await;
    let resolved: serde_json::Value = client
        .post(api("/themes/resolve"))
        .body(r#"{"name":"Draft","dark":{"parts":{"header-title":{"letter-spacing":"0.08em"}}}}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        resolved["resolved"]["dark"]["--part-header-title-letter-spacing"],
        "0.08em"
    );
    assert!(resolved["resolved"]["light"]
        .get("--part-header-title-letter-spacing")
        .is_none());
    assert_eq!(resolved["modes"], serde_json::json!([]));

    let refused = client
        .post(api("/themes/resolve"))
        .body(r#"{"name":"Draft","parts":{"chat-text":{"text-shadow":"0 0 2em red"}}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 422);
    let error: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(
        error["error"],
        "parts.chat-text.text-shadow: blur 2em is over the 0.6em cap."
    );

    // A save refuses a workspace font that is not installed, so a draft does too.
    let missing_font = client
        .post(api("/themes/resolve"))
        .body(r#"{"name":"Draft","fonts":{"ui":"ws-not-installed"}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(missing_font.status(), 422);
    let error: serde_json::Value = missing_font.json().await.unwrap();
    let reason = error["error"].as_str().unwrap_or_default();
    assert!(
        reason.contains("not an installed workspace font"),
        "{error}"
    );
}

/// `theme` named the light/dark mode before the rename, so an older app still
/// sends `theme=dark`. The write is refused with the key it meant. It is never
/// stored as a theme id that silently paints the default.
#[tokio::test]
async fn a_theme_mode_written_to_the_theme_key_is_refused_with_the_key_it_meant() {
    let client = user_client().await;
    let device = format!("e2e-theme-mode-{}", uuid::Uuid::new_v4().simple());

    let refused: serde_json::Value = client
        .put(api("/preferences?key=theme"))
        .json(&serde_json::json!({ "value": "dark", "device_id": device }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(refused["success"], false, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or("")
            .contains("theme-mode"),
        "the refusal names the key it meant: {refused}"
    );

    let accepted: serde_json::Value = client
        .put(api("/preferences?key=theme-mode"))
        .json(&serde_json::json!({ "value": "dark", "device_id": device }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["success"], true, "{accepted}");
}
