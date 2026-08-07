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
//! * a thread-level **Stop** ending the turn and leaving every subscription
//!   watching, which is what a Stop used to silently destroy
//! * the agent's own two verbs over the routes `lucidos event-waits list` /
//!   `cancel` call: the read, the refusals, and a stand-down that records its
//!   own cause

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

/// The **arming lookback** over the real route: the event lands BEFORE the
/// subscription exists, so nothing will ever wake the thread for it, and the
/// registration response is the only place the caller can learn about it.
///
/// This is the 2026-08-06 failure end to end. A chat thread checked the change
/// list, worked for 84 seconds, then armed a wait 26 seconds after the
/// `ChangeProposed` it wanted had already landed. The wait was armed, the
/// response said "subscribed", and the change was never applied.
///
/// Registered over HTTP because that path is the one no unit test covers: the
/// coding-agent route composes the same text through the same
/// `register_event_wait`, and a report that never reached the JSON body would
/// look exactly like a report that was never generated.
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

    // The event happens first. This is the whole point: it is below the
    // watermark the registration is about to record.
    emit_domain_event(&event_type, "landed while the caller was still working").await;

    let resp = http_client()
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
        message.contains("will NOT wake you"),
        "a forward-only watch is the trap, and it has to be stated: {message}"
    );
    assert!(
        message.contains("Nothing is blocking"),
        "the wait is still armed and the caller may still finish: {message}"
    );

    // A report is not a delivery. The wait is live, unconsumed, and the thread
    // is idle rather than mid-wake.
    await_event_row(&pool, thread_id, "EventWaitStarted", 10).await;
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitDelivered").await,
        0,
        "the lookback reports; only a forward match delivers"
    );
    assert_eq!(thread_status(&pool, thread_id).await, "idle");

    // And the subscription really is still watching: the NEXT one wakes it.
    emit_domain_event(&event_type, "the one the subscription is for").await;
    await_event_row(&pool, thread_id, "EventWaitDelivered", 25).await;
}

/// **Stop cancels the turn only.** The regression that this whole change is
/// about: a Stop on a running turn used to cancel every subscription on the
/// thread. A watch armed at 00:08 died at 02:07 because the user stopped an
/// unrelated turn, with no toast, no transcript line, and the indicator row
/// simply gone.
///
/// Driven end to end rather than as a unit test because the coupling lived in
/// the HTTP handler, and because the half that matters is what happens AFTER:
/// the subscription must still be armed, and must still wake the thread.
#[tokio::test]
async fn stopping_a_turn_leaves_the_thread_s_subscriptions_watching() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let event_type = format!("E2eStopKeepsWatch{}", Uuid::new_v4().simple());
    let thread_id = subscribe_a_thread(&pool, &event_type, "api-event-wait-stop").await;

    // Stop with nothing running. The server has no turn to end, so it honestly
    // reports it did nothing, and the subscription is untouched. Before the
    // fix this cancelled the wait AND reported `canceled: true` for it.
    let resp = http_client()
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

    // Now the reported shape: a Stop aimed at a turn that is actually running.
    // Whether it lands mid-turn is a genuine race here (the mock's reply on a
    // thread that already subscribed is one short line), and the assertions
    // below are deliberately true on both sides of it: the Stop either ends the
    // turn or arrives just after it ended, and neither may touch a
    // subscription. The pre-fix code cancelled on every `/chat/cancel`
    // regardless, which is exactly why both sides of the race catch it.
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

    let resp = http_client()
        .post(format!(
            "{}/api/v1/chat/cancel?thread_id={}",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("cancel request failed");
    assert_eq!(resp.status().as_u16(), 200);

    // Let the turn reach whichever terminator it was going to reach.
    await_event_count(&pool, thread_id, "ResponseGenerated", 1, 25).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitCanceled").await,
        0,
        "Stop ends the turn; the subscription was never holding it"
    );

    // The proof that the subscription is not merely un-cancelled but still
    // ARMED: the event still wakes the thread.
    emit_domain_event(&event_type, "arriving after the Stop").await;
    let delivered = await_event_row(&pool, thread_id, "EventWaitDelivered", 25).await;
    assert_eq!(delivered["event_type"], event_type.as_str());
}

/// The agent's own surface: read this thread's subscriptions, then stand one
/// down. Both routes are what `lucidos event-waits list` / `cancel` call, and
/// the chat agent's `list_event_waits` / `cancel_event_wait` tools reach the
/// same code in process.
///
/// The reported failure is the read half: on 2026-08-06 a thread told the user
/// twice that a watch was armed when it had been dead for two hours, because
/// the only way it could answer was to diff four event types by eye across the
/// whole store.
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
    // Everything the agent was asked for and could not answer.
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
    // Ages spelled out beside the timestamps: a fresh subscription is seconds
    // old, not the whole timeout.
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

    // Both arguments, and neither, are refused rather than defaulted: a bare
    // call must not stop everything, and a no-op must not report success.
    for body in [
        serde_json::json!({}),
        serde_json::json!({ "wait_id": wait_id, "all": true }),
    ] {
        let resp = http_client()
            .post(&cancel_url)
            .json(&body)
            .send()
            .await
            .expect("cancel request failed");
        assert_eq!(resp.status(), 400, "ambiguous cancel must be refused");
    }

    // A `wait_id` that is not live on THIS thread is refused, not obeyed. Both
    // verbs are scoped to the calling thread and take no thread argument, so
    // this is the only shape a cross-thread attempt can take.
    let resp = http_client()
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

    // The real stand-down.
    let resp = http_client()
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

    // Its own cause, so an agent stand-down is distinguishable in the event log
    // from a user pressing Stop waiting, from an archive, and from a timeout.
    let canceled = await_event_row(&pool, thread_id, "EventWaitCanceled", 10).await;
    assert_eq!(canceled["cause"], "agent_stand_down");
    // Self-contained, so the transcript entry can name what was stopped even
    // when the registration is outside the loaded window.
    assert_eq!(canceled["on"][0]["event_type"], event_type.as_str());
    assert!(canceled["reason"]
        .as_str()
        .unwrap_or_default()
        .contains(&event_type));

    // Nothing is watching any more, and the read says so in those words.
    let resp = http_client()
        .get(&list_url)
        .send()
        .await
        .expect("list request failed");
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["count"], 0);

    // And it really is stood down: the event no longer wakes the thread.
    emit_domain_event(&event_type, "arriving after the stand-down").await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert_eq!(
        count_events(&pool, thread_id, "EventWaitDelivered").await,
        0,
        "a stopped subscription does not fire"
    );
}
