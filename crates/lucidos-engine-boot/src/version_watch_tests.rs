#[cfg(unix)]
use lucidos_engine::engine::engine_version::disk_build_id_at;
use lucidos_engine::engine::engine_version::{
    DeferredBuild, FrontendBuildWatchStopped, QueuedBuild,
};

#[test]
fn version_status_watch_is_dev_only() {
    assert!(!super::version_status_watch_enabled(true));
    assert!(super::version_status_watch_enabled(false));
}

#[test]
fn stopped_build_watch_status_is_omitted_until_reported() {
    let mut status = watched_status();
    let wire = serde_json::to_value(&status).unwrap();
    assert!(wire.get("frontend_build_watch_stopped").is_none());

    status.frontend_build_watch_stopped = Some(FrontendBuildWatchStopped {
        since_ms: 123_000,
        cause: Some("SIGTERM".to_owned()),
    });
    let wire = serde_json::to_value(&status).unwrap();
    assert_eq!(
        wire["frontend_build_watch_stopped"],
        serde_json::json!({"since_ms": 123_000, "cause": "SIGTERM"})
    );
}

fn watched_status() -> super::VersionStatus {
    super::VersionStatus {
        build_id: "running".into(),
        update_available: false,
        disk_build_id: Some("disk-a".into()),
        packaged: false,
        build_state: "idle",
        source_behind_head: false,
        shared_build_in_progress: false,
        head_commit: None,
        build_failure: None,
        build_elapsed_ms: None,
        build_queued: None,
        build_deferred: Some(DeferredBuild {
            coding_threads: 1,
            threads: vec![],
            deferred_ms: 0,
        }),
        pending_commits: None,
        frontend_refresh_elapsed_ms: None,
        frontend_build_watch_stopped: None,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn watcher_ticks_read_replaced_binary_and_emit_once() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("engine");
    let publish = |id: &str| {
        let staged = dir.path().join("staged");
        std::fs::write(&staged, format!("#!/bin/sh\nprintf '%s\\n' '{id}'\n")).unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(staged, &binary).unwrap();
    };
    publish("disk-a");
    let connections = super::SseConnectionCounter::new();
    let counter = connections.clone();
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let (samples, mut sampled) = tokio::sync::mpsc::unbounded_channel();
    let executable = binary.clone();
    let watcher = tokio::spawn(async move {
        super::run_version_status_watch(
            &counter,
            || async {
                let mut status = watched_status();
                status.disk_build_id = disk_build_id_at(&executable).await;
                samples.send(status.disk_build_id.clone()).unwrap();
                status
            },
            |event| {
                events.send(event).unwrap();
                std::future::ready(())
            },
        )
        .await;
    });
    // Dropping a JoinHandle detaches its task; abort even on assertion failure.
    struct AbortOnDrop(tokio::task::JoinHandle<()>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _watcher = AbortOnDrop(watcher);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        sampled.try_recv().is_err(),
        "idle tick must not read the executable"
    );
    let _page = connections.connect();
    for id in ["disk-a", "disk-b"] {
        if id == "disk-b" {
            publish(id);
        }
        let sample = tokio::time::timeout(Duration::from_secs(15), sampled.recv())
            .await
            .unwrap();
        assert_eq!(sample, Some(Some(id.into())));
        let event = tokio::time::timeout(Duration::from_secs(1), received.recv())
            .await
            .unwrap();
        assert!(matches!(
            event,
            Some(super::SystemEvent::EngineVersionStatusChanged { .. })
        ));
    }
    tokio::time::timeout(Duration::from_secs(15), sampled.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::task::yield_now().await;
    assert!(received.try_recv().is_err(), "unchanged tick must not emit");
}

#[tokio::test]
async fn watcher_notices_outside_binary_once() {
    let mut watch = super::VersionStatusWatch::default();
    assert!(watch.observe(true, async { watched_status() }).await);
    let changed = || {
        let mut s = watched_status();
        s.disk_build_id = Some("disk-b".into());
        s
    };
    assert!(watch.observe(true, async { changed() }).await);
    assert!(!watch.observe(true, async { changed() }).await);
}

#[tokio::test]
async fn watcher_ignores_elapsed_but_keeps_deferred_details() {
    let mut watch = super::VersionStatusWatch::default();
    assert!(watch.observe(true, async { watched_status() }).await);
    assert!(
        !watch
            .observe(true, async {
                let mut s = watched_status();
                s.build_elapsed_ms = Some(4000);
                s.frontend_refresh_elapsed_ms = Some(4000);
                s.build_deferred.as_mut().unwrap().deferred_ms = 4000;
                s
            })
            .await
    );
    assert!(
        watch
            .observe(true, async {
                let mut s = watched_status();
                s.build_deferred.as_mut().unwrap().coding_threads = 2;
                s
            })
            .await
    );
}

#[tokio::test]
async fn watcher_notices_each_outside_input_independently() {
    for field in ["head", "queued", "shared"] {
        let mut watch = super::VersionStatusWatch::default();
        assert!(watch.observe(true, async { watched_status() }).await);
        let changed = || {
            let mut s = watched_status();
            match field {
                "head" => s.head_commit = Some("next-head".into()),
                "queued" => {
                    s.build_queued = Some(QueuedBuild {
                        holders: vec!["another build".into()],
                    })
                }
                "shared" => s.shared_build_in_progress = true,
                _ => unreachable!(),
            }
            s
        };
        assert!(watch.observe(true, async { changed() }).await, "{field}");
        assert!(!watch.observe(true, async { changed() }).await, "{field}");
    }
}

#[tokio::test]
async fn watcher_repairs_first_connection_and_reconnection() {
    let mut watch = super::VersionStatusWatch::default();
    assert!(watch.observe(true, async { watched_status() }).await);
    assert!(!watch.observe(true, async { watched_status() }).await);
    assert!(
        !watch
            .observe(false, async { panic!("idle watcher read") })
            .await
    );
    assert!(watch.observe(true, async { watched_status() }).await);
    assert!(!watch.observe(true, async { watched_status() }).await);
}

#[tokio::test]
async fn watcher_does_not_read_without_a_page() {
    let mut watch = super::VersionStatusWatch::default();
    assert!(
        !watch
            .observe(false, async { panic!("idle watcher read status") })
            .await
    );
    assert!(watch.previous.is_none());
}
