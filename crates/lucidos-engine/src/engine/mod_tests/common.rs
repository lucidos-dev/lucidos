use super::*;
use std::sync::Arc;
use uuid::Uuid;

/// Create a standalone active_threads map for testing thread registration.
pub(crate) fn make_threads() -> Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>> {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

/// Create a standalone completion notifiers map for testing.
pub(crate) fn make_completions() -> Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>> {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

/// Admit a run, panicking if the thread is already owned.
///
/// The real [`try_register_thread`], not a copy of it. A hand-copied helper is
/// how the check-then-insert race stayed untested through the incident that
/// produced it: the copy was correct about a map, and said nothing about the
/// production path.
pub(crate) fn register(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    thread_id: Uuid,
) -> ThreadRegistration {
    try_register_thread(threads, &make_completions(), thread_id)
        .admitted()
        .expect("the thread must be free in this test")
}

/// The real `register_thread_queued` body, with a caller-chosen timeout so a
/// test does not sit for 60 s. Only the engine's `ResponseAborted` pre-emit is
/// dropped, since it needs a bus and a pool.
pub(crate) async fn register_queued_with_timeout(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    completions: &Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
    thread_id: Uuid,
    timeout: std::time::Duration,
) -> ThreadRegistration {
    admit_with_stuck_turn_eviction(threads, completions, thread_id, timeout, || async {}).await
}

/// A plain user follow-up, for the tests that only care that a prompt reaches
/// the live turn's channel.
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

/// Convenience wrapper with the default 60s timeout.
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
