//! Affected triggers through the real update and uninstall paths: the staged
//! preview, the event, the confirm's outcome and its one notification. The
//! matching rules themselves are tested in `triggers::affected`.

use super::helpers::*;
use super::*;

use crate::engine::event_bus::MockEventBus;
use crate::scheduler::notifications::{NavigateTarget, Tap};
use crate::triggers::affected::{AffectedPath, AffectedReason};

const MANIFEST_V1: &str = r#"
id = "token-cost"
version = "0.4.0"
name = "Token Cost"
description = "test"
source = "https://github.com/example-org/example-repo"
"#;

const MANIFEST_V2: &str = r#"
id = "token-cost"
version = "0.6.0"
name = "Token Cost"
description = "test"
source = "https://github.com/example-org/example-repo"
"#;

const INDEX: &str = "apps/token-cost/index.html";
const DAILY: &str = "apps/token-cost/knowhow/daily-price-check.md";

/// A trigger an agent created with `create_trigger`: it belongs to the app,
/// names no path, and has no plugin provenance.
fn agent_trigger(id: &str, name: &str) -> TriggerConfig {
    TriggerConfig::from_created_payload(&serde_json::json!({
        "trigger_id": id,
        "name": name,
        "schedule": ["0 0 8 * * *"],
        "run": {"type": "intent", "intent": "Check the official model prices against my price table."},
        "app_id": "token-cost",
    }))
    .unwrap()
}

/// A trigger the acting plugin ships, which its own re-sync handles.
fn own_trigger() -> TriggerConfig {
    let mut t = agent_trigger("t-own", "Token Cost refresh");
    t.plugin_id = Some("token-cost".to_string());
    t
}

fn stage_version(ws: &Path, tag: &str, manifest: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = ws.join("staging").join(tag);
    std::fs::create_dir_all(&dir).unwrap();
    let archive = build_archive(&dir, "v.lucidos-plugin", manifest, files);
    extract_to(&dir, &archive)
}

/// A workspace with Token Cost v0.4.0 installed, the knowhow file included.
async fn installed_v1(bus: &MockEventBus) -> (tempfile::TempDir, PathBuf) {
    let (tmp, ws) = fresh_workspace();
    let root = stage_version(
        &ws,
        "v1",
        MANIFEST_V1,
        &[(INDEX, b"<h1>v1</h1>\n"), (DAILY, b"# Daily price check\n")],
    );
    install_from_unpacked_with_bus(
        &ws,
        bus,
        &root,
        InstallContext::plain(SourceType::Archive, false),
    )
    .await
    .expect("first install");
    (tmp, ws)
}

/// Confirm the update to v0.6.0, which drops the knowhow file.
async fn update_to_v2(
    ws: &Path,
    bus: &MockEventBus,
    removals: Vec<String>,
    triggers: Vec<TriggerConfig>,
) -> InstallWrite {
    let root = stage_version(ws, "v2", MANIFEST_V2, &[(INDEX, b"<h1>v2</h1>\n")]);
    install_from_unpacked_with_bus(
        ws,
        bus,
        &root,
        InstallContext {
            removals,
            triggers,
            ..InstallContext::plain(SourceType::Archive, true)
        },
    )
    .await
    .expect("update")
}

/// Stage the update to v0.6.0 the way `stage_install_request` does, matching
/// over `triggers`, and return the preview.
fn staged_v2_preview(ws: &Path, triggers: &[TriggerConfig]) -> serde_json::Value {
    let pending = std::sync::Arc::new(PendingInstallsMap::default());
    let archive_dir = ws.join("archive-v2");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let archive = build_archive(
        &archive_dir,
        "v2.lucidos-plugin",
        MANIFEST_V2,
        &[(INDEX, b"<h1>v2</h1>\n")],
    );
    let installed = InstalledPlugins {
        files: [(
            "token-cost".to_string(),
            vec![INDEX.to_string(), DAILY.to_string()],
        )]
        .into(),
        ..Default::default()
    };
    let staged = affected_triggers::annotate_staged(
        prepare_install_request(
            ws,
            &pending,
            archive.to_str().unwrap(),
            &Ok(installed),
            &GitCredentials::none(),
            &running(),
        ),
        triggers,
    );
    serde_json::from_str(staged.strip_prefix(PLUGIN_INSTALL_REQUEST_PREFIX).unwrap()).unwrap()
}

fn notifications(bus: &MockEventBus) -> Vec<(String, String, Tap)> {
    bus.emitted_events()
        .into_iter()
        .filter_map(|e| match e {
            BusEvent::System(SystemEvent::NotificationCreated {
                title,
                message,
                tap,
                ..
            }) => Some((title, message, tap)),
            _ => None,
        })
        .collect()
}

fn tap_target(tap: &Tap) -> (NavigateTarget, Option<String>) {
    match tap {
        Tap::Navigate { to } => (to.target, to.id.clone()),
        Tap::Modal => panic!("the notification must open a trigger surface"),
    }
}

fn no_trigger_writes(bus: &MockEventBus) -> bool {
    bus.emitted_events().iter().all(|e| {
        !matches!(
            e,
            BusEvent::System(
                SystemEvent::TriggerCreated { .. }
                    | SystemEvent::TriggerUpdated { .. }
                    | SystemEvent::TriggerDeleted { .. }
            )
        )
    })
}

fn daily_owned_by_app(trigger_id: &str, name: &str) -> AffectedTrigger {
    AffectedTrigger {
        trigger_id: trigger_id.to_string(),
        trigger_name: name.to_string(),
        paths: vec![AffectedPath {
            path: DAILY.to_string(),
            reason: AffectedReason::OwningApp,
        }],
    }
}

#[tokio::test]
async fn an_update_reports_the_trigger_left_without_its_knowhow() {
    let bus = MockEventBus::new();
    let (_tmp, ws) = installed_v1(&bus).await;
    let triggers = vec![
        agent_trigger("t-daily", "Token Cost daily price check"),
        own_trigger(),
    ];

    let write = update_to_v2(&ws, &bus, vec![DAILY.to_string()], triggers).await;

    let expected = vec![daily_owned_by_app(
        "t-daily",
        "Token Cost daily price check",
    )];
    assert_eq!(
        write.affected_triggers, expected,
        "the acting plugin's own trigger is skipped"
    );
    let recorded = bus.emitted_events().iter().rev().find_map(|e| match e {
        BusEvent::System(SystemEvent::PluginInstalled {
            affected_triggers, ..
        }) => Some(affected_triggers.clone()),
        _ => None,
    });
    assert_eq!(recorded, Some(expected), "the event records it");

    let notes = notifications(&bus);
    assert_eq!(notes.len(), 1, "exactly one notification: {notes:?}");
    let (title, message, tap) = &notes[0];
    assert_eq!(title, "Token Cost update removed know-how a trigger uses");
    assert!(
        message.contains(DAILY) && message.contains("v0.6.0"),
        "{message}"
    );
    assert_eq!(
        tap_target(tap),
        (NavigateTarget::Trigger, Some("t-daily".to_string()))
    );

    assert!(no_trigger_writes(&bus), "the update never writes a trigger");
}

#[tokio::test]
async fn an_affected_trigger_never_changes_what_the_confirm_does() {
    let with_bus = MockEventBus::new();
    let (_with_tmp, with_ws) = installed_v1(&with_bus).await;
    let with = update_to_v2(
        &with_ws,
        &with_bus,
        vec![DAILY.to_string()],
        vec![agent_trigger("t-daily", "Daily")],
    )
    .await;

    let without_bus = MockEventBus::new();
    let (_without_tmp, without_ws) = installed_v1(&without_bus).await;
    let without = update_to_v2(&without_ws, &without_bus, vec![DAILY.to_string()], vec![]).await;

    assert_eq!(with.removed_files, vec![DAILY.to_string()]);
    assert_eq!(with.removed_files, without.removed_files);
    assert_eq!(with.installed_files, without.installed_files);
    assert!(!with_ws.join("data").join(DAILY).exists());
    assert!(notifications(&without_bus).is_empty());
}

#[tokio::test]
async fn a_clean_update_raises_no_notification() {
    let bus = MockEventBus::new();
    let (_tmp, ws) = installed_v1(&bus).await;

    let write = update_to_v2(&ws, &bus, vec![], vec![agent_trigger("t-daily", "Daily")]).await;

    assert!(write.affected_triggers.is_empty());
    assert!(notifications(&bus).is_empty());
}

/// The confirm matches over its own trigger snapshot, so a trigger created
/// after the panel was staged still lands in the event.
#[tokio::test]
async fn the_confirm_sees_a_trigger_created_after_staging() {
    let bus = MockEventBus::new();
    let (_tmp, ws) = installed_v1(&bus).await;

    let preview = staged_v2_preview(&ws, &[]);
    assert_eq!(preview["removes"], serde_json::json!([DAILY]));
    assert_eq!(preview["affected_triggers"], serde_json::json!([]));

    let write = update_to_v2(
        &ws,
        &bus,
        vec![DAILY.to_string()],
        vec![agent_trigger("t-late", "Created later")],
    )
    .await;
    assert_eq!(
        write.affected_triggers,
        vec![daily_owned_by_app("t-late", "Created later")]
    );
}

#[tokio::test]
async fn the_staged_update_preview_lists_affected_triggers() {
    let bus = MockEventBus::new();
    let (_tmp, ws) = installed_v1(&bus).await;
    let preview = staged_v2_preview(&ws, &[agent_trigger("t-daily", "Daily"), own_trigger()]);

    assert_eq!(
        preview["affected_triggers"],
        serde_json::json!([{
            "trigger_id": "t-daily",
            "trigger_name": "Daily",
            "paths": [{"path": DAILY, "reason": "owning-app"}],
        }])
    );
}

#[tokio::test]
async fn an_uninstall_reports_previews_and_notifies_once() {
    let bus = MockEventBus::new();
    let (_tmp, ws) = installed_v1(&bus).await;
    let pending = std::sync::Arc::new(PendingUninstallsMap::default());
    let triggers = vec![agent_trigger("t-daily", "Daily"), own_trigger()];

    let staged = affected_triggers::annotate_staged(
        prepare_uninstall_request(
            &ws,
            &pending,
            "token-cost",
            "0.4.0",
            "Token Cost",
            vec![INDEX.to_string(), DAILY.to_string()],
        ),
        &triggers,
    );
    let preview: serde_json::Value = serde_json::from_str(
        staged
            .strip_prefix(PLUGIN_UNINSTALL_REQUEST_PREFIX)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        preview["affected_triggers"],
        serde_json::json!([{
            "trigger_id": "t-daily",
            "trigger_name": "Daily",
            "paths": [{"path": DAILY, "reason": "owning-app"}],
        }])
    );

    let entry = PendingUninstall {
        plugin_id: "token-cost".to_string(),
        plugin_version: "0.4.0".to_string(),
        plugin_name: "Token Cost".to_string(),
        files_present: vec![INDEX.to_string(), DAILY.to_string()],
        files_missing: vec![],
        created_at: chrono::Utc::now(),
    };
    let outcome = uninstall_with_bus(&ws, &bus, &entry, &triggers, None)
        .await
        .expect("uninstall");

    let expected = vec![daily_owned_by_app("t-daily", "Daily")];
    assert_eq!(outcome.affected_triggers, expected);
    assert_eq!(
        outcome.files_deleted.len(),
        2,
        "the confirm deletes as before"
    );
    let recorded = bus.emitted_events().iter().rev().find_map(|e| match e {
        BusEvent::System(SystemEvent::PluginUninstalled {
            affected_triggers, ..
        }) => Some(affected_triggers.clone()),
        _ => None,
    });
    assert_eq!(recorded, Some(expected));

    let notes = notifications(&bus);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(
        notes[0].0,
        "Removing Token Cost removed know-how a trigger uses"
    );
    assert_eq!(
        tap_target(&notes[0].2),
        (NavigateTarget::Trigger, Some("t-daily".to_string()))
    );
    assert!(
        no_trigger_writes(&bus),
        "uninstall_with_bus never writes a trigger"
    );
}

#[tokio::test]
async fn several_affected_triggers_open_the_trigger_list() {
    let bus = MockEventBus::new();
    let (_tmp, ws) = installed_v1(&bus).await;

    update_to_v2(
        &ws,
        &bus,
        vec![DAILY.to_string()],
        vec![agent_trigger("t-a", "A"), agent_trigger("t-b", "B")],
    )
    .await;

    let notes = notifications(&bus);
    assert_eq!(notes.len(), 1, "one notification per confirm: {notes:?}");
    assert_eq!(
        notes[0].0,
        "Token Cost update removed know-how 2 triggers use"
    );
    assert_eq!(tap_target(&notes[0].2), (NavigateTarget::Triggers, None));
}
