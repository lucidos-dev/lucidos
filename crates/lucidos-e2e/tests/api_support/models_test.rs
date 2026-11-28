use crate::support::{base_url, unique_marker, user_client};
use serde_json::json;

async fn find_model(client: &reqwest::Client, api: &str, id: &str) -> Option<serde_json::Value> {
    let resp = client
        .get(format!("{}/api/v1/models", api))
        .send()
        .await
        .expect("list models failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    body["models"]
        .as_array()
        .expect("models array")
        .iter()
        .find(|m| m["id"] == id)
        .cloned()
}

async fn delete_model(client: &reqwest::Client, api: &str, id: &str) {
    client
        .delete(format!("{}/api/v1/models", api))
        .query(&[("id", id)])
        .send()
        .await
        .expect("delete failed");
}

#[tokio::test]
async fn context_window_round_trips_over_http() {
    let client = user_client().await;
    let api = base_url();
    let id = unique_marker("e2e-model");

    let resp = client
        .post(format!("{}/api/v1/models", api))
        .json(&json!({
            "id": id,
            "label": "E2E Model",
            "provider": "openrouter",
            "context_window": 1_048_576,
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );

    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert_eq!(
        listed["routes"][0]["context_window"], 1_048_576,
        "the declared window must survive the create round trip"
    );

    let resp = client
        .put(format!("{}/api/v1/models", api))
        .query(&[("id", &id)])
        .json(&json!({ "context_window": 262_144 }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );
    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert_eq!(listed["routes"][0]["context_window"], 262_144);

    let resp = client
        .put(format!("{}/api/v1/models", api))
        .query(&[("id", &id)])
        .json(&json!({ "on_chat_ladder": false }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(resp.status(), 200);
    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert_eq!(
        listed["routes"][0]["context_window"], 262_144,
        "an unrelated PUT must not clear the declared window"
    );
    assert_eq!(listed["on_chat_ladder"], false, "a new model starts off");

    let resp = client
        .put(format!("{}/api/v1/models", api))
        .query(&[("id", &id)])
        .json(&json!({ "context_window": null }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(resp.status(), 200);
    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert!(
        listed["routes"][0]["context_window"].is_null(),
        "an explicit null must clear the declaration"
    );

    delete_model(&client, &api, &id).await;
}

#[tokio::test]
async fn every_model_declares_the_reasoning_tiers_its_provider_supports() {
    let client = user_client().await;
    let api = base_url();

    for (id, expected) in [
        (
            "claude-opus-5",
            vec!["none", "low", "medium", "high", "xhigh", "max"],
        ),
        (
            "claude-haiku-5-5",
            vec!["none", "low", "medium", "high", "xhigh", "max"],
        ),
        (
            "claude-fable-5-1",
            vec!["low", "medium", "high", "xhigh", "max"],
        ),
        (
            "claude-opus-5-5",
            vec!["low", "medium", "high", "xhigh", "max"],
        ),
        (
            "claude-sonnet-5-5",
            vec!["low", "medium", "high", "xhigh", "max"],
        ),
        (
            "claude-sonnet-4-6",
            vec!["none", "low", "medium", "high", "max"],
        ),
        (
            "gemini-3-flash-preview",
            vec!["none", "low", "medium", "high"],
        ),
        ("gemini-3.8-flash", vec!["low", "medium", "high"]),
        (
            "gpt-5.6-sol",
            vec!["none", "low", "medium", "high", "xhigh", "max"],
        ),
        (
            "gpt-6-astra",
            vec!["none", "low", "medium", "high", "xhigh", "max"],
        ),
        ("gpt-5.5", vec!["none", "low", "medium", "high", "xhigh"]),
        ("z-ai/glm-5.2", vec!["none", "low", "medium", "high"]),
        ("grok-4.6", vec!["none", "low", "medium", "high"]),
        ("x-preview-f-free", vec!["low", "high", "max"]),
        ("laguna-s-2.1-free", vec!["none", "low", "medium", "high"]),
    ] {
        let m = find_model(&client, &api, id)
            .await
            .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
        let tiers: Vec<&str> = m["routes"][0]["reasoning_efforts"]
            .as_array()
            .unwrap_or_else(|| panic!("{id} must carry reasoning_efforts on its route"))
            .iter()
            .map(|v| v.as_str().expect("tier is a string"))
            .collect();
        assert_eq!(tiers, expected, "{id}");
    }
}

#[tokio::test]
async fn a_retired_builtin_is_off_the_chat_ladder_and_names_its_successor() {
    let client = user_client().await;
    let api = base_url();
    let retired = find_model(&client, &api, "gemini-3.5-flash")
        .await
        .expect("the retired row stays in the registry");
    assert_eq!(retired["on_chat_ladder"], false);
    assert_eq!(retired["successor"], "gemini-3.8-flash");
    let current = find_model(&client, &api, "gemini-3.8-flash")
        .await
        .expect("the successor is seeded");
    assert!(current["successor"].is_null());
}

#[tokio::test]
async fn a_new_local_model_is_offered_only_the_universally_safe_tiers() {
    let client = user_client().await;
    let api = base_url();
    let id = unique_marker("e2e-model-local");

    let resp = client
        .post(format!("{}/api/v1/models", api))
        .json(&json!({ "id": id, "label": "Local", "provider": "local" }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);

    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert_eq!(
        listed["routes"][0]["reasoning_efforts"],
        json!(["none", "low", "medium", "high"])
    );

    delete_model(&client, &api, &id).await;
}

#[tokio::test]
async fn seeded_grok_models_report_xai_and_their_declared_windows() {
    let client = user_client().await;
    let api = base_url();

    for (id, window) in [
        ("grok-4.6", 500_000),
        ("grok-4.5", 500_000),
        ("grok-4.20", 2_000_000),
        ("grok-4.3", 1_000_000),
    ] {
        let m = find_model(&client, &api, id)
            .await
            .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
        assert_eq!(m["routes"][0]["provider"], "xai", "{id}");
        assert_eq!(m["source"], "builtin", "{id}");
        assert_eq!(m["routes"][0]["context_window"], window, "{id}");
    }
}

#[tokio::test]
async fn a_bare_xai_grok_and_an_openrouter_grok_coexist() {
    let client = user_client().await;
    let api = base_url();
    let prefixed = "x-ai/grok-4.6";

    let resp = client
        .post(format!("{}/api/v1/models", api))
        .json(&json!({
            "id": prefixed,
            "label": "Grok 4.6 (OpenRouter)",
            "provider": "openrouter",
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true,
        "creating the OpenRouter-prefixed Grok row must succeed"
    );

    let bare = find_model(&client, &api, "grok-4.6")
        .await
        .expect("the seeded bare id must still be listed");
    let via_openrouter = find_model(&client, &api, prefixed)
        .await
        .expect("the prefixed id must list as its own row");
    assert_eq!(bare["routes"][0]["provider"], "xai");
    assert_eq!(via_openrouter["routes"][0]["provider"], "openrouter");

    delete_model(&client, &api, prefixed).await;
    assert!(
        find_model(&client, &api, "grok-4.6").await.is_some(),
        "removing the OpenRouter row must not touch the xAI one"
    );
}

#[tokio::test]
async fn the_keyless_provider_is_accepted_at_the_models_api() {
    let client = user_client().await;
    let api = base_url();
    let id = unique_marker("e2e-free-model");

    let resp = client
        .post(format!("{}/api/v1/models", api))
        .json(&json!({
            "id": id,
            "label": "E2E Free Model",
            "provider": "opencode-free",
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true,
        "the models API must accept the opencode-free provider"
    );

    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert_eq!(listed["routes"][0]["provider"], "opencode-free");

    let seeded = find_model(&client, &api, "laguna-s-2.1-free")
        .await
        .expect("the seeded free models must be listed");
    assert_eq!(seeded["routes"][0]["provider"], "opencode-free");
    assert_eq!(seeded["routes"][0]["context_window"], 256_000);

    delete_model(&client, &api, &id).await;
}

#[tokio::test]
async fn omitted_context_window_is_null() {
    let client = user_client().await;
    let api = base_url();
    let id = unique_marker("e2e-model-nowin");

    let resp = client
        .post(format!("{}/api/v1/models", api))
        .json(&json!({ "id": id, "label": "No Window", "provider": "local" }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);

    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert!(listed["routes"][0]["context_window"].is_null());

    delete_model(&client, &api, &id).await;
}

#[tokio::test]
async fn seeded_builtins_declare_the_window_the_prefix_map_gets_wrong() {
    let client = user_client().await;
    let api = base_url();

    for id in [
        "z-ai/glm-5.2",
        "gemini-3.1-pro-preview",
        "gemini-3.5-flash",
        "gemini-3-flash-preview",
    ] {
        let m = find_model(&client, &api, id)
            .await
            .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
        assert_eq!(m["source"], "builtin");
        assert_eq!(
            m["routes"][0]["context_window"], 1_048_576,
            "{id} must declare its real 1M window — the prefix map gives it 200k"
        );
    }

    for id in [
        "gpt-5.5",
        "gpt-5.5-pro",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-6-astra",
    ] {
        let m = find_model(&client, &api, id)
            .await
            .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
        assert_eq!(m["source"], "builtin");
        assert_eq!(
            m["routes"][0]["context_window"], 1_050_000,
            "{id} must declare its real window — the prefix map understates it at 400k"
        );
    }

    for id in [
        "claude-fable-5-1[1m]",
        "claude-fable-5[1m]",
        "claude-opus-5-5[1m]",
        "claude-sonnet-5-5[1m]",
        "claude-opus-5[1m]",
        "claude-opus-4-8[1m]",
        "claude-opus-4-7[1m]",
        "claude-opus-4-6[1m]",
        "claude-sonnet-5[1m]",
        "claude-sonnet-4-6[1m]",
        "claude-fable-5-1",
        "claude-fable-5",
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-opus-5",
        "claude-haiku-5-5",
    ] {
        let m = find_model(&client, &api, id)
            .await
            .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
        assert_eq!(m["source"], "builtin");
        assert_eq!(
            m["routes"][0]["context_window"], 1_000_000,
            "{id} runs a 1M window, so its declared window must say so"
        );
    }

    for id in [
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-opus-4-6",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "gpt-5.4",
        "gpt-5.3-codex",
        "gpt-5.3-codex-spark",
        "gpt-5.2-codex",
    ] {
        let m = find_model(&client, &api, id)
            .await
            .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
        assert!(
            m["routes"][0]["context_window"].is_null(),
            "{id} must stay undeclared — the prefix map is authoritative for it"
        );
    }
}

#[tokio::test]
async fn builtin_accepts_context_window_but_keeps_its_identity() {
    let client = user_client().await;
    let api = base_url();
    let id = "claude-opus-4-5@20251101";

    let seeded = find_model(&client, &api, id)
        .await
        .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
    assert_eq!(seeded["source"], "builtin");
    let label = seeded["label"].clone();
    let provider = seeded["routes"][0]["provider"].clone();
    let sort_order = seeded["sort_order"].clone();
    let seeded_window = seeded["routes"][0]["context_window"].clone();

    let resp = client
        .put(format!("{}/api/v1/models", api))
        .query(&[("id", id)])
        .json(&json!({
            "context_window": 1_048_576,
            "label": "Hijacked",
        }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );

    let listed = find_model(&client, &api, id).await.expect("model listed");
    assert_eq!(
        listed["routes"][0]["context_window"], 1_048_576,
        "a builtin's context window must be correctable"
    );
    assert_eq!(listed["label"], label, "identity must not change");
    assert_eq!(
        listed["routes"][0]["provider"], provider,
        "a window edit must not move the route"
    );
    assert_eq!(listed["sort_order"], sort_order, "identity must not change");
    assert_eq!(listed["source"], "builtin");

    let resp = client
        .put(format!("{}/api/v1/models", api))
        .query(&[("id", id)])
        .json(&json!({ "context_window": 0 }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        false
    );
    let listed = find_model(&client, &api, id).await.expect("model listed");
    assert_eq!(
        listed["routes"][0]["context_window"], 1_048_576,
        "rejected edit changes nothing"
    );

    let resp = client
        .put(format!("{}/api/v1/models", api))
        .query(&[("id", id)])
        .json(&json!({ "context_window": seeded_window }))
        .send()
        .await
        .expect("restore failed");
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );
    let listed = find_model(&client, &api, id).await.expect("model listed");
    assert_eq!(
        listed["routes"][0]["context_window"], seeded_window,
        "the seeded window must be restored for the rest of the run"
    );
}

#[tokio::test]
async fn non_positive_context_window_is_rejected() {
    let client = user_client().await;
    let api = base_url();

    for bad in [0, -1] {
        let id = unique_marker("e2e-model-bad");
        let resp = client
            .post(format!("{}/api/v1/models", api))
            .json(&json!({
                "id": id,
                "label": "Bad",
                "provider": "openrouter",
                "context_window": bad,
            }))
            .send()
            .await
            .expect("create failed");
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            body["success"], false,
            "context_window {bad} must be rejected"
        );
        assert!(
            find_model(&client, &api, &id).await.is_none(),
            "a rejected create must not leave a row behind"
        );
    }
}

async fn set_on_chat_ladder(client: &reqwest::Client, api: &str, id: &str, on: bool) {
    let resp = client
        .put(format!("{}/api/v1/models", api))
        .query(&[("id", id)])
        .json(&json!({ "on_chat_ladder": on }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true, "{body}");
}

#[tokio::test]
async fn a_user_model_turns_on_and_off_the_chat_ladder() {
    let client = user_client().await;
    let api = base_url();
    let id = unique_marker("e2e-model-ladder");
    let resp = client
        .post(format!("{}/api/v1/models", api))
        .json(&json!({ "id": id, "label": "Ladder", "provider": "local" }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200);
    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert_eq!(listed["on_chat_ladder"], false);

    set_on_chat_ladder(&client, &api, &id, true).await;
    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert_eq!(listed["on_chat_ladder"], true);

    set_on_chat_ladder(&client, &api, &id, false).await;
    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert_eq!(listed["on_chat_ladder"], false);

    client
        .delete(format!(
            "{}/api/v1/preferences?key={}",
            api,
            lucidos_engine::core::prefs::INTELLIGENCE_LADDER_CHAT.key()
        ))
        .send()
        .await
        .expect("preference reset failed");
    delete_model(&client, &api, &id).await;
}
