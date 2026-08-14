//! E2E coverage for the `/api/v1/models` registry, focused on `context_window`
//! — the field that sizes the engine's context trim budget.
//!
//! Why this matters: the window is only inferred from the model id when the row
//! doesn't declare one, and that fallback recognises just `claude-*` and
//! `gpt-5*`. Every OpenRouter / Gemini / local model is treated as 200k until
//! this field is set, which trims their context far earlier than needed. So the
//! HTTP surface has to carry the value in both directions and has to keep the
//! absent-vs-explicit-null distinction that lets a caller clear it.

use crate::support::{base_url, http_client, unique_marker};
use serde_json::json;

/// Fetch one model from `GET /models`, or `None` if absent.
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

/// The full lifecycle of a declared context window over HTTP: set it on create,
/// read it back, change it, and clear it with an explicit `null`.
#[tokio::test]
async fn context_window_round_trips_over_http() {
    let client = http_client();
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
        listed["context_window"], 1_048_576,
        "the declared window must survive the create round trip"
    );

    // Change it.
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
    assert_eq!(listed["context_window"], 262_144);

    // A PUT that doesn't mention the field must LEAVE IT ALONE — otherwise
    // toggling `enabled` from the Settings row would silently wipe the window
    // and drop the model back to the 200k fallback.
    let resp = client
        .put(format!("{}/api/v1/models", api))
        .query(&[("id", &id)])
        .json(&json!({ "enabled": false }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(resp.status(), 200);
    let listed = find_model(&client, &api, &id).await.expect("model listed");
    assert_eq!(
        listed["context_window"], 262_144,
        "an unrelated PUT must not clear the declared window"
    );
    assert_eq!(listed["enabled"], false);

    // An explicit null DOES clear it, back to inferring from the id.
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
        listed["context_window"].is_null(),
        "an explicit null must clear the declaration"
    );

    delete_model(&client, &api, &id).await;
}

/// Every row carries the reasoning tiers its provider supports, and the picker
/// filters against exactly this list.
///
/// It is derived, not stored: `llm::reasoning::supported_efforts` keyed on the
/// row's provider and id, the SAME function `RoutingProvider` clamps a request
/// with. That shared derivation is the point. When the picker derived its own
/// answer from the model id, it offered a local model `max`, the wire layer
/// rewrote that into `xhigh` because the id was not `gpt-5.6`, and the local
/// server rejected it (400, 2026-08-12).
///
/// The sharp case is `local` / `openrouter`: `xhigh` is OpenAI-proprietary, so
/// an arbitrary third-party server must never be offered it whatever its id
/// looks like. Asserted here rather than only in the unit tests because the
/// wire shape is what the picker consumes, and `#[serde(flatten)]` means a
/// refactor could nest or drop the field without any Rust test noticing.
#[tokio::test]
async fn every_model_declares_the_reasoning_tiers_its_provider_supports() {
    let client = http_client();
    let api = base_url();

    for (id, expected) in [
        // Adaptive Claude sends the effort verbatim, so every tier is distinct.
        (
            "claude-opus-5@default",
            vec!["none", "low", "medium", "high", "xhigh", "max"],
        ),
        // The Claude budget path deliberately omits xhigh.
        (
            "claude-sonnet-4-6",
            vec!["none", "low", "medium", "high", "max"],
        ),
        // Gemini collapses everything above high onto high.
        ("gemini-3.5-flash", vec!["none", "low", "medium", "high"]),
        // GPT-5.6 has a real max; earlier OpenAI families top out at xhigh.
        (
            "gpt-5.6-sol",
            vec!["none", "low", "medium", "high", "xhigh", "max"],
        ),
        ("gpt-5.5", vec!["none", "low", "medium", "high", "xhigh"]),
        // A third-party OpenAI-compatible server: no xhigh, no max.
        ("z-ai/glm-5.2", vec!["none", "low", "medium", "high"]),
    ] {
        let m = find_model(&client, &api, id)
            .await
            .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
        let tiers: Vec<&str> = m["reasoning_efforts"]
            .as_array()
            .unwrap_or_else(|| panic!("{id} must carry reasoning_efforts at the top level"))
            .iter()
            .map(|v| v.as_str().expect("tier is a string"))
            .collect();
        assert_eq!(tiers, expected, "{id}");
    }
}

/// A user-added local model gets the conservative set the moment it is created,
/// with nothing for the user to declare. Asking someone adding a local server
/// to name the reasoning tiers it validates would be asking for something they
/// cannot know, and a wrong answer would fail their turns.
#[tokio::test]
async fn a_new_local_model_is_offered_only_the_universally_safe_tiers() {
    let client = http_client();
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
        listed["reasoning_efforts"],
        json!(["none", "low", "medium", "high"])
    );

    delete_model(&client, &api, &id).await;
}

/// A model added without the field is simply undeclared — the engine infers a
/// window from the id. This is the back-compat path every existing row takes.
#[tokio::test]
async fn omitted_context_window_is_null() {
    let client = http_client();
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
    assert!(listed["context_window"].is_null());

    delete_model(&client, &api, &id).await;
}

/// The migration seeds are really there. `scripts/lib/e2e.sh` recreates the
/// workspace database from zero on every run, so the whole migration chain —
/// and the builtin rows it inserts — runs against an empty database. When the
/// reset merely truncated every table but `_sqlx_migrations`, sqlx saw the
/// migrations as applied, their seeds never re-ran, `models` was empty, and
/// `llm::model_registry` fell back to the prefix heuristic for everything.
/// This asserts the seeds survive all the way to the HTTP surface.
#[tokio::test]
async fn seeded_builtins_declare_the_window_the_prefix_map_gets_wrong() {
    let client = http_client();
    let api = base_url();

    // Every group below mirrors one decision in the two seeding migrations
    // (20260725200708 + 20260725211150). Read their comments for the full
    // rationale; the short version is repeated per group.
    //
    // Declared, because the prefix map has no rule for these ids at all and
    // silently hands them the bare 200k default.
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
            m["context_window"], 1_048_576,
            "{id} must declare its real 1M window — the prefix map gives it 200k"
        );
    }

    // Declared, because the prefix map's `gpt-5` → 400k UNDERSTATES these. The
    // OpenAI path has no context opt-in, so the model's full window applies to
    // every request and there is nothing to under-declare for.
    for id in [
        "gpt-5.5",
        "gpt-5.5-pro",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
    ] {
        let m = find_model(&client, &api, id)
            .await
            .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
        assert_eq!(m["source"], "builtin");
        assert_eq!(
            m["context_window"], 1_050_000,
            "{id} must declare its real window — the prefix map understates it at 400k"
        );
    }

    // Declared, even though the `[1m]` suffix already infers the same number:
    // these rows DO request 1M mode, so the declaration matches the request and
    // Settings can show a real value instead of "inferred".
    for id in [
        "claude-fable-5[1m]",
        "claude-opus-5@default[1m]",
        "claude-opus-4-8@default[1m]",
        "claude-opus-4-7[1m]",
        "claude-opus-4-6[1m]",
        "claude-sonnet-5[1m]",
        "claude-sonnet-4-6[1m]",
    ] {
        let m = find_model(&client, &api, id)
            .await
            .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
        assert_eq!(m["source"], "builtin");
        assert_eq!(
            m["context_window"], 1_000_000,
            "{id} requests 1M mode, so its declared window must say so"
        );
    }

    // Undeclared on purpose, and this is the load-bearing half of the contract:
    //   * bare `claude-*` — 1M mode is gated on Lucidos's own `[1m]` suffix, so a
    //     bare id sends no `context-1m-2025-08-07` beta and 200k really is the
    //     window of the request the engine makes. Declaring 1M here would let the
    //     packer build a prompt larger than the API mode selected — the dangerous
    //     direction, since the provider then rejects it outright.
    //   * the older GPT rows — windows unverified, and under-declaring only trims
    //     early while over-declaring breaks the request.
    // (`claude-opus-4-5@20251101` is deliberately absent: it is the row
    // `builtin_accepts_context_window_but_keeps_its_identity` mutates, and these
    // tests share one database within a run.)
    for id in [
        "claude-fable-5",
        "claude-opus-5@default",
        "claude-opus-4-8@default",
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
            m["context_window"].is_null(),
            "{id} must stay undeclared — the prefix map is authoritative for it"
        );
    }
}

/// A **builtin** must accept a context-window correction while keeping its
/// identity fields. The window is a factual property of the model — the vendor
/// can raise it, and a seeded value can simply be wrong — so refusing the edit
/// would strand a builtin on a bad window forever. Identity (label / provider /
/// sort_order) stays engine-owned.
///
/// Runs against a real migration-seeded builtin, and restores its declared
/// window at the end: the database is recreated per run, but the registry is
/// shared by every test within a run.
#[tokio::test]
async fn builtin_accepts_context_window_but_keeps_its_identity() {
    let client = http_client();
    let api = base_url();
    // A seeded builtin that no other test asserts on, so these edits can't race
    // one. Identity is read from the row rather than hardcoded, so a future
    // migration relabelling it doesn't turn into a spurious failure here.
    let id = "claude-opus-4-5@20251101";

    let seeded = find_model(&client, &api, id)
        .await
        .unwrap_or_else(|| panic!("{id} must be seeded in the e2e workspace"));
    assert_eq!(seeded["source"], "builtin");
    let label = seeded["label"].clone();
    let provider = seeded["provider"].clone();
    let sort_order = seeded["sort_order"].clone();
    let seeded_window = seeded["context_window"].clone();

    let resp = client
        .put(format!("{}/api/v1/models", api))
        .query(&[("id", id)])
        .json(&json!({
            "context_window": 1_048_576,
            // These must be ignored — a builtin's identity is engine-owned.
            "label": "Hijacked",
            "provider": "local",
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
        listed["context_window"], 1_048_576,
        "a builtin's context window must be correctable"
    );
    assert_eq!(listed["label"], label, "identity must not change");
    assert_eq!(listed["provider"], provider, "identity must not change");
    assert_eq!(listed["sort_order"], sort_order, "identity must not change");
    assert_eq!(listed["source"], "builtin");

    // A bad value is rejected for builtins too, not silently swallowed.
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
        listed["context_window"], 1_048_576,
        "rejected edit changes nothing"
    );

    // Put the seeded value back (an explicit null clears the declaration).
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
        listed["context_window"], seeded_window,
        "the seeded window must be restored for the rest of the run"
    );
}

/// A non-positive window is rejected rather than stored. A zero would produce a
/// zero trim budget (everything trimmed); a negative one, cast to `usize`,
/// an enormous one.
#[tokio::test]
async fn non_positive_context_window_is_rejected() {
    let client = http_client();
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
