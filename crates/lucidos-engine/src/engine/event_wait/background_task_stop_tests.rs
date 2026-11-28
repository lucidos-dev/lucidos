use super::*;
use crate::core::event_subscription::matchable_thread_payload;
use crate::engine::bash_background_recovery::completion_event;
use crate::engine::event_wait::{catch_up_from_watermark, waits_matching};
use crate::engine::thread_events::{ActorMode, ThreadEvent};
use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};
use serde_json::json;
use std::path::Path;

pub(super) struct Stores {
    pub(super) pool: sqlx::PgPool,
    db_name: String,
    pub(super) bus: EventBus,
    pub(super) live: LiveWaits,
    pub(super) registry: BackgroundBashRegistry,
}

impl Stores {
    pub(super) async fn new() -> Self {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        Stores {
            pool,
            db_name,
            bus,
            live: LiveWaits::new(),
            registry: BackgroundBashRegistry::new(),
        }
    }

    pub(super) async fn teardown(self) {
        self.pool.close().await;
        teardown_test_db(&self.db_name).await;
    }

    pub(super) async fn thread(&self) -> Uuid {
        let thread_id = Uuid::new_v4();
        seed_thread_event(
            &self.bus,
            thread_id,
            ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "lint it".into(),
                user_image_hashes: vec![],
                device_id: None,
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

    async fn task(&self, thread_id: Uuid, description: &str) -> String {
        let (task_id, _finished) = self
            .registry
            .spawn(
                "sleep 30",
                60,
                Path::new("/tmp"),
                &[],
                Some(thread_id),
                Some(description),
            )
            .await
            .expect("spawn");
        task_id
    }

    async fn wait(
        &self,
        thread_id: Uuid,
        tool_use_id: &str,
        on: Vec<EventSubscription>,
    ) -> LiveWait {
        let watermark = crate::engine::event_bus::committed_event_horizon(&self.pool)
            .await
            .expect("watermark");
        let armed_at = Utc::now();
        let wait = LiveWait {
            background_timing: None,
            wait_id: Uuid::new_v4(),
            thread_id,
            tool_use_id: tool_use_id.to_string(),
            on,
            reason: format!("the wait {tool_use_id}"),
            armed_at,
            expires_at: armed_at + Duration::hours(1),
            watermark,
        };
        super::super::register::persist_wait(&self.bus, &self.live, &wait)
            .await
            .expect("persist");
        wait
    }

    pub(super) async fn completion(
        &self,
        task_id: &str,
        owner: Uuid,
    ) -> (String, serde_json::Value) {
        assert!(
            self.registry
                .wait_for_finish(task_id, std::time::Duration::from_secs(15))
                .await,
            "task {task_id} never finished"
        );
        let record = self
            .registry
            .completion_record(task_id)
            .await
            .expect("record");
        let event = completion_event(task_id.to_string(), record);
        (
            event.event_type().to_string(),
            matchable_thread_payload(
                &event,
                &crate::engine::thread_events::EventMeta::NONE,
                owner,
            ),
        )
    }

    pub(super) async fn woken_by(
        &self,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> Vec<Uuid> {
        waits_matching(&self.live.snapshot().await, event_type, payload)
            .into_iter()
            .map(|(wait_id, _)| wait_id)
            .collect()
    }

    async fn wait_events(&self, thread_id: Uuid) -> Vec<(String, serde_json::Value)> {
        sqlx::query_as(
            "SELECT event_type, payload FROM events \
             WHERE aggregate_id = $1 AND event_type LIKE 'EventWait%' ORDER BY sequence",
        )
        .bind(thread_id.to_string())
        .fetch_all(&self.pool)
        .await
        .expect("query")
    }
}

fn completed(task_id: &str) -> EventSubscription {
    EventSubscription {
        event_type: BACKGROUND_BASH_COMPLETED.to_string(),
        condition: Some(json!({ "task_id": task_id })),
    }
}

#[tokio::test]
async fn a_threads_own_stop_does_not_wake_it() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let lint = s.task(thread, "make lint").await;
    s.wait(thread, &engine_tool_use_id(), vec![completed(&lint)])
        .await;

    let outcome = stop_and_stand_down(&s.registry, &s.bus, &s.live, thread, &lint)
        .await
        .expect("the task was running");
    assert_eq!(outcome.stopped.label, "make lint");
    assert!(outcome.replacements.is_empty());

    let (event_type, payload) = s.completion(&lint, thread).await;
    assert_eq!(payload["killed"], json!(true), "the stop really ended it");
    assert!(
        s.woken_by(&event_type, &payload).await.is_empty(),
        "a thread's own stop must not wake it"
    );
    let events = s.wait_events(thread).await;
    let (last_type, last) = events.last().expect("events");
    assert_eq!(last_type, "EventWaitCanceled");
    assert_eq!(last["cause"], json!("agent_stand_down"));

    s.teardown().await;
}

#[tokio::test]
async fn a_stop_from_another_thread_still_wakes_the_owner() {
    let s = Stores::new().await;
    let owner = s.thread().await;
    let other = s.thread().await;
    let build = s.task(owner, "the release build").await;
    let wait = s
        .wait(owner, &engine_tool_use_id(), vec![completed(&build)])
        .await;

    stop_and_stand_down(&s.registry, &s.bus, &s.live, other, &build)
        .await
        .expect("the task was running");

    let (event_type, payload) = s.completion(&build, owner).await;
    assert_eq!(s.woken_by(&event_type, &payload).await, vec![wait.wait_id]);

    s.teardown().await;
}

#[tokio::test]
async fn a_kill_for_the_whole_thread_still_delivers() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let build = s.task(thread, "the release build").await;
    let wait = s
        .wait(thread, &engine_tool_use_id(), vec![completed(&build)])
        .await;

    assert_eq!(s.registry.kill_for_thread(thread).await, 1);

    let (event_type, payload) = s.completion(&build, thread).await;
    assert_eq!(s.woken_by(&event_type, &payload).await, vec![wait.wait_id]);

    s.teardown().await;
}

#[tokio::test]
async fn stopping_one_task_narrows_the_engines_wait_to_the_rest() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let lint = s.task(thread, "make lint").await;
    let e2e = s.task(thread, "the e2e suite").await;
    let old = s
        .wait(
            thread,
            &engine_tool_use_id(),
            vec![completed(&lint), completed(&e2e)],
        )
        .await;
    seed_thread_event(
        &s.bus,
        thread,
        ThreadEvent::BackgroundBashCompleted {
            task_id: e2e.clone(),
            command: "sleep 30".into(),
            exit_code: Some(0),
            signal: None,
            stdout: String::new(),
            stderr: String::new(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
            timed_out: false,
            killed: false,
            abandoned: false,
        },
    )
    .await;

    let outcome = stop_and_stand_down(&s.registry, &s.bus, &s.live, thread, &lint)
        .await
        .expect("the task was running");
    let [replacement] = outcome.replacements.as_slice() else {
        panic!("expected one replacement, got {:?}", outcome.replacements);
    };
    assert_eq!(replacement.on, vec![completed(&e2e)]);
    assert_eq!(replacement.watermark, old.watermark);
    assert_eq!(replacement.expires_at, old.expires_at);
    assert!(is_engine_armed(replacement));
    assert_eq!(replacement.reason, "the e2e suite to finish");
    assert!(
        catch_up_from_watermark(&s.pool, replacement)
            .await
            .expect("scan")
            .is_some(),
        "the other task's completion must still reach the thread"
    );

    let types: Vec<String> = s
        .wait_events(thread)
        .await
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert_eq!(
        types,
        ["EventWaitStarted", "EventWaitStarted", "EventWaitCanceled"],
        "replacement first, then the cancel"
    );
    let (event_type, payload) = s.completion(&lint, thread).await;
    assert!(s.woken_by(&event_type, &payload).await.is_empty());

    s.registry.kill_for_thread(thread).await;
    s.teardown().await;
}

#[tokio::test]
async fn a_model_wait_watching_more_ends_whole_and_is_named() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let lint = s.task(thread, "make lint").await;
    let model = s
        .wait(
            thread,
            "toolu_model",
            vec![
                completed(&lint),
                EventSubscription {
                    event_type: "ChangeProposed".into(),
                    condition: None,
                },
            ],
        )
        .await;

    let outcome = stop_and_stand_down(&s.registry, &s.bus, &s.live, thread, &lint)
        .await
        .expect("the task was running");
    assert!(
        outcome.replacements.is_empty(),
        "no wait the model never armed"
    );
    assert_eq!(
        outcome.stopped.ended_with_others,
        vec![model.reason.clone()]
    );
    assert!(s.live.for_thread(thread).await.is_empty());

    s.teardown().await;
}

#[tokio::test]
async fn stopping_all_of_a_threads_tasks_leaves_everything_else_alone() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let neighbour = s.thread().await;
    let lint = s.task(thread, "make lint").await;
    let tests = s.task(thread, "the engine tests").await;
    s.wait(thread, &engine_tool_use_id(), vec![completed(&lint)])
        .await;
    s.wait(thread, &engine_tool_use_id(), vec![completed(&tests)])
        .await;
    let child = s
        .wait(
            thread,
            "toolu_child",
            vec![EventSubscription {
                event_type: "ChildThreadCompleted".into(),
                condition: None,
            }],
        )
        .await;
    let theirs = s.task(neighbour, "their build").await;
    let their_wait = s
        .wait(neighbour, &engine_tool_use_id(), vec![completed(&theirs)])
        .await;

    let outcomes = stop_all_and_stand_down(&s.registry, &s.bus, &s.live, thread).await;
    let mut stopped: Vec<String> = outcomes.iter().map(|o| o.stopped.label.clone()).collect();
    stopped.sort();
    assert_eq!(stopped, ["make lint", "the engine tests"]);

    let left: Vec<Uuid> = s
        .live
        .for_thread(thread)
        .await
        .iter()
        .map(|w| w.wait_id)
        .collect();
    assert_eq!(
        left,
        vec![child.wait_id],
        "only the unrelated wait survives"
    );
    assert!(
        s.registry.is_running(&theirs).await,
        "another thread's task is untouched"
    );
    let neighbour_waits: Vec<Uuid> = s
        .live
        .for_thread(neighbour)
        .await
        .iter()
        .map(|w| w.wait_id)
        .collect();
    assert_eq!(neighbour_waits, vec![their_wait.wait_id]);
    for task_id in [&lint, &tests] {
        let (event_type, payload) = s.completion(task_id, thread).await;
        assert!(s.woken_by(&event_type, &payload).await.is_empty());
    }

    s.registry.kill_for_thread(neighbour).await;
    s.teardown().await;
}

#[tokio::test]
async fn a_task_already_being_stopped_is_not_stopped_twice() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let lint = s.task(thread, "make lint").await;

    assert!(
        stop_and_stand_down(&s.registry, &s.bus, &s.live, thread, &lint)
            .await
            .is_some()
    );
    assert!(
        stop_and_stand_down(&s.registry, &s.bus, &s.live, thread, &lint)
            .await
            .is_none()
    );

    s.teardown().await;
}
