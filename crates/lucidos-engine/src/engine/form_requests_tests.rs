use super::*;
use crate::engine::thread_events::ActorMode;
use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};

/// A thread the projection knows about. `pending` joins `thread_summaries`,
/// and a form request creates no summary row of its own.
async fn seed_thread(bus: &EventBus) -> Uuid {
    let thread_id = Uuid::new_v4();
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "connect my weather API".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
    )
    .await;
    thread_id
}

fn credential_request(service: &str) -> (Uuid, ThreadEvent) {
    let request_id = Uuid::new_v4();
    let payload = serde_json::json!({ "service": service, "prompt": "Paste it." }).to_string();
    (
        request_id,
        ThreadEvent::CredentialRequested {
            request_id,
            payload,
        },
    )
}

fn oauth_request() -> (Uuid, ThreadEvent) {
    let request_id = Uuid::new_v4();
    let payload = serde_json::json!({
        "target": "url",
        "url": "https://auth.example.com/authorize",
        "purpose": "oauth",
    })
    .to_string();
    (
        request_id,
        ThreadEvent::OAuthAuthorizationRequested {
            request_id,
            payload,
        },
    )
}

fn plugin_install_request(plugin_id: &str) -> (Uuid, ThreadEvent) {
    let request_id = Uuid::new_v4();
    let payload = serde_json::json!({
        "install_id": request_id,
        "plugin_id": plugin_id,
    })
    .to_string();
    (
        request_id,
        ThreadEvent::PluginInstallRequested {
            request_id,
            payload,
        },
    )
}

/// Every resolution written for `request_id`, oldest first.
async fn outcomes(pool: &PgPool, request_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT payload->>'outcome' FROM events \
         WHERE event_type = 'FormRequestResolved' AND payload->>'request_id' = $1 \
         ORDER BY sequence",
    )
    .bind(request_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn pending_ids(pool: &PgPool) -> Vec<Uuid> {
    pending(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.request_id)
        .collect()
}

#[tokio::test]
async fn an_open_request_is_pending_until_it_resolves_and_never_after() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_thread(&bus).await;
    let (request_id, event) = credential_request("weather");
    seed_thread_event(&bus, thread_id, event).await;

    let listed = pending(&pool).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].thread_id, thread_id);
    assert_eq!(listed[0].request_id, request_id);
    assert_eq!(
        listed[0].event["type"], "CredentialRequested",
        "the client renders the row as the stream frame it replaces"
    );

    assert!(
        resolve(&pool, &bus, request_id, FormRequestOutcome::Canceled, None)
            .await
            .unwrap()
    );
    assert!(pending_ids(&pool).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_second_resolve_is_a_no_op_not_a_second_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_thread(&bus).await;
    let (request_id, event) = credential_request("weather");
    seed_thread_event(&bus, thread_id, event).await;

    let first = resolve(&pool, &bus, request_id, FormRequestOutcome::Completed, None);
    let second = resolve(&pool, &bus, request_id, FormRequestOutcome::Canceled, None);
    let (first, second) = tokio::join!(first, second);
    assert_ne!(
        first.unwrap(),
        second.unwrap(),
        "exactly one of two racing resolves emits"
    );
    assert_eq!(outcomes(&pool, request_id).await.len(), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolving_an_id_no_request_carries_emits_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    // A Settings-initiated OAuth flow, or a plugin staged over HTTP.
    let unknown = Uuid::new_v4();
    assert!(
        !resolve(&pool, &bus, unknown, FormRequestOutcome::Completed, None)
            .await
            .unwrap()
    );
    assert!(outcomes(&pool, unknown).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_archived_thread_offers_no_form() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_thread(&bus).await;
    let (_, event) = credential_request("weather");
    seed_thread_event(&bus, thread_id, event).await;

    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(pending_ids(&pool).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_newer_request_for_the_same_subject_supersedes_the_older_only() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_thread(&bus).await;
    let (old_weather, event) = credential_request("weather");
    seed_thread_event(&bus, thread_id, event).await;
    let (maps, event) = credential_request("maps");
    seed_thread_event(&bus, thread_id, event).await;

    let (new_weather, event) = credential_request("weather");
    supersede_same_subject(&pool, &bus, thread_id, &event).await;
    seed_thread_event(&bus, thread_id, event).await;

    assert_eq!(outcomes(&pool, old_weather).await, ["superseded"]);
    assert!(
        outcomes(&pool, maps).await.is_empty(),
        "another service is another question"
    );
    let open = pending_ids(&pool).await;
    assert!(open.contains(&maps) && open.contains(&new_weather));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_user_message_supersedes_the_forms_but_not_a_live_authorization() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_thread(&bus).await;
    let (credential, event) = credential_request("weather");
    seed_thread_event(&bus, thread_id, event).await;
    let (authorization, event) = oauth_request();
    seed_thread_event(&bus, thread_id, event).await;

    supersede_on_user_message(&pool, &bus, thread_id, None).await;

    assert_eq!(outcomes(&pool, credential).await, ["superseded"]);
    assert!(
        outcomes(&pool, authorization).await.is_empty(),
        "the OAuth listener owns its request and is still waiting"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_saved_credential_answers_every_open_request_for_its_service() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let one = seed_thread(&bus).await;
    let other = seed_thread(&bus).await;
    let (first, event) = credential_request("weather");
    seed_thread_event(&bus, one, event).await;
    let (second, event) = credential_request("weather");
    seed_thread_event(&bus, other, event).await;
    let (maps, event) = credential_request("maps");
    seed_thread_event(&bus, one, event).await;

    complete_credential_requests_for(&pool, &bus, "weather", None).await;

    assert_eq!(outcomes(&pool, first).await, ["completed"]);
    assert_eq!(outcomes(&pool, second).await, ["completed"]);
    assert!(outcomes(&pool, maps).await.is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn boot_expires_only_requests_whose_state_died_with_the_engine() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_thread(&bus).await;
    let (credential, event) = credential_request("weather");
    seed_thread_event(&bus, thread_id, event).await;
    let (install, event) = plugin_install_request("habit-tracker");
    seed_thread_event(&bus, thread_id, event).await;
    let (authorization, event) = oauth_request();
    seed_thread_event(&bus, thread_id, event).await;

    expire_memory_backed_requests(&pool, &bus).await;

    assert_eq!(outcomes(&pool, install).await, ["expired"]);
    assert_eq!(outcomes(&pool, authorization).await, ["expired"]);
    assert!(
        outcomes(&pool, credential).await.is_empty(),
        "a credential request holds no engine state, so a restart leaves it open"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn a_plugin_request_without_staging_is_not_offered() {
    let request = |kind: &str| PendingFormRequest {
        thread_id: Uuid::nil(),
        request_id: Uuid::nil(),
        event: serde_json::json!({ "type": kind }),
    };
    let nothing_staged = |_: &str, _: Uuid| false;
    assert!(!is_answerable(
        &request("PluginInstallRequested"),
        nothing_staged
    ));
    assert!(!is_answerable(
        &request("PluginUninstallRequested"),
        nothing_staged
    ));
    assert!(is_answerable(&request("PluginInstallRequested"), |_, _| {
        true
    }));
    assert!(
        is_answerable(&request("CredentialRequested"), nothing_staged),
        "a credential request rests on no engine memory"
    );
}
