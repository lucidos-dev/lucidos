use super::*;
use chrono::{TimeZone, Utc};
use serde_json::json;

/// A thread's events, built in order with distinct timestamps.
#[derive(Default)]
struct Thread {
    events: Vec<EventRow>,
}

impl Thread {
    fn push(&mut self, event_type: &str, payload: serde_json::Value) -> Uuid {
        let id = Uuid::new_v4();
        let secs = self.events.len() as i64;
        self.events.push(EventRow {
            id,
            event_type: event_type.to_string(),
            payload,
            created: Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap(),
            thread_id: None,
            sequence: None,
        });
        id
    }

    fn tool(&mut self, name: &str, id: &str, result: &str, success: bool) {
        let call = self.push("ToolCalled", json!({"name": name, "args": {"id": id}}));
        self.push(
            "ToolResult",
            json!({
                "name": name,
                "result": result,
                "success": success,
                "tool_called_event_id": call.to_string(),
            }),
        );
    }

    fn load(&mut self, id: &str, body: &str) {
        self.tool(LOAD_KNOWHOW, id, body, true);
    }

    fn keep(&mut self, id: &str) {
        self.tool(LOAD_KNOWHOW, id, &kept_result(id), true);
    }

    fn unload(&mut self, id: &str) {
        self.tool(UNLOAD_KNOWHOW, id, "Unloaded.", true);
    }

    fn message(&mut self) {
        self.push("MessageReceived", json!({"text": "hi"}));
    }

    fn finish_turn(&mut self) {
        self.push(TURN_FINISHED_EVENT, json!({"text": "done"}));
    }

    /// One finished turn that touches no knowhow.
    fn idle_turn(&mut self) {
        self.message();
        self.finish_turn();
    }

    fn loaded(&self) -> Vec<LoadedKnowhow> {
        loaded_knowhow(&self.events)
    }

    fn ids(&self) -> Vec<String> {
        self.loaded().into_iter().map(|d| d.id).collect()
    }

    fn doc(&self, id: &str) -> LoadedKnowhow {
        self.loaded()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("`{id}` is not loaded"))
    }
}

/// A thread whose first turn loaded `alpha`.
fn loaded_in_first_turn() -> Thread {
    let mut t = Thread::default();
    t.message();
    t.load("alpha", "ALPHA BODY");
    t.finish_turn();
    t
}

#[test]
fn a_doc_stays_for_the_limit_of_idle_turns_then_leaves() {
    let mut t = loaded_in_first_turn();
    assert_eq!(t.doc("alpha").idle_turns, 1);
    for _ in 1..IDLE_TURNS_BEFORE_UNLOAD {
        t.idle_turn();
    }
    let last = t.doc("alpha");
    assert_eq!(last.idle_turns, IDLE_TURNS_BEFORE_UNLOAD);
    assert_eq!(
        last.retention_note(),
        "leaves after this turn unless loaded again"
    );

    t.idle_turn();
    assert!(t.ids().is_empty(), "still loaded: {:?}", t.ids());
}

#[test]
fn a_full_reload_inside_the_window_keeps_the_doc() {
    let mut t = loaded_in_first_turn();
    t.idle_turn();
    t.message();
    t.load("alpha", "ALPHA BODY");
    t.finish_turn();
    for _ in 1..IDLE_TURNS_BEFORE_UNLOAD {
        t.idle_turn();
    }
    assert_eq!(t.doc("alpha").idle_turns, IDLE_TURNS_BEFORE_UNLOAD);
}

/// The doc in active use stays, however long the task runs, as long as some
/// turn in each window keeps it.
#[test]
fn a_kept_result_keeps_the_doc_and_its_body() {
    let mut t = loaded_in_first_turn();
    for _ in 0..4 * IDLE_TURNS_BEFORE_UNLOAD {
        t.message();
        t.keep("alpha");
        t.finish_turn();
    }
    let doc = t.doc("alpha");
    assert_eq!(doc.body, "ALPHA BODY");
    assert_eq!(doc.idle_turns, 1);
}

#[test]
fn a_kept_result_for_a_doc_not_loaded_loads_nothing() {
    let mut t = Thread::default();
    t.message();
    t.keep("alpha");
    t.finish_turn();
    assert!(t.ids().is_empty());
}

/// An answered question and a typed injection stay inside the turn. A turn
/// that did not finish (Stop, abort, failure) ages nothing.
#[test]
fn only_a_finished_turn_ages_a_doc() {
    let mut t = loaded_in_first_turn();
    for _ in 0..10 {
        t.message();
        t.push("PromptInjected", json!({"text": "also"}));
        t.push("UserQuestionAsked", json!({}));
        t.push("UserQuestionAnswered", json!({}));
        t.push("ResponseCanceled", json!({}));
        t.push("ResponseAborted", json!({}));
        t.push("ResponseFailed", json!({"error": "boom"}));
    }
    assert_eq!(t.doc("alpha").idle_turns, 1);
}

#[test]
fn unload_drops_the_doc_and_a_reload_brings_it_back() {
    let mut t = loaded_in_first_turn();
    t.message();
    t.unload("alpha");
    t.finish_turn();
    assert!(t.ids().is_empty());

    t.message();
    t.load("alpha", "ALPHA BODY");
    t.finish_turn();
    assert_eq!(t.doc("alpha").idle_turns, 1);
}

/// A re-load works the same way whether the doc was evicted or never loaded.
#[test]
fn a_reload_after_eviction_is_a_first_load() {
    let mut t = loaded_in_first_turn();
    for _ in 0..IDLE_TURNS_BEFORE_UNLOAD {
        t.idle_turn();
    }
    assert!(t.ids().is_empty());

    t.message();
    t.load("alpha", "ALPHA BODY v2");
    t.finish_turn();
    let mut fresh = Thread::default();
    fresh.message();
    fresh.load("alpha", "ALPHA BODY v2");
    fresh.finish_turn();
    assert_eq!(t.loaded(), fresh.loaded());
}

#[test]
fn misses_failures_orphans_and_missing_ids_change_nothing() {
    let mut t = Thread::default();
    t.message();
    t.load(
        "ghost",
        &crate::core::knowhow::knowhow_not_found_body("ghost"),
    );
    t.tool(LOAD_KNOWHOW, "broken", "Error: database unreachable", false);
    t.push(
        "ToolCalled",
        json!({"name": LOAD_KNOWHOW, "args": {"id": "orphan"}}),
    );
    t.tool(LOAD_KNOWHOW, "", "BODY", true);
    t.finish_turn();
    assert!(t.ids().is_empty(), "loaded: {:?}", t.ids());
}

#[test]
fn a_failed_unload_keeps_the_doc() {
    let mut t = loaded_in_first_turn();
    t.message();
    t.tool(
        UNLOAD_KNOWHOW,
        "alpha",
        "Error: database unreachable",
        false,
    );
    t.finish_turn();
    assert_eq!(t.ids(), vec!["alpha".to_string()]);
}

/// The kept result is only safe when the block really carries the body: one
/// that arrived in the running turn is in a live tool result instead.
#[test]
fn the_body_is_in_the_block_only_after_a_finished_turn() {
    let mut t = Thread::default();
    t.message();
    t.load("alpha", "ALPHA BODY");
    assert!(!t.doc("alpha").body_in_block());
    t.finish_turn();
    assert!(t.doc("alpha").body_in_block());

    t.message();
    t.load("alpha", "ALPHA BODY v2");
    let reloaded = t.doc("alpha");
    assert_eq!(reloaded.body, "ALPHA BODY v2");
    assert!(!reloaded.body_in_block());

    t.keep("alpha");
    assert!(!t.doc("alpha").body_in_block(), "a keep moves no body");
}

#[test]
fn the_retention_note_counts_down() {
    let notes: Vec<String> = (1..=IDLE_TURNS_BEFORE_UNLOAD)
        .map(|n| LoadedKnowhow::loaded_turns_ago("a", "b", n).retention_note())
        .collect();
    assert_eq!(
        notes.last().unwrap(),
        "leaves after this turn unless loaded again"
    );
    assert!(notes[..notes.len() - 1]
        .iter()
        .all(|n| n.starts_with("stays ") && n.ends_with(" unless loaded again")));
    let unique: std::collections::HashSet<_> = notes.iter().collect();
    assert_eq!(unique.len(), notes.len(), "{notes:?}");
}

#[test]
fn docs_age_independently_and_come_back_sorted_by_id() {
    let mut t = loaded_in_first_turn();
    t.idle_turn();
    t.message();
    t.load("beta", "BETA BODY");
    t.finish_turn();
    assert_eq!(t.ids(), vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(
        (t.doc("alpha").idle_turns, t.doc("beta").idle_turns),
        (3, 1)
    );

    // Alpha passes the limit while beta, two turns younger, is still inside it.
    for _ in 0..IDLE_TURNS_BEFORE_UNLOAD - 2 {
        t.idle_turn();
    }
    assert_eq!(t.ids(), vec!["beta".to_string()]);
}

/// Legacy rows carry no `tool_called_event_id` and pair by name.
#[test]
fn legacy_results_without_a_call_id_still_pair() {
    let mut t = Thread::default();
    t.message();
    t.push(
        "ToolCalled",
        json!({"name": LOAD_KNOWHOW, "args": {"id": "alpha"}}),
    );
    t.push(
        "ToolResult",
        json!({"name": LOAD_KNOWHOW, "result": "ALPHA BODY"}),
    );
    t.finish_turn();
    assert_eq!(t.doc("alpha").body, "ALPHA BODY");
}

mod persisted {
    //! Against a real database: the narrow query, and a restart.

    use super::*;
    use crate::core::EventStore;
    use crate::engine::event_bus::EventBus;
    use crate::engine::thread_events::{ActorMode, ThreadEvent};
    use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};

    fn message() -> ThreadEvent {
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "hi".to_string(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            parent_thread_id: None,
            spawning_event_id: None,
            origin: None,
        }
    }

    fn finished() -> ThreadEvent {
        ThreadEvent::ResponseGenerated {
            text: "done".to_string(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        }
    }

    async fn tool(bus: &EventBus, thread: Uuid, name: &str, id: &str, result: String) {
        let call = seed_thread_event(
            bus,
            thread,
            ThreadEvent::ToolCalled {
                name: name.to_string(),
                args: json!({"id": id}),
                description: String::new(),
            },
        )
        .await;
        seed_thread_event(
            bus,
            thread,
            ThreadEvent::ToolResult {
                name: name.to_string(),
                result,
                images: vec![],
                success: true,
                tool_called_event_id: Some(call),
            },
        )
        .await;
    }

    async fn turn(bus: &EventBus, thread: Uuid, tools: &[(&str, &str, String)]) {
        seed_thread_event(bus, thread, message()).await;
        for (name, id, result) in tools {
            tool(bus, thread, name, id, result.clone()).await;
        }
        seed_thread_event(bus, thread, finished()).await;
    }

    /// The set has no home but the events, so an engine restart, which keeps
    /// the database and nothing else, finds exactly what was there before.
    #[tokio::test]
    async fn eviction_and_unload_survive_a_restart() {
        let (pool, db) = setup_test_db().await;
        let thread = Uuid::new_v4();
        {
            let (bus, _rx) = EventBus::new(pool.clone());
            let body = |id: &str| format!("[KNOW-HOW: {id}]\nbody\n[END KNOW-HOW]");
            turn(
                &bus,
                thread,
                &[
                    (LOAD_KNOWHOW, "stale", body("stale")),
                    (LOAD_KNOWHOW, "dropped", body("dropped")),
                    (LOAD_KNOWHOW, "kept", body("kept")),
                ],
            )
            .await;
            turn(
                &bus,
                thread,
                &[(UNLOAD_KNOWHOW, "dropped", "Unloaded.".into())],
            )
            .await;
            for _ in 1..IDLE_TURNS_BEFORE_UNLOAD {
                turn(&bus, thread, &[(LOAD_KNOWHOW, "kept", kept_result("kept"))]).await;
            }
            turn(&bus, thread, &[]).await;
        }
        pool.close().await;

        let restarted = sqlx::PgPool::connect(&crate::test_support::test_db_url(&db))
            .await
            .expect("reconnect");
        let store = EventStore::new(restarted.clone());
        let narrow = loaded_knowhow(&store.get_thread_knowhow_events(thread).await.unwrap());
        let ids: Vec<&str> = narrow.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["kept"], "stale was evicted, dropped unloaded");
        assert_eq!(narrow[0].idle_turns, 2);

        let full = loaded_knowhow(&store.get_thread_events(&thread.to_string()).await.unwrap());
        assert_eq!(narrow, full, "the narrow query folds to the same set");

        restarted.close().await;
        teardown_test_db(&db).await;
    }

    /// The narrow query must not fold in another thread's loads, nor drop a
    /// finished turn that has no knowhow call in it.
    #[tokio::test]
    async fn the_narrow_query_reads_only_this_thread() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let (mine, other) = (Uuid::new_v4(), Uuid::new_v4());
        turn(&bus, mine, &[(LOAD_KNOWHOW, "alpha", "ALPHA".into())]).await;
        turn(&bus, other, &[(LOAD_KNOWHOW, "beta", "BETA".into())]).await;
        turn(&bus, other, &[]).await;

        let store = EventStore::new(pool.clone());
        let events = store.get_thread_knowhow_events(mine).await.unwrap();
        let docs = loaded_knowhow(&events);
        assert_eq!(docs.len(), 1);
        assert_eq!((docs[0].id.as_str(), docs[0].idle_turns), ("alpha", 1));
        assert!(events
            .iter()
            .all(|e| e.event_type == TURN_FINISHED_EVENT || e.event_type.starts_with("Tool")));

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
