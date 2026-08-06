//! E2E for the *event wait* (ADR 0047, reshaped by ADR 0049): a chat thread
//! subscribes through `await_event` and the engine re-opens it when a matching
//! event arrives.
//!
//! This is the one behaviour in the feature that CANNOT be covered by seeding
//! events the way `agent_question_test` and `load_knowhow_dedup_test` do. The
//! whole thing is what the engine does at the moment the tool call arrives:
//! the wait is registered, the dispatcher takes over, and delivery re-enters
//! the thread as a new turn. Seeding the aftermath would prove nothing about
//! any of it, so the subscription is driven through a real tool call, with the
//! mock provider's `MOCK_SUBSCRIBE_ON:<EventType>` sentinel standing in for a
//! model that decided to wait (see `llm::mock::scripted_await_event`).
//!
//! What it pins, end to end and against a real engine:
//!
//! * registration (`EventWaitStarted`), the `await_event` call PAIRED with its
//!   own result, and a turn that terminates normally and leaves the thread
//!   `idle`, which is what makes a subscription not a park
//! * delivery on a domain event emitted over HTTP, and the woken turn that
//!   follows, proving the message array the provider saw was well-formed
//! * one-shot: a second matching event resolves nothing
//! * an unrelated event resolving nothing, then Stop waiting cancelling
//! * the HTTP registration route, the one a coding agent reaches through
//!   `lucidos await-event`

use crate::support::{
    base_url, db_url, http_client, poll_thread_summary_by_marker, unique_marker, user_client,
};
use uuid::Uuid;

/// Poll a thread's events for one of `types`, returning the first row found.
/// Fails the test with the thread's actual event list, which is the thing you
/// want in the output when a wake did not happen.
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

/// Poll until `thread_id` has at least `want` events of `event_type`. Fails
/// with the count actually reached, which is what you need in the output.
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

/// Send a chat message that makes the mock subscribe to `event_type`, and wait
/// until the subscription is live AND the turn has finished. Returns the thread
/// id.
///
/// Waiting for the terminator, not just for `EventWaitStarted`, is what makes
/// the callers' `idle` assertions meaningful: registration happens mid-turn, so
/// the thread is legitimately `running` for a moment after it.
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

/// Emit a workspace domain event, the same way an app or a script would.
async fn emit_domain_event(event_type: &str, summary: &str) {
    let resp = http_client()
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

/// The headline case: subscribe, emit, wake. Everything else in this file is a
/// variation on it.
#[tokio::test]
async fn a_subscribed_thread_wakes_when_its_event_arrives() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    // A name nothing else in the suite emits, so a concurrent test cannot
    // resolve this wait out from under it.
    let event_type = format!("E2eParkTarget{}", Uuid::new_v4().simple());
    let thread_id = subscribe_a_thread(&pool, &event_type, "api-event-wait").await;

    // Subscribed and FINISHED. Both halves are the 2026-08-06 change: the turn
    // ended normally, and the thread is plain idle rather than carrying a
    // status of its own.
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
    // The call is PAIRED. An unpaired `tool_use` is a provider 400 on the very
    // next turn, which is the whole reason the attached shape is gone.
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

    // The wake arrives as a new turn, anchored on a `UserPromptInjected`
    // carrying the event as prose. That the provider accepted the rebuilt
    // message array is the point.
    let anchor = await_event_row(&pool, thread_id, "UserPromptInjected", 25).await;
    assert!(
        anchor["text"]
            .as_str()
            .unwrap_or_default()
            .contains(&event_type),
        "the delivered event travels in the wake prompt: {anchor:?}"
    );
    // Poll rather than read once: the woken turn is still running when its
    // anchor lands, so its terminator arrives a beat later.
    await_event_count(&pool, thread_id, "ResponseGenerated", 2, 25).await;

    let status = thread_status(&pool, thread_id).await;
    assert!(
        status == "idle" || status == "running",
        "a woken thread settles like any other turn, got {status}"
    );
}

/// A wait is a rendezvous, not a stream. A tool call has exactly one result,
/// so a second matching event must resolve nothing.
#[tokio::test]
async fn a_second_matching_event_wakes_the_thread_only_once() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eOneShot{}", Uuid::new_v4().simple());
    let thread_id = subscribe_a_thread(&pool, &event_type, "api-event-wait-once").await;

    emit_domain_event(&event_type, "first").await;
    await_event_row(&pool, thread_id, "EventWaitDelivered", 25).await;
    await_event_row(&pool, thread_id, "UserPromptInjected", 25).await;

    emit_domain_event(&event_type, "second, after the wait was consumed").await;
    // Long enough that a second delivery would have landed: the first one took
    // well under this, and the dispatcher acts on the bus event immediately.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    assert_eq!(
        count_events(&pool, thread_id, "EventWaitDelivered").await,
        1,
        "the wait was consumed by the first match"
    );
    assert_eq!(
        count_events(&pool, thread_id, "UserPromptInjected").await,
        1,
        "one subscription, one wake"
    );
}

/// An event nobody is waiting for must not disturb a thread subscribed to a
/// different one. Guards the matcher, and the shared-cache design behind it.
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
        count_events(&pool, thread_id, "UserPromptInjected").await,
        0,
        "still watching for its own event, and nothing woke it"
    );

    // Leave no live subscription behind for the rest of the suite.
    let wait_id = await_event_row(&pool, thread_id, "EventWaitStarted", 5).await["wait_id"]
        .as_str()
        .expect("wait_id")
        .to_string();
    let resp = http_client()
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

/// The coding-agent route. A coding agent has no `await_event` LLM tool (the
/// engine does not own its tool set), so it registers over HTTP through
/// `lucidos await-event`. This pins the endpoint the CLI calls, and that a
/// refusal comes back as a readable 400 rather than a 500.
///
/// It registers on a chat thread rather than standing up a coding-agent
/// session, because the route is agent-agnostic by construction: it calls the
/// same `register_event_wait` the tool does, and which lane the eventual
/// delivery takes is decided later, at wake time, from the thread's own row.
#[tokio::test]
async fn a_wait_can_be_registered_over_http() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eHttpWait{}", Uuid::new_v4().simple());

    // A thread to hang it on. Any finished chat thread will do. The setup send
    // goes through `user_client` because it claims `mode: "human"`, which the
    // engine accepts only from a registered device; the registration POST under
    // test below is unaffected either way.
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
    let resp = http_client()
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

    // A refusal reaches the caller as its own words, with a 400. Re-registering
    // the identical subscription is the cheapest one to provoke.
    let resp = http_client()
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

    // An unknown thread is a 404, not a wait armed against nothing. This is the
    // one caller that can get the id wrong: the LLM tool's comes from
    // `execute_tool`, while a CLI caller passes `$LUCIDOS_THREAD_ID`.
    let resp = http_client()
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

    emit_domain_event(&event_type, "waking the HTTP-registered wait").await;
    await_event_row(&pool, thread_id, "EventWaitDelivered", 25).await;
    await_event_row(&pool, thread_id, "UserPromptInjected", 25).await;
}
