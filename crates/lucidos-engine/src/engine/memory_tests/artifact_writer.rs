//! The thread an artifact extraction bills: the one that wrote the file at
//! that commit, while it still exists.

use super::artifact_writer_thread;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

async fn chat_thread(bus: &EventBus) -> Uuid {
    let thread_id = Uuid::new_v4();
    let event: ThreadEvent = serde_json::from_value(serde_json::json!({
        "type": "MessageReceived", "text": "write the notes", "mode": "human",
    }))
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("seed the writer thread");
    thread_id
}

async fn write(bus: &EventBus, commit: &str, writer: Option<Uuid>) {
    bus.emit(BusEvent::System(SystemEvent::ArtifactUpdated {
        artifact_path: "notes/plan.md".to_string(),
        commit: commit.to_string(),
        source: Some("run_python".to_string()),
        writer_thread_id: writer,
    }))
    .await
    .expect("seed the artifact write");
}

#[tokio::test]
async fn an_artifact_bills_the_surviving_thread_that_wrote_it() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let writer = chat_thread(&bus).await;
    write(&bus, "c1", Some(writer)).await;
    write(&bus, "c2", Some(Uuid::new_v4())).await;
    write(&bus, "c3", None).await;

    let at = |commit: &'static str| artifact_writer_thread(&pool, "notes/plan.md", commit);
    assert_eq!(at("c1").await.unwrap(), Some(writer));
    assert_eq!(at("c2").await.unwrap(), None, "a writer no longer here");
    assert_eq!(at("c3").await.unwrap(), None, "a write no thread made");
    assert_eq!(at("c4").await.unwrap(), None, "a commit never announced");

    teardown_test_db(&db).await;
}
