use super::*;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) fn make_threads() -> Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>> {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

pub(crate) fn make_completions() -> Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>> {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

pub(crate) fn register(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    thread_id: Uuid,
) -> ThreadRegistration {
    try_register_thread(threads, &make_completions(), thread_id)
        .admitted()
        .expect("the thread must be free in this test")
}

pub(crate) async fn register_queued_with_timeout(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    completions: &Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
    thread_id: Uuid,
    timeout: std::time::Duration,
) -> ThreadRegistration {
    admit_with_stuck_turn_eviction(threads, completions, thread_id, timeout, || async {}).await
}

pub(crate) fn test_prompt(text: &str) -> InjectedPrompt {
    InjectedPrompt {
        text: text.into(),
        event_id: None,
        mode: thread_events::ActorMode::Human,
        spawning_event_id: None,
        images: None,
        origin: None,
        kind: InjectedPromptKind::UserText,
    }
}

pub(crate) async fn register_queued(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    completions: &Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
    thread_id: Uuid,
) -> ThreadRegistration {
    register_queued_with_timeout(
        threads,
        completions,
        thread_id,
        std::time::Duration::from_secs(60),
    )
    .await
}
