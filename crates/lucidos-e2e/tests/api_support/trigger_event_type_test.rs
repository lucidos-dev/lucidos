use crate::support::{base_url, unique_marker, user_client};
use serde_json::json;
use uuid::Uuid;

fn unreachable_intent() -> serde_json::Value {
    json!({ "type": "intent", "intent": "never executed: an event-type validation probe" })
}

fn never_emitted_name(prefix: &str) -> String {
    format!("E2e{prefix}{}", Uuid::new_v4().simple())
}

async fn post_trigger(
    client: &reqwest::Client,
    name: &str,
    on: serde_json::Value,
) -> serde_json::Value {
    client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&json!({ "name": name, "run": unreachable_intent(), "on": on }))
        .send()
        .await
        .expect("POST /triggers failed")
        .json()
        .await
        .expect("Invalid JSON")
}

async fn put_trigger(
    client: &reqwest::Client,
    id: &str,
    on: serde_json::Value,
) -> serde_json::Value {
    client
        .put(format!("{}/api/v1/triggers?id={}", base_url(), id))
        .json(&json!({ "on": on }))
        .send()
        .await
        .expect("PUT /triggers failed")
        .json()
        .await
        .expect("Invalid JSON")
}

async fn list_triggers(client: &reqwest::Client) -> Vec<serde_json::Value> {
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/triggers", base_url()))
        .send()
        .await
        .expect("GET /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    body["triggers"].as_array().cloned().unwrap_or_default()
}

async fn trigger_id_by_name(client: &reqwest::Client, name: &str) -> Option<String> {
    list_triggers(client)
        .await
        .iter()
        .find(|t| t["name"] == name)
        .and_then(|t| t["id"].as_str().map(str::to_string))
}

async fn delete_trigger(client: &reqwest::Client, id: &str) {
    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), id))
        .send()
        .await;
}

#[tokio::test]
async fn create_refuses_a_misspelled_engine_event_and_names_the_near_match() {
    let client = user_client().await;
    let name = unique_marker("e2e-event-type-misspelled");
    let result = post_trigger(
        &client,
        &name,
        json!([{ "event_type": "CredentialStored" }]),
    )
    .await;

    assert_eq!(result["success"], false, "must be refused: {result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("CredentialStored") && error.contains("not an event Lucidos emits"),
        "the refusal names what was wrong: {error}"
    );
    assert!(
        error.contains("CredentialCreated"),
        "and offers the near match, which is what makes it actionable: {error}"
    );
    assert!(
        trigger_id_by_name(&client, &name).await.is_none(),
        "a refused create arms nothing"
    );
}

#[tokio::test]
async fn create_refuses_a_retired_event_name() {
    let client = user_client().await;
    let name = unique_marker("e2e-event-type-retired");
    let result = post_trigger(&client, &name, json!([{ "event_type": "ClaudeCodeIdled" }])).await;

    assert_eq!(result["success"], false, "must be refused: {result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("retired event name"),
        "a rename is its own diagnosis, not a spelling mistake: {error}"
    );
    assert!(
        error.contains("CodingAgentIdled"),
        "and the current name is the whole answer: {error}"
    );
}

#[tokio::test]
async fn create_refuses_a_transient_frame_and_names_its_terminator() {
    let client = user_client().await;
    let name = unique_marker("e2e-event-type-transient");
    let result = post_trigger(&client, &name, json!([{ "event_type": "BackupProgress" }])).await;

    assert_eq!(result["success"], false, "must be refused: {result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("transient system frame"),
        "it exists, so the refusal must not call it a typo: {error}"
    );
    assert!(
        error.contains("BackupCompleted"),
        "and points at the event that is written to the store: {error}"
    );
}

#[tokio::test]
async fn create_accepts_an_unseen_domain_event_and_warns() {
    let client = user_client().await;
    let name = unique_marker("e2e-event-type-warn");
    let event_type = never_emitted_name("NeverEmitted");
    let result = post_trigger(&client, &name, json!([{ "event_type": event_type }])).await;

    assert_eq!(
        result["success"], true,
        "a forward-looking domain event must not be blocked: {result}"
    );
    let warnings = result["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("the success carries its warnings: {result}"));
    assert_eq!(warnings.len(), 1, "one entry, one warning: {result}");
    let warning = warnings[0].as_str().unwrap_or_default();
    assert!(
        warning.contains(&event_type) && warning.contains("never been emitted"),
        "the warning names the type and says what it means: {warning}"
    );

    let id = trigger_id_by_name(&client, &name)
        .await
        .expect("the accepted trigger is listed");
    delete_trigger(&client, &id).await;
}

#[tokio::test]
async fn create_checks_every_entry_in_the_on_array() {
    let client = user_client().await;
    let name = unique_marker("e2e-event-type-second-entry");
    let result = post_trigger(
        &client,
        &name,
        json!([
            { "event_type": "ChangeProposed" },
            { "event_type": "ThreadFinished" },
        ]),
    )
    .await;

    assert_eq!(result["success"], false, "must be refused: {result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("ThreadFinished"),
        "the second entry is the bad one and the error must say so: {error}"
    );
    assert!(
        trigger_id_by_name(&client, &name).await.is_none(),
        "a refused create arms nothing, not even the live half of the list"
    );
}

#[tokio::test]
async fn update_refuses_a_dead_name_and_warns_on_an_unseen_one() {
    let client = user_client().await;
    let name = unique_marker("e2e-event-type-update");
    let armed = never_emitted_name("UpdateArmed");
    let created = post_trigger(&client, &name, json!([{ "event_type": armed }])).await;
    assert_eq!(created["success"], true, "setup create failed: {created}");
    let id = trigger_id_by_name(&client, &name)
        .await
        .expect("the created trigger is listed");

    let refused = put_trigger(
        &client,
        &id,
        json!([
            { "event_type": armed },
            { "event_type": "CredentialRequestResolved" },
        ]),
    )
    .await;
    assert_eq!(refused["success"], false, "must be refused: {refused}");
    let error = refused["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("CredentialRequestResolved") && error.contains("CredentialRequested"),
        "the refusal names the dead entry and its near match: {error}"
    );

    let unseen = never_emitted_name("UpdateUnseen");
    let ok = put_trigger(&client, &id, json!([{ "event_type": unseen }])).await;
    assert_eq!(ok["success"], true, "an unseen name is accepted: {ok}");
    assert!(
        ok["cron_preview"].is_null(),
        "this update rewrote no schedule: {ok}"
    );
    let warnings = ok["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("an event-only update carries its warnings: {ok}"));
    assert_eq!(warnings.len(), 1, "one entry, one warning: {ok}");
    assert!(warnings[0].as_str().unwrap_or_default().contains(&unseen));

    let quiet = put_trigger(&client, &id, json!([{ "event_type": "ChangeProposed" }])).await;
    assert_eq!(quiet["success"], true, "a real engine event: {quiet}");
    assert!(
        quiet["warnings"].is_null(),
        "nothing to say about a name the engine emits: {quiet}"
    );

    delete_trigger(&client, &id).await;
}

#[tokio::test]
async fn every_name_the_catalog_offers_survives_the_validator() {
    let client = user_client().await;
    let types: Vec<String> = client
        .get(format!("{}/api/v1/events/types", base_url()))
        .send()
        .await
        .expect("GET /events/types failed")
        .json()
        .await
        .expect("Invalid JSON");

    assert!(types.len() > 50, "the catalog is derived, not a stub");
    assert!(
        types.contains(&"BackupCompleted".to_string()),
        "a persisted system event belongs here: the scheduler routes those to \
         the trigger matcher, and the old hand-written list carried none"
    );
    assert!(types.contains(&"ChildThreadCompleted".to_string()));

    let mut on: Vec<serde_json::Value> = types.iter().map(|t| json!({ "event_type": t })).collect();
    on.push(json!({ "event_type": "CredentialStored" }));

    let name = unique_marker("e2e-event-type-catalog");
    let result = post_trigger(&client, &name, json!(on)).await;
    assert_eq!(
        result["success"], false,
        "the trailing dead name refuses the create: {result}"
    );
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("CredentialStored"),
        "the ONLY entry that may fail is the dead one this test appended. A \
         different name here is a catalog offering something the validator \
         refuses: {error}"
    );
    assert!(
        trigger_id_by_name(&client, &name).await.is_none(),
        "a refused create arms nothing"
    );
}
