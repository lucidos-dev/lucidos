use lucidos_engine::engine::{frontend_preview::reap_orphaned_preview, BuildWatchTracker};
use lucidos_engine::{runtime, LucidosEngine};
use std::sync::Arc;
use std::time::Duration;

const SERVED_FRONTEND_SYNC_INTERVAL: Duration = Duration::from_secs(10);

const LIVENESS_INTERVAL: Duration = Duration::from_secs(60);

pub fn spawn_served_frontend_sync(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if runtime::is_packaged() {
            return;
        }
        let mut build_watch = BuildWatchTracker::default();
        let mut ticker = tokio::time::interval(SERVED_FRONTEND_SYNC_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            engine.track_frontend_build_watch(&mut build_watch).await;
            engine.sync_served_frontend_if_safe().await;
            engine.start_deferred_build_if_admitted().await;
            engine.self_heal_engine_version_if_needed().await;
        }
    })
}

pub fn init_frontend_preview(engine: Arc<LucidosEngine>) {
    if runtime::is_packaged() {
        return;
    }
    reap_orphaned_preview(engine.workspace_path());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(LIVENESS_INTERVAL).await;
            engine.reconcile_frontend_preview().await;
        }
    });
}

#[cfg(test)]
mod guard_tests {
    #[test]
    fn frontend_tick_syncs_before_admitting_deferred_builds_and_self_healing() {
        let source = include_str!("frontend_tasks.rs");
        let call = |method| source.find(&format!("engine.{method}().await")).unwrap();
        let sync = call("sync_served_frontend_if_safe");
        let admit = call("start_deferred_build_if_admitted");
        let heal = call("self_heal_engine_version_if_needed");
        assert!(sync < admit && admit < heal);
    }
}
