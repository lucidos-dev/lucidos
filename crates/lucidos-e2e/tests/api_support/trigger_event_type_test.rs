//! E2E for **event-type validation on the trigger write endpoints**.
//!
//! A trigger armed on a name the engine never emits is accepted, sits armed and
//! never fires. Nothing reports it: not the response, not the row, not the UI.
//! `EventSubscription::matches` compares names as exact strings, so a typo, a
//! hallucinated name and a name retired in a past release all read the same.
//!
//! The unit tests in `core::event_subscription` pin the three verdicts against
//! the derived corpus. This pins the WIRING at the two trigger surfaces, over
//! real HTTP, because the check is worth nothing unless the caller reads it:
//!
//! * a dead engine name is refused, with the near match named
//! * a transient streaming frame is refused, pointed at its terminator
//! * a never-seen name is ACCEPTED and warns, so a forward-looking domain event
//!   still works and a typo in one is still caught
//! * every entry in the `on` array is checked, not just the first
//! * the same three verdicts on update, not only on create
//! * every name `GET /events/types` offers survives the validator, so the
//!   refusals point somewhere that actually answers them

use crate::support::{base_url, unique_marker, user_client};
use serde_json::json;
use uuid::Uuid;

/// A `run` for a trigger that is never meant to fire. Every trigger here is
/// refused outright or deleted at the end of its test. Nothing reaches
/// execution, so no file is written under `data/`.
fn unreachable_intent() -> serde_json::Value {
    json!({ "type": "intent", "intent": "never executed: an event-type validation probe" })
}

/// A name no engine event resembles and nobody has emitted. The uuid is what
/// makes the second half true: the suite emits domain events of its own, and a
/// name another test happened to emit would be real by proof.
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

/// Look the trigger up by NAME rather than counting rows. The suite runs its
/// tests concurrently, so a neighbour creating one of its own would move a
/// count while this test is reading it.
async fn trigger_id_by_name(client: &reqwest::Client, name: &str) -> Option<String> {
    list_triggers(client)
        .await
        .iter()
        .find(|t| t["name"] == name)
        .and_then(|t| t["id"].as_str().map(str::to_string))
}

/// Best-effort: a failed cleanup must not fail the assertion the test exists
/// for.
async fn delete_trigger(client: &reqwest::Client, id: &str) {
    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), id))
        .send()
        .await;
}

/// Case 1, the reported incident. `CredentialStored` does not exist. The engine
/// emits `CredentialCreated` when a credential modal resolves.
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

/// Case 1 again, through the other door: a name retired by a rename. Rows
/// written before the rename still read back under it, so a subscription looks
/// plausible and can only ever match history. Both live renames took a live
/// subscription with them.
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

/// Case 3: a frame that genuinely exists, writes no row and reaches no matcher.
/// The message is distinct from case 1, because the name is not wrong. It names
/// the terminator to subscribe to instead.
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

/// Case 2, the one to get right. A domain event this workspace has not emitted
/// yet is legitimate: "make X emit an event, then trigger on it" is the ordinary
/// order of work. It is accepted, and the warning rides along so a typo in the
/// caller's OWN event name is still catchable.
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

/// Every entry in the `on` array, not just the first. A list where only the
/// second name is dead still reads as fully armed. It then watches for less
/// than the caller asked for.
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
    // The first failure refuses the WHOLE call. A partly armed trigger would
    // read as a success while watching for less than was asked.
    assert!(
        trigger_id_by_name(&client, &name).await.is_none(),
        "a refused create arms nothing, not even the live half of the list"
    );
}

/// Update is the other door onto the same list, and it had no validation at all
/// before this change. A live trigger must not be editable into a dead
/// subscription.
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

    // A dead name anywhere in the new list refuses the whole update.
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

    // The same warning as on create, on an update that carries no cron. The
    // warnings must not depend on a cron preview to hang them on.
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

    // A known engine event is silent: no warning, no refusal.
    let quiet = put_trigger(&client, &id, json!([{ "event_type": "ChangeProposed" }])).await;
    assert_eq!(quiet["success"], true, "a real engine event: {quiet}");
    assert!(
        quiet["warnings"].is_null(),
        "nothing to say about a name the engine emits: {quiet}"
    );

    delete_trigger(&client, &id).await;
}

/// The loop every refusal promises: look the real name up, and it is accepted.
///
/// A refusal that names no source of truth just moves the guessing one step on.
/// So `GET /events/types` answers "which names exist here?", and this walks the
/// whole answer back through the validator that refused the guess. Both read
/// `subscribable_event_type_names`, and this is what catches them parting.
///
/// **Nothing is armed.** One dead name is appended to the list, and the first
/// failure refuses the whole create. So an error that names the dead entry
/// proves every real name ahead of it passed clean, and the row is never
/// written. Arming the list for real would subscribe to `MessageReceived` and
/// fire on the suite's own traffic.
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
