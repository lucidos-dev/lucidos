use crate::support::{
    base_url, db_url, http_client, poll_thread_summary_by_marker, unique_marker, user_client,
};
use uuid::Uuid;

async fn await_event_row(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event_type: &str,
    max_secs: u64,
) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(max_secs);
    loop {
        let row: Option<(serde_json::Value,)> = sqlx::query_as(
            "SELECT payload FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
               AND event_type = $2 ORDER BY sequence LIMIT 1",
        )
        .bind(thread_id.to_string())
        .bind(event_type)
        .fetch_optional(pool)
        .await
        .expect("DB query failed");
        if let Some((payload,)) = row {
            return payload;
        }
        if std::time::Instant::now() >= deadline {
            let seen: Vec<String> = sqlx::query_scalar(
                "SELECT event_type FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
                 ORDER BY sequence",
            )
            .bind(thread_id.to_string())
            .fetch_all(pool)
            .await
            .unwrap_or_default();
            panic!("no {event_type} on thread {thread_id} within {max_secs}s; saw: {seen:?}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn await_event_count(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event_type: &str,
    want: i64,
    max_secs: u64,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(max_secs);
    loop {
        let have = count_events(pool, thread_id, event_type).await;
        if have >= want {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "thread {thread_id} reached {have} {event_type} event(s), wanted {want}, \
                 within {max_secs}s"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn count_events(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
           AND event_type = $2",
    )
    .bind(thread_id.to_string())
    .bind(event_type)
    .fetch_one(pool)
    .await
    .expect("DB query failed")
}

async fn thread_status(pool: &sqlx::PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("DB query failed")
}

async fn subscribe_a_thread(pool: &sqlx::PgPool, event_type: &str, label: &str) -> Uuid {
    let client = user_client().await;
    let marker = unique_marker(label);
    let body = serde_json::json!({
        "message": format!("{marker} MOCK_SUBSCRIBE_ON:{event_type}"),
        "mode": "human",
    });
    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&body)
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200, "chat/stream should accept the message");

    let thread_id = poll_thread_summary_by_marker(pool, &marker, 20)
        .await
        .thread_id;
    await_event_row(pool, thread_id, "EventWaitStarted", 25).await;
    await_event_row(pool, thread_id, "ResponseGenerated", 25).await;
    thread_id
}

async fn emit_domain_event(event_type: &str, summary: &str) {
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/events/emit", base_url()))
        .json(&serde_json::json!({
            "event_type": event_type,
            "payload": { "summary": summary },
        }))
        .send()
        .await
        .expect("emit request failed");
    assert_eq!(resp.status(), 200, "events/emit should accept the event");
}

#[tokio::test]
async fn a_subscribed_thread_wakes_when_its_event_arrives() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eParkTarget{}", Uuid::new_v4().simple());
    let thread_id = subscribe_a_thread(&pool, &event_type, "api-event-wait").await;

    assert_eq!(
        thread_status(&pool, thread_id).await,
        "idle",
        "a subscription does not hold the turn, so the thread is ordinary idle"
    );
    assert_eq!(
        count_events(&pool, thread_id, "ResponseAborted").await,
        0,
        "nothing was interrupted, so no safety net may synthesize an abort"
    );
    assert_eq!(count_events(&pool, thread_id, "ToolCalled").await, 1);
    let registered = await_event_row(&pool, thread_id, "ToolResult", 5).await;
    assert_eq!(registered["name"], "await_event");
    assert_eq!(
        count_events(&pool, thread_id, "ToolResult").await,
        1,
        "await_event pairs its own call at registration"
    );

    emit_domain_event(&event_type, "the thing the thread was waiting for").await;

    let delivered = await_event_row(&pool, thread_id, "EventWaitDelivered", 25).await;
    assert_eq!(delivered["event_type"], event_type.as_str());

    let anchor = await_event_row(&pool, thread_id, "PromptInjected", 25).await;
    assert!(
        anchor["text"]
            .as_str()
            .unwrap_or_default()
            .contains(&event_type),
        "the delivered event travels in the re-entry prompt: {anchor:?}"
    );
    await_event_count(&pool, thread_id, "ResponseGenerated", 2, 25).await;

    let status = thread_status(&pool, thread_id).await;
    assert!(
        status == "idle" || status == "running",
        "a re-entered thread settles like any other turn, got {status}"
    );
}

#[tokio::test]
async fn a_second_matching_event_delivers_only_once() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eOneShot{}", Uuid::new_v4().simple());
    let thread_id = subscribe_a_thread(&pool, &event_type, "api-event-wait-once").await;

    emit_domain_event(&event_type, "first").await;
    await_event_row(&pool, thread_id, "EventWaitDelivered", 25).await;
    await_event_row(&pool, thread_id, "PromptInjected", 25).await;

    emit_domain_event(&event_type, "second, after the wait was consumed").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    assert_eq!(
        count_events(&pool, thread_id, "EventWaitDelivered").await,
        1,
        "the wait was consumed by the first match"
    );
    assert_eq!(
        count_events(&pool, thread_id, "PromptInjected").await,
        1,
        "one subscription, one delivery"
    );
}

#[tokio::test]
async fn an_unrelated_event_leaves_a_subscribed_thread_subscribed() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eNoMatch{}", Uuid::new_v4().simple());
    let thread_id = subscribe_a_thread(&pool, &event_type, "api-event-wait-nomatch").await;

    emit_domain_event(
        &format!("E2eSomethingElse{}", Uuid::new_v4().simple()),
        "not what this thread asked for",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    assert_eq!(
        count_events(&pool, thread_id, "EventWaitDelivered").await,
        0,
    );
    assert_eq!(
        count_events(&pool, thread_id, "PromptInjected").await,
        0,
        "still watching for its own event, and nothing delivered to it"
    );

    let wait_id = await_event_row(&pool, thread_id, "EventWaitStarted", 5).await["wait_id"]
        .as_str()
        .expect("wait_id")
        .to_string();
    let resp = user_client()
        .await
        .post(format!(
            "{}/api/v1/threads/{}/event-waits/{}/cancel",
            base_url(),
            thread_id,
            wait_id
        ))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status(), 200, "Stop waiting should cancel the wait");
    await_event_row(&pool, thread_id, "EventWaitCanceled", 10).await;
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitDelivered").await,
        0,
        "a cancel is not a delivery: the thread settles rather than resuming"
    );
}

#[tokio::test]
async fn the_reload_snapshot_carries_the_event_wait_payloads() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eSnapshot{}", Uuid::new_v4().simple());
    let thread_id = subscribe_a_thread(&pool, &event_type, "api-event-wait-snapshot").await;

    let wait_id = await_event_row(&pool, thread_id, "EventWaitStarted", 5).await["wait_id"]
        .as_str()
        .expect("wait_id")
        .to_string();
    let resp = user_client()
        .await
        .post(format!(
            "{}/api/v1/threads/{}/event-waits/{}/cancel",
            base_url(),
            thread_id,
            wait_id
        ))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status(), 200, "Stop waiting should cancel the wait");
    await_event_row(&pool, thread_id, "EventWaitCanceled", 10).await;

    let snapshot: serde_json::Value = http_client()
        .get(format!(
            "{}/api/v1/threads/{}/events",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("snapshot request failed")
        .json()
        .await
        .expect("snapshot should be JSON");
    let rows = snapshot["events"]
        .as_array()
        .expect("snapshot carries an events array");
    let of_type = |t: &str| {
        rows.iter()
            .find(|r| r["event_type"] == t)
            .unwrap_or_else(|| panic!("no {t} in the snapshot; saw {rows:?}"))
            .clone()
    };

    let started = of_type("EventWaitStarted");
    assert_eq!(started["payload"]["wait_id"], wait_id.as_str());
    assert_eq!(
        started["payload"]["on"][0]["event_type"],
        event_type.as_str(),
        "the row names its subscription"
    );
    assert!(
        started["payload"]["reason"].is_string(),
        "the row names the model's own reason: {started:?}"
    );
    assert!(
        started["payload"]["expires_at"].is_string(),
        "the row carries its deadline: {started:?}"
    );

    let canceled = of_type("EventWaitCanceled");
    assert_eq!(canceled["payload"]["wait_id"], wait_id.as_str());
    assert_eq!(canceled["payload"]["cause"], "user_stop");
    assert_eq!(
        canceled["payload"]["on"][0]["event_type"],
        event_type.as_str(),
        "a stop is self-contained: it says what it stopped"
    );
}

#[tokio::test]
async fn a_wait_can_be_registered_over_http() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eHttpWait{}", Uuid::new_v4().simple());

    let marker = unique_marker("api-event-wait-http");
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({ "message": marker, "mode": "human" }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200);
    let thread_id = poll_thread_summary_by_marker(&pool, &marker, 20)
        .await
        .thread_id;
    await_event_row(&pool, thread_id, "ResponseGenerated", 25).await;

    let url = format!("{}/api/v1/threads/{}/event-waits", base_url(), thread_id);
    let resp = user_client()
        .await
        .post(&url)
        .json(&serde_json::json!({
            "on": [{ "event_type": event_type }],
            "timeout_secs": 300,
            "reason": "e2e: registering over HTTP the way a coding agent does",
        }))
        .send()
        .await
        .expect("register request failed");
    assert_eq!(resp.status(), 200, "the route should accept a valid wait");
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["status"], "subscribed");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Nothing is blocking"),
        "the caller is told it may finish: {body:?}"
    );

    await_event_row(&pool, thread_id, "EventWaitStarted", 10).await;
    assert_eq!(
        thread_status(&pool, thread_id).await,
        "idle",
        "registering over HTTP must not put the thread into a waiting state either"
    );

    let resp = user_client()
        .await
        .post(&url)
        .json(&serde_json::json!({
            "on": [{ "event_type": event_type }],
            "timeout_secs": 300,
            "reason": "e2e: the same subscription twice",
        }))
        .send()
        .await
        .expect("duplicate register request failed");
    assert_eq!(resp.status(), 400, "a duplicate subscription is refused");
    let text = resp.text().await.unwrap_or_default();
    assert!(
        text.contains("already waiting"),
        "the refusal carries the reason the agent must act on: {text}"
    );

    let resp = user_client()
        .await
        .post(format!(
            "{}/api/v1/threads/{}/event-waits",
            base_url(),
            Uuid::new_v4()
        ))
        .json(&serde_json::json!({
            "on": [{ "event_type": event_type }],
            "timeout_secs": 300,
            "reason": "e2e: a thread that does not exist",
        }))
        .send()
        .await
        .expect("unknown-thread register request failed");
    assert_eq!(resp.status(), 404, "an unknown thread must not arm a wait");

    emit_domain_event(&event_type, "delivering to the HTTP-registered wait").await;
    await_event_row(&pool, thread_id, "EventWaitDelivered", 25).await;
    await_event_row(&pool, thread_id, "PromptInjected", 25).await;
}

#[tokio::test]
async fn a_registration_reports_a_match_that_landed_before_it() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eLookback{}", Uuid::new_v4().simple());

    let marker = unique_marker("api-event-wait-lookback");
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({ "message": marker, "mode": "human" }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200);
    let thread_id = poll_thread_summary_by_marker(&pool, &marker, 20)
        .await
        .thread_id;
    await_event_row(&pool, thread_id, "ResponseGenerated", 25).await;

    emit_domain_event(&event_type, "landed while the caller was still working").await;

    let resp = user_client()
        .await
        .post(format!(
            "{}/api/v1/threads/{}/event-waits",
            base_url(),
            thread_id
        ))
        .json(&serde_json::json!({
            "on": [{ "event_type": event_type }],
            "timeout_secs": 300,
            "reason": "e2e: subscribing to something that already happened",
        }))
        .send()
        .await
        .expect("register request failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    let message = body["message"].as_str().unwrap_or_default();

    assert!(
        message.contains("ALREADY HAPPENED"),
        "the caller must be told, or it finishes with the thing unhandled: {message}"
    );
    assert!(
        message.contains(&event_type),
        "the report names the event: {message}"
    );
    assert!(
        message.contains("will NOT deliver anything below"),
        "a forward-only watch is the trap, and it has to be stated: {message}"
    );
    assert!(
        message.contains("Nothing is blocking"),
        "the wait is still armed and the caller may still finish: {message}"
    );

    await_event_row(&pool, thread_id, "EventWaitStarted", 10).await;
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitDelivered").await,
        0,
        "the lookback reports; only a forward match delivers"
    );
    assert_eq!(thread_status(&pool, thread_id).await, "idle");

    emit_domain_event(&event_type, "the one the subscription is for").await;
    await_event_row(&pool, thread_id, "EventWaitDelivered", 25).await;
}

#[tokio::test]
async fn stopping_a_turn_leaves_the_thread_s_subscriptions_watching() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eStopKeepsWatch{}", Uuid::new_v4().simple());
    let thread_id = subscribe_a_thread(&pool, &event_type, "api-event-wait-stop").await;

    let resp = user_client()
        .await
        .post(format!(
            "{}/api/v1/chat/cancel?thread_id={}",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(
        body["canceled"], false,
        "an idle thread has no turn to stop, and a subscription is not one: {body:?}"
    );

    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": "keep talking while I stop you",
            "mode": "human",
            "thread_id": thread_id.to_string(),
        }))
        .send()
        .await
        .expect("follow-up request failed");
    assert_eq!(resp.status(), 200);

    let resp = user_client()
        .await
        .post(format!(
            "{}/api/v1/chat/cancel?thread_id={}",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status().as_u16(), 200);

    await_event_count(&pool, thread_id, "ResponseGenerated", 1, 25).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitCanceled").await,
        0,
        "Stop ends the turn; the subscription was never holding it"
    );

    emit_domain_event(&event_type, "arriving after the Stop").await;
    let delivered = await_event_row(&pool, thread_id, "EventWaitDelivered", 25).await;
    assert_eq!(delivered["event_type"], event_type.as_str());
}

#[tokio::test]
async fn an_agent_can_read_and_stand_down_its_own_subscriptions() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eAgentSurface{}", Uuid::new_v4().simple());
    let thread_id = subscribe_a_thread(&pool, &event_type, "api-event-wait-agent").await;
    let list_url = format!("{}/api/v1/threads/{}/event-waits", base_url(), thread_id);

    let resp = http_client()
        .get(&list_url)
        .send()
        .await
        .expect("list request failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["count"], 1);
    let entry = &body["event_waits"][0];
    assert_eq!(entry["on"][0]["event_type"], event_type.as_str());
    assert_eq!(entry["subscription"], event_type.as_str());
    assert!(
        entry["reason"]
            .as_str()
            .unwrap_or_default()
            .contains(&event_type),
        "the reason the subscription was armed with: {entry}"
    );
    assert!(entry["wait_id"].is_string(), "{entry}");
    assert!(entry["armed_at"].is_string(), "{entry}");
    assert!(entry["expires_at"].is_string(), "{entry}");
    assert!(
        entry["armed_ago"]
            .as_str()
            .unwrap_or_default()
            .ends_with('s'),
        "armed seconds ago, not hours: {entry}"
    );
    let wait_id = entry["wait_id"].as_str().expect("wait_id").to_string();

    let cancel_url = format!(
        "{}/api/v1/threads/{}/event-waits/cancel",
        base_url(),
        thread_id
    );

    for body in [
        serde_json::json!({}),
        serde_json::json!({ "wait_id": wait_id, "all": true }),
    ] {
        let resp = user_client()
            .await
            .post(&cancel_url)
            .json(&body)
            .send()
            .await
            .expect("cancel request failed");
        assert_eq!(resp.status(), 400, "ambiguous cancel must be refused");
    }

    let resp = user_client()
        .await
        .post(&cancel_url)
        .json(&serde_json::json!({ "wait_id": Uuid::new_v4() }))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status(), 400);
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitCanceled").await,
        0,
        "a refused cancel stops nothing"
    );

    let resp = user_client()
        .await
        .post(&cancel_url)
        .json(&serde_json::json!({ "wait_id": wait_id }))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["status"], "stopped");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains(&event_type),
        "the result names what it stopped: {body:?}"
    );

    let canceled = await_event_row(&pool, thread_id, "EventWaitCanceled", 10).await;
    assert_eq!(canceled["cause"], "agent_stand_down");
    assert_eq!(canceled["on"][0]["event_type"], event_type.as_str());
    assert!(canceled["reason"]
        .as_str()
        .unwrap_or_default()
        .contains(&event_type));

    let resp = http_client()
        .get(&list_url)
        .send()
        .await
        .expect("list request failed");
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["count"], 0);

    emit_domain_event(&event_type, "arriving after the stand-down").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitDelivered").await,
        0,
        "a stopped subscription does not fire"
    );
}

#[tokio::test]
async fn an_agent_can_stand_down_by_event_type_without_touching_the_rest() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let answered = format!("E2eAnswered{}", Uuid::new_v4().simple());
    let unrelated = format!("E2eUnrelated{}", Uuid::new_v4().simple());
    let thread_id = subscribe_a_thread(&pool, &answered, "api-event-wait-on").await;

    let register_url = format!("{}/api/v1/threads/{}/event-waits", base_url(), thread_id);
    let resp = user_client()
        .await
        .post(&register_url)
        .json(&serde_json::json!({
            "on": [{ "event_type": unrelated }],
            "timeout_secs": 300,
            "reason": "e2e: the watch a by-type stand-down must leave alone",
        }))
        .send()
        .await
        .expect("register request failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(count_events(&pool, thread_id, "EventWaitStarted").await, 2);

    let cancel_url = format!(
        "{}/api/v1/threads/{}/event-waits/cancel",
        base_url(),
        thread_id
    );

    for body in [
        serde_json::json!({ "on": answered, "all": true }),
        serde_json::json!({ "on": answered, "wait_id": Uuid::new_v4() }),
    ] {
        let resp = user_client()
            .await
            .post(&cancel_url)
            .json(&body)
            .send()
            .await
            .expect("cancel request failed");
        assert_eq!(resp.status(), 400, "over-specified cancel must be refused");
    }

    let resp = user_client()
        .await
        .post(&cancel_url)
        .json(&serde_json::json!({ "on": "E2eNobodyWatchesThis" }))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status(), 400);
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitCanceled").await,
        0,
        "a refused cancel stops nothing"
    );

    let resp = user_client()
        .await
        .post(&cancel_url)
        .json(&serde_json::json!({ "on": answered }))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["status"], "stopped");
    let message = body["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains(&answered),
        "names what it stopped: {message}"
    );
    assert!(
        !message.contains("Nothing is subscribed"),
        "an on-stop that left a watch standing must not report a clean sweep: {message}"
    );
    assert!(
        message.contains("1 other subscription(s)"),
        "and it counts the survivor: {message}"
    );

    let canceled = await_event_row(&pool, thread_id, "EventWaitCanceled", 10).await;
    assert_eq!(canceled["cause"], "agent_stand_down");
    assert_eq!(canceled["on"][0]["event_type"], answered.as_str());
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitCanceled").await,
        1,
        "only the watch for that event type was ended"
    );

    let list_url = format!("{}/api/v1/threads/{}/event-waits", base_url(), thread_id);
    let body: serde_json::Value = http_client()
        .get(&list_url)
        .send()
        .await
        .expect("list request failed")
        .json()
        .await
        .expect("JSON body");
    assert_eq!(body["count"], 1);
    assert_eq!(
        body["event_waits"][0]["on"][0]["event_type"],
        unrelated.as_str()
    );

    emit_domain_event(&answered, "arriving after the by-type stand-down").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitDelivered").await,
        0,
        "the stopped subscription does not fire"
    );
    emit_domain_event(&unrelated, "the watch that was left alone").await;
    let delivered = await_event_row(&pool, thread_id, "EventWaitDelivered", 25).await;
    assert_eq!(delivered["event_type"], unrelated.as_str());
}
