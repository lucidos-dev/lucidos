//! E2E for a pinned trigger's route reaching every turn it starts.
//!
//! A trigger pinned to one model once ran an intent on another: the intent
//! sub-loop called the model with no selection, so it fell to the boot default.
//! Here a pinned trigger's mock script loads an intent and spawns a child. Every
//! turn in both threads must run on the pin.
//!
//! The mock answers any model it is asked for. So the test reads the route that
//! each `ContextCaptured` row and the child's `MessageReceived` recorded.

use crate::support::{
    db_url, remove_data_fixtures, unique_marker, user_client, write_data_fixture,
};
use crate::trigger_run_test::{create_trigger, delete_trigger, run_trigger_with_a_free_slot};
use serde_json::json;
use std::time::{Duration, Instant};

/// A schedule that never fires inside a test run, so the off-schedule run is
/// the only fire.
const NEVER_SOON_CRON: &str = "0 0 3 1 1 *";

/// The trigger's pin. Neither is the e2e engine's boot default, `mock`.
const PINNED_MODEL: &str = "claude-sonnet-5";
const PINNED_EFFORT: &str = "low";

/// What the two threads recorded, read once both turns ended.
#[derive(Debug)]
struct Recorded {
    /// `(model, reasoning_effort)` of every turn capture in both threads.
    turn_routes: Vec<(String, Option<String>)>,
    /// `(model, reasoning_effort)` the child's first message was stamped with.
    child_message_route: (Option<String>, Option<String>),
    /// Captures of the retired intent sub-loop, in either thread.
    intent_loop_captures: i64,
}

/// The fire's thread and its child, once both have ended a turn.
async fn ended_threads(
    pool: &sqlx::PgPool,
    trigger_id: &str,
) -> Result<(uuid::Uuid, uuid::Uuid), String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let parent: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT thread_id FROM events \
             WHERE event_type = 'TriggerStarted' \
               AND payload->'origin'->'reason'->>'trigger_id' = $1 \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(trigger_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("query TriggerStarted: {e}"))?;
        let child: Option<uuid::Uuid> = match parent {
            Some(parent) => sqlx::query_scalar(
                "SELECT thread_id FROM thread_summaries WHERE parent_thread_id = $1 LIMIT 1",
            )
            .bind(parent)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("query the child: {e}"))?,
            None => None,
        };
        if let (Some(parent), Some(child)) = (parent, child) {
            let ended: i64 = sqlx::query_scalar(
                "SELECT COUNT(DISTINCT thread_id) FROM events \
                 WHERE thread_id = ANY($1) \
                   AND event_type IN ('ResponseGenerated', 'ResponseCanceled', \
                                      'ResponseAborted', 'ResponseFailed')",
            )
            .bind(vec![parent, child])
            .fetch_one(pool)
            .await
            .map_err(|e| format!("query terminators: {e}"))?;
            if ended == 2 {
                return Ok((parent, child));
            }
        }
        if Instant::now() > deadline {
            return Err(format!(
                "the fire and its child did not both end a turn \
                 (parent {parent:?}, child {child:?})"
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Read what both threads recorded. Returns a `Result` so the caller can clean
/// up before it unwraps.
async fn read_recorded(trigger_id: &str) -> Result<Recorded, String> {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .map_err(|e| format!("connect to the e2e workspace database: {e}"))?;
    let result = async {
        let (parent, child) = ended_threads(&pool, trigger_id).await?;
        let threads = vec![parent, child];
        let turn_routes = sqlx::query_as(
            "SELECT payload->>'model', payload->>'reasoning_effort' FROM events \
             WHERE thread_id = ANY($1) AND event_type = 'ContextCaptured' \
               AND payload->>'producer' = 'main_llm' \
             ORDER BY sequence",
        )
        .bind(&threads)
        .fetch_all(&pool)
        .await
        .map_err(|e| format!("query turn captures: {e}"))?;
        let child_message_route = sqlx::query_as(
            "SELECT payload->>'model', payload->>'reasoning_effort' FROM events \
             WHERE thread_id = $1 AND event_type = 'MessageReceived' \
             ORDER BY sequence LIMIT 1",
        )
        .bind(child)
        .fetch_one(&pool)
        .await
        .map_err(|e| format!("query the child's MessageReceived: {e}"))?;
        let intent_loop_captures = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events \
             WHERE thread_id = ANY($1) AND event_type = 'ContextCaptured' \
               AND payload->>'purpose' = 'intent_loop'",
        )
        .bind(&threads)
        .fetch_one(&pool)
        .await
        .map_err(|e| format!("query intent_loop captures: {e}"))?;
        Ok(Recorded {
            turn_routes,
            child_message_route,
            intent_loop_captures,
        })
    }
    .await;
    pool.close().await;
    result
}

#[tokio::test]
async fn a_pinned_trigger_runs_its_intent_and_its_child_on_the_pin() {
    let client = user_client().await;
    let slug = unique_marker("pinned-route");
    let intent_id = unique_marker("pinned-route-intent");
    let intent_file = format!("triggers/{intent_id}/{intent_id}.md");
    {
        let _tree = crate::support::workspace_tree_lock().read().await;
        write_data_fixture(
            &client,
            &intent_file,
            "---\nname: Pinned route probe\n---\nSpawn a child that counts the files.\n",
        )
        .await
        .expect("write the intent fixture");
    }

    let load = json!({ "id": intent_id });
    let spawn = json!({ "prompt": "count the files", "wake_mode": "no-wake" });
    let id = create_trigger(
        &client,
        &format!("Pinned route {slug}"),
        &slug,
        json!({
            "type": "intent",
            "intent": format!(
                "MOCK_TOOL_CALL: load_knowhow {load} MOCK_TOOL_CALL: run_thread {spawn}"
            ),
        }),
        json!({
            "cron_expressions": [NEVER_SOON_CRON],
            "model": PINNED_MODEL,
            "reasoning_effort": PINNED_EFFORT,
        }),
    )
    .await;

    let recorded = match run_trigger_with_a_free_slot(&client, &id).await {
        Ok(run) if run["success"] == true => read_recorded(&id).await,
        Ok(run) => Err(format!("run refused: {run}")),
        Err(e) => Err(e),
    };

    // Every unwrap waits for the cleanup: a panic before it strands a trigger
    // and its intent in the shared workspace.
    delete_trigger(&client, &id).await;
    remove_data_fixtures(&client, &format!("triggers/{intent_id}"), &[intent_file]).await;

    let recorded = recorded.expect("fire the pinned trigger");
    let pinned = (PINNED_MODEL.to_string(), Some(PINNED_EFFORT.to_string()));
    assert!(
        recorded.turn_routes.len() >= 4,
        "the fire takes three rounds and the child at least one: {recorded:?}"
    );
    assert!(
        recorded.turn_routes.iter().all(|route| *route == pinned),
        "every turn in both threads must run on the pin: {recorded:?}"
    );
    assert_eq!(
        recorded.child_message_route,
        (
            Some(PINNED_MODEL.to_string()),
            Some(PINNED_EFFORT.to_string())
        ),
        "the child must be stamped with its parent's route"
    );
    assert_eq!(recorded.intent_loop_captures, 0, "no intent sub-loop ran");
}
