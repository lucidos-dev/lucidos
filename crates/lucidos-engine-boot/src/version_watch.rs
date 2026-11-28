use lucidos_engine::engine::change_ops::now_epoch_millis;
use lucidos_engine::engine::engine_version::VersionStatus;
use lucidos_engine::engine::event_bus::{BusEvent, SystemEvent};
use lucidos_engine::engine::sse_connections::SseConnectionCounter;
use lucidos_engine::{runtime, LucidosEngine};
use std::sync::Arc;
use std::time::Duration;

/// One observation per engine, independent of the number of connected clients.
const VERSION_STATUS_WATCH_INTERVAL: Duration = Duration::from_secs(4);

fn version_status_watch_enabled(packaged: bool) -> bool {
    !packaged
}

fn fingerprint(mut status: VersionStatus) -> serde_json::Value {
    status.build_elapsed_ms = None;
    status.frontend_refresh_elapsed_ms = None;
    status.frontend_build_watch_stopped = None;
    if let Some(deferred) = &mut status.build_deferred {
        deferred.deferred_ms = 0;
    }
    serde_json::to_value(status).expect("VersionStatus is JSON serializable")
}

#[derive(Default)]
struct VersionStatusWatch {
    previous: Option<serde_json::Value>,
}

impl VersionStatusWatch {
    async fn observe(
        &mut self,
        connected: bool,
        read: impl std::future::Future<Output = VersionStatus>,
    ) -> bool {
        if !connected {
            self.previous = None;
            return false;
        }
        let next = fingerprint(read.await);
        let changed = self.previous.as_ref() != Some(&next);
        self.previous = Some(next);
        changed
    }
}

/// Watch external build inputs only while a page can consume the result.
pub fn spawn_version_status_watch(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if !version_status_watch_enabled(runtime::is_packaged()) {
            return;
        }
        run_version_status_watch(
            &engine.sse_connections,
            || engine.version_status(),
            |event| async {
                engine
                    .event_bus
                    .emit_or_log(
                        BusEvent::System(event),
                        "[Version] EngineVersionStatusChanged",
                    )
                    .await;
            },
        )
        .await;
    })
}

async fn run_version_status_watch<R, F, E, G>(connections: &SseConnectionCounter, read: R, emit: E)
where
    R: Fn() -> F,
    F: std::future::Future<Output = VersionStatus>,
    E: Fn(SystemEvent) -> G,
    G: std::future::Future<Output = ()>,
{
    let mut watch = VersionStatusWatch::default();
    let mut ticker = tokio::time::interval(VERSION_STATUS_WATCH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if watch
            .observe(connections.count() > 0, async { read().await })
            .await
        {
            emit(SystemEvent::EngineVersionStatusChanged {
                sent_at_ms: now_epoch_millis(),
            })
            .await;
        }
    }
}

#[cfg(test)]
#[path = "version_watch_tests.rs"]
mod tests;
