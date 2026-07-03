use super::*;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Create a standalone active_threads map for testing thread registration.
pub(crate) fn make_threads() -> Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>> {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

/// Create a standalone completion notifiers map for testing.
pub(crate) fn make_completions() -> Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>> {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

/// Replicate register_thread logic for standalone testing (sync, no queuing).
pub(crate) fn register(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    thread_id: Uuid,
) -> (
    CancellationToken,
    mpsc::UnboundedReceiver<InjectedPrompt>,
    ThreadGuard,
) {
    let token = CancellationToken::new();
    let (injection_tx, injection_rx) = mpsc::unbounded_channel();
    let gen = THREAD_GENERATION.fetch_add(1, Ordering::Relaxed);
    let mut guard_map = threads.lock().unwrap();
    guard_map.insert(
        thread_id,
        ThreadHandle {
            token: token.clone(),
            injection_tx,
            generation: gen,
            cancel_actor: Arc::new(std::sync::Mutex::new(None)),
        },
    );
    let guard = ThreadGuard {
        active_threads: threads.clone(),
        thread_id,
        completion_notify: Arc::new(std::sync::Mutex::new(HashMap::new())),
        generation: gen,
    };
    (token, injection_rx, guard)
}

/// Replicate register_thread_queued logic: waits for existing thread to
/// finish before registering. Force-evicts after `timeout`.
pub(crate) async fn register_queued_with_timeout(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    completions: &Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
    thread_id: Uuid,
    timeout: std::time::Duration,
) -> (
    CancellationToken,
    mpsc::UnboundedReceiver<InjectedPrompt>,
    ThreadGuard,
) {
    let wait_result = tokio::time::timeout(timeout, async {
        loop {
            let n = {
                let guard_map = threads.lock().unwrap();
                if guard_map.contains_key(&thread_id) {
                    let mut comps = completions.lock().unwrap();
                    comps
                        .entry(thread_id)
                        .or_insert_with(|| Arc::new(tokio::sync::Notify::new()))
                        .clone()
                } else {
                    return;
                }
            };
            tokio::select! {
                _ = n.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            }
        }
    })
    .await;

    if wait_result.is_err() {
        if let Some(handle) = threads.lock().unwrap().remove(&thread_id) {
            handle.token.cancel();
        }
        if let Some(n) = completions.lock().unwrap().remove(&thread_id) {
            n.notify_waiters();
        }
    }

    let token = CancellationToken::new();
    let (injection_tx, injection_rx) = mpsc::unbounded_channel();
    let gen = THREAD_GENERATION.fetch_add(1, Ordering::Relaxed);
    let mut guard_map = threads.lock().unwrap();
    guard_map.insert(
        thread_id,
        ThreadHandle {
            token: token.clone(),
            injection_tx,
            generation: gen,
            cancel_actor: Arc::new(std::sync::Mutex::new(None)),
        },
    );
    let guard = ThreadGuard {
        active_threads: threads.clone(),
        thread_id,
        completion_notify: completions.clone(),
        generation: gen,
    };
    (token, injection_rx, guard)
}

/// Convenience wrapper with the default 60s timeout.
pub(crate) async fn register_queued(
    threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    completions: &Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
    thread_id: Uuid,
) -> (
    CancellationToken,
    mpsc::UnboundedReceiver<InjectedPrompt>,
    ThreadGuard,
) {
    register_queued_with_timeout(
        threads,
        completions,
        thread_id,
        std::time::Duration::from_secs(60),
    )
    .await
}

