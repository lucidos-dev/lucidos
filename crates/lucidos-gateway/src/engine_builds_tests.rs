use super::*;
use crate::registry::Workspace;
use std::os::unix::fs::PermissionsExt;

fn workspace(id: &str, dir: &Path) -> Workspace {
    Workspace {
        id: id.into(),
        name: id.to_uppercase(),
        dir: dir.join(id).to_string_lossy().into_owned(),
        port: 5199,
        database_url: None,
        cleanup: Default::default(),
        autostart: true,
    }
}

/// Publish a "new build": the engine binary now answers `--build-id` with `id`.
fn publish_build(engine_bin: &Path, id: &str) {
    std::fs::write(engine_bin, format!("#!/bin/sh\necho {id}\n")).unwrap();
    std::fs::set_permissions(engine_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A gateway with a stopped `eval` and a running `dev` whose engine does not
/// answer.
async fn gateway_with_stopped_and_running() -> (GatewayState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
    std::fs::create_dir_all(state.engine_bin().parent().unwrap()).unwrap();
    state.register_for_test(workspace("eval", dir.path()));
    state
        .install_notification_stack_for_test(&dir.path().join("dev"))
        .await;
    (state, dir)
}

#[tokio::test]
async fn a_new_build_changes_no_stopped_workspace() {
    let (state, _dir) = gateway_with_stopped_and_running().await;
    publish_build(state.engine_bin(), "src-old");
    list(&state).await;
    publish_build(state.engine_bin(), "src-new");

    let builds = list(&state).await;

    assert_eq!(builds.newest.as_deref(), Some("src-new"));
    let eval = builds.workspaces.iter().find(|w| w.id == "eval").unwrap();
    assert!(!eval.running && !eval.behind, "{eval:?}");
    assert!(
        !state.running_engine_addresses().await.contains_key("eval"),
        "listing a new build started a stopped workspace"
    );
}

#[tokio::test]
async fn switch_all_running_targets_running_workspaces_only() {
    let running = HashMap::from([("dev".to_string(), RunningBuild::Unreported)]);
    let registered = ["dev", "eval"]
        .map(|id| (id.to_string(), id.to_string()))
        .to_vec();
    let builds = assemble(
        Path::new("/nonexistent"),
        registered,
        &running,
        Some("src-new".into()),
    )
    .await;
    assert_eq!(builds.switch_targets(), vec!["dev".to_string()]);

    let outcome = tally(
        builds
            .switch_targets()
            .into_iter()
            .map(|id| (id, Ok(true)))
            .collect(),
    );
    assert_eq!(outcome.switched, vec!["dev".to_string()]);
}

#[tokio::test]
async fn an_engine_that_does_not_answer_is_not_switched() {
    let (state, _dir) = gateway_with_stopped_and_running().await;
    publish_build(state.engine_bin(), "src-new");

    let builds = list(&state).await;

    let dev = builds.workspaces.iter().find(|w| w.id == "dev").unwrap();
    assert!(dev.running && dev.unanswered && !dev.behind, "{dev:?}");
    assert!(builds.switch_targets().is_empty());
    assert!(switch_all_running(&state, Some("device-1"))
        .await
        .switched
        .is_empty());
}

#[tokio::test]
async fn the_switch_restart_leaves_a_stopped_workspace_stopped() {
    let (state, _dir) = gateway_with_stopped_and_running().await;

    let restarted = state
        .restart_running(&["eval"], Some("device-1"), |_| std::future::ready(true))
        .await;

    assert!(matches!(restarted.as_slice(), [(id, Ok(false))] if id == "eval"));
    assert!(!state.running_engine_addresses().await.contains_key("eval"));
}

#[tokio::test]
async fn a_running_workspace_no_longer_behind_is_not_restarted() {
    let (state, _dir) = gateway_with_stopped_and_running().await;
    let asked = std::sync::Mutex::new(Vec::new());

    let restarted = state
        .restart_running(&["dev"], Some("device-1"), |id| {
            asked.lock().unwrap().push(id);
            std::future::ready(false)
        })
        .await;

    assert!(matches!(restarted.as_slice(), [(id, Ok(false))] if id == "dev"));
    assert_eq!(*asked.lock().unwrap(), vec!["dev".to_string()]);
    assert!(state.running_engine_addresses().await.contains_key("dev"));
}

#[test]
fn a_workspace_stopped_mid_switch_counts_as_skipped() {
    let outcome = tally(vec![("eval".into(), Ok(false))]);

    assert_eq!(outcome.skipped, vec!["eval".to_string()]);
    assert!(outcome.switched.is_empty());
}

#[tokio::test]
async fn a_running_engine_on_the_newest_build_is_not_behind() {
    let bin = Path::new("/nonexistent/lucidos-engine");
    let running = HashMap::from([
        ("dev".to_string(), RunningBuild::Reported("src-new".into())),
        ("demo".to_string(), RunningBuild::Reported("src-old".into())),
    ]);
    let registered = ["dev", "demo", "eval"]
        .map(|id| (id.to_string(), id.to_string()))
        .to_vec();

    let builds = assemble(bin, registered, &running, Some("src-new".into())).await;

    assert_eq!(builds.switch_targets(), vec!["demo".to_string()]);
}

#[tokio::test]
async fn an_unreadable_newest_build_switches_nothing() {
    let running = HashMap::from([("dev".to_string(), RunningBuild::Unreported)]);
    let registered = vec![("dev".to_string(), "Dev".to_string())];

    let builds = assemble(Path::new("/nonexistent"), registered, &running, None).await;

    assert!(builds.switch_targets().is_empty());
}
