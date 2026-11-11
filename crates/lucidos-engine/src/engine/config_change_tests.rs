use std::collections::HashMap;
use std::sync::RwLock;

use super::*;
use crate::core::McpServerStore;
use crate::engine::event_bus::EventBus;
use crate::engine::trigger_writes::TriggerRegistryWriter;
use crate::gateway_operators::Operator;
use crate::test_support::{setup_test_db, teardown_test_db};
use crate::triggers::TriggerConfig;

fn device(id: &str) -> MessageOrigin {
    MessageOrigin::Device {
        device_id: id.into(),
    }
}

fn person(id: &str, devices: &[&str]) -> Operator {
    Operator {
        id: id.into(),
        name: id.to_uppercase(),
        emails: vec![],
        device_ids: devices.iter().map(|d| d.to_string()).collect(),
    }
}

/// Sam holds two devices; Kim holds one.
fn team() -> OperatorSnapshot {
    OperatorSnapshot {
        operators: vec![
            person("sam", &["sam-phone", "sam-laptop"]),
            person("kim", &["kim-phone"]),
        ],
        relayed: vec![],
    }
}

/// Everything an undo writes through, over a fresh database.
struct Bench {
    pool: sqlx::PgPool,
    db: String,
    bus: EventBus,
    configs: RwLock<HashMap<String, TriggerConfig>>,
    write_lock: tokio::sync::Mutex<()>,
    workspace: tempfile::TempDir,
    mcp: crate::mcp::McpManager,
}

impl Bench {
    async fn new() -> Self {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let workspace = tempfile::tempdir().unwrap();
        let mcp = crate::mcp::McpManager::new(pool.clone(), bus.clone(), workspace.path().into());
        Self {
            pool,
            db,
            bus,
            configs: RwLock::new(HashMap::new()),
            write_lock: tokio::sync::Mutex::new(()),
            workspace,
            mcp,
        }
    }

    fn targets(&self) -> UndoTargets<'_> {
        UndoTargets {
            pool: &self.pool,
            bus: &self.bus,
            triggers: TriggerRegistryWriter {
                event_bus: &self.bus,
                trigger_configs: &self.configs,
                workspace_path: self.workspace.path(),
                write_lock: &self.write_lock,
            },
            mcp: &self.mcp,
        }
    }

    /// The newest stored event of a type, read back as a config change.
    async fn last_change(&self, event_type: &str) -> (Uuid, ConfigChange) {
        let id: Uuid = sqlx::query_scalar(
            "SELECT id FROM events WHERE event_type = $1 ORDER BY sequence DESC LIMIT 1",
        )
        .bind(event_type)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        let change = stored_change(&self.pool, id)
            .await
            .unwrap()
            .expect("a config change");
        (id, change)
    }

    /// Check the newest `event_type` names its maker and offers Undo, then
    /// undo it as Kim. A second undo finds the config moved on.
    async fn undo_last(&self, event_type: &str) -> ConfigChange {
        let (id, change) = self.last_change(event_type).await;
        let notice = notice(&change, id, &team()).expect("a team notice");
        assert!(notice.message.starts_with("SAM "), "{}", notice.message);
        assert_eq!(notice.tap, Tap::UndoConfigChange { event_id: id });
        assert_eq!(notice.audience, Some(vec!["kim-phone".to_string()]));
        let undo = change.undo.clone().expect("undoable");
        assert!(
            apply_undo(&self.targets(), id, undo.clone(), device("kim-phone"))
                .await
                .unwrap()
        );
        assert!(
            !apply_undo(&self.targets(), id, undo, device("kim-phone"))
                .await
                .unwrap(),
            "a second undo finds the config moved on"
        );
        change
    }

    fn trigger_name(&self, id: &str) -> Option<String> {
        self.configs.read().unwrap().get(id).map(|c| c.name.clone())
    }

    async fn done(self) {
        teardown_test_db(&self.db).await;
    }
}

#[test]
fn a_change_notifies_everyone_but_its_maker_and_only_on_a_team() {
    let change = ConfigChange {
        title: "Repository added",
        what: "added the repository \"x\"".into(),
        actor: Some(device("sam-laptop")),
        undo: None,
    };
    let id = Uuid::new_v4();
    let notice = notice(&change, id, &team()).unwrap();
    assert_eq!(notice.message, "SAM added the repository \"x\".");
    assert_eq!(notice.tap, Tap::Modal, "no undo, no Undo tap");
    assert_eq!(notice.audience, Some(vec!["kim-phone".to_string()]));

    let agent = ConfigChange {
        actor: Some(crate::engine::tools::agent_tool_actor(Uuid::new_v4())),
        ..change.clone()
    };
    let notice = super::notice(&agent, id, &team()).unwrap();
    assert_eq!(notice.message, "An agent added the repository \"x\".");
    assert_eq!(notice.audience, None, "nobody made it, so the team hears");

    let solo = OperatorSnapshot {
        operators: vec![person("sam", &["sam-laptop"])],
        relayed: vec![],
    };
    assert_eq!(super::notice(&change, id, &solo), None);
    let nobody = ConfigChange {
        actor: None,
        ..change
    };
    assert_eq!(super::notice(&nobody, id, &team()), None);
}

#[test]
fn a_tool_change_inverts_only_when_one_tool_moved() {
    let a = || "a".to_string();
    let b = || "b".to_string();
    assert_eq!(one_toggled(&[], &[a()]), Some((a(), true)));
    assert_eq!(one_toggled(&[a(), b()], &[b()]), Some((a(), false)));
    assert_eq!(one_toggled(&[a()], &[b()]), None);
    assert_eq!(one_toggled(&[a()], &[a()]), None);
}

/// Undoing a revoke, or restoring a deleted granted trigger, grants coding
/// agents again. The undo route holds that to the grant's own rule.
#[test]
fn an_undo_that_restores_a_coding_agent_grant_is_named() {
    let config = |granted: bool| {
        TriggerConfig::from_created_payload(&serde_json::json!({
            "trigger_id": "t1", "name": "T", "schedule": ["0 0 8 * * *"],
            "run": { "type": "intent", "intent": "x" },
            "coding_agent_grant": granted,
        }))
        .unwrap()
    };
    let undo = |write, granted: bool| ConfigUndo::WriteTrigger {
        write,
        trigger_id: "t1".to_string(),
        payload: config(granted).to_created_payload(),
    };
    let revoked = config(false);
    let granted = config(true);

    assert!(undo(TriggerWrite::Updated, true).needs_the_owner(Some(&revoked)));
    assert!(undo(TriggerWrite::Created, true).needs_the_owner(None));
    assert!(!undo(TriggerWrite::Updated, true).needs_the_owner(Some(&granted)));
    assert!(!undo(TriggerWrite::Updated, false).needs_the_owner(Some(&granted)));
    assert!(!undo(TriggerWrite::Deleted, true).needs_the_owner(Some(&revoked)));
    // Rewriting a granted trigger's work back to an older version.
    let mut rewritten = granted.clone();
    rewritten.apply_update(&serde_json::json!({ "mcp_servers": ["slack"] }));
    assert!(undo(TriggerWrite::Updated, true).needs_the_owner(Some(&rewritten)));
    assert!(!ConfigUndo::RemoveMcpServer {
        server_id: "s".to_string()
    }
    .needs_the_owner(None));
}

#[tokio::test]
async fn a_repository_add_and_remove_each_undo() {
    let bench = Bench::new().await;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_string_lossy().to_string();
    let repo = RepositoryStore::register(
        &bench.pool,
        &bench.bus,
        "Example",
        &path,
        Some("notes"),
        None,
        Some(device("sam-phone")),
    )
    .await
    .unwrap();

    bench.undo_last("RepositoryAdded").await;
    assert!(RepositoryStore::get(&bench.pool, repo.id)
        .await
        .unwrap()
        .is_none());

    RepositoryStore::register(
        &bench.pool,
        &bench.bus,
        "Example",
        &path,
        Some("notes"),
        None,
        None,
    )
    .await
    .unwrap();
    RepositoryStore::unregister(&bench.pool, &bench.bus, repo.id, Some(device("sam-phone")))
        .await
        .unwrap();
    bench.undo_last("RepositoryRemoved").await;
    let back = RepositoryStore::get(&bench.pool, repo.id)
        .await
        .unwrap()
        .expect("restored");
    assert_eq!(
        (back.name.as_str(), back.description.as_deref()),
        ("Example", Some("notes"))
    );
    bench.done().await;
}

#[tokio::test]
async fn a_trigger_create_update_delete_and_pause_each_undo() {
    let bench = Bench::new().await;
    let writer = bench.targets().triggers;
    let sam = || Some(device("sam-phone"));
    writer
        .write(
            TriggerWrite::Created,
            "t-1",
            serde_json::json!({
                "trigger_id": "t-1", "name": "Inbox", "slug": "inbox", "schedule": [],
                "timezone": "UTC", "run": { "type": "intent", "intent": "triage" },
                "on": [{ "event_type": "MailReceived" }],
            }),
            sam(),
        )
        .await
        .unwrap();

    writer
        .write(
            TriggerWrite::Updated,
            "t-1",
            serde_json::json!({ "trigger_id": "t-1", "name": "Renamed" }),
            sam(),
        )
        .await
        .unwrap();
    bench.undo_last("TriggerUpdated").await;
    assert_eq!(bench.trigger_name("t-1").as_deref(), Some("Inbox"));

    writer
        .write(
            TriggerWrite::Disabled,
            "t-1",
            serde_json::json!({ "trigger_id": "t-1", "paused": true }),
            sam(),
        )
        .await
        .unwrap();
    bench.undo_last("TriggerDisabled").await;
    assert!(!bench.configs.read().unwrap()["t-1"].paused);

    writer
        .write(
            TriggerWrite::Deleted,
            "t-1",
            serde_json::json!({ "trigger_id": "t-1", "name": "Inbox" }),
            sam(),
        )
        .await
        .unwrap();
    bench.undo_last("TriggerDeleted").await;
    assert_eq!(bench.trigger_name("t-1").as_deref(), Some("Inbox"));

    // The newest TriggerCreated is the undo's own, so craft Sam's anew.
    writer
        .write(
            TriggerWrite::Created,
            "t-2",
            serde_json::json!({
                "trigger_id": "t-2", "name": "Second", "schedule": [], "timezone": "UTC",
                "run": { "type": "intent", "intent": "x" }, "on": [{ "event_type": "X" }],
            }),
            sam(),
        )
        .await
        .unwrap();
    bench.undo_last("TriggerCreated").await;
    assert_eq!(bench.trigger_name("t-2"), None);
    bench.done().await;
}

#[tokio::test]
async fn an_mcp_server_add_auto_approve_and_tool_change_each_undo() {
    let bench = Bench::new().await;
    let sam = || Some(device("sam-phone"));
    McpServerStore::register(
        &bench.pool,
        &bench.bus,
        "srv",
        "Srv",
        "cmd",
        &[],
        &HashMap::new(),
        sam(),
    )
    .await
    .unwrap();
    // Undo refuses once a later change touched the server, so the
    // registration is undone before anything else changes it.
    bench.undo_last("McpServerRegistered").await;
    assert!(McpServerStore::get(&bench.pool, "srv")
        .await
        .unwrap()
        .is_none());

    McpServerStore::register(
        &bench.pool,
        &bench.bus,
        "srv",
        "Srv",
        "cmd",
        &[],
        &HashMap::new(),
        None,
    )
    .await
    .unwrap();

    bench
        .mcp
        .set_auto_approve("srv", true, sam())
        .await
        .unwrap();
    bench.undo_last("McpServerUpdated").await;
    let server = McpServerStore::get(&bench.pool, "srv")
        .await
        .unwrap()
        .unwrap();
    assert!(!server.auto_approve);

    bench
        .mcp
        .set_tool_disabled("srv", "mcp__srv__alpha", true, sam())
        .await
        .unwrap();
    bench.undo_last("McpServerDisabledToolsChanged").await;
    let server = McpServerStore::get(&bench.pool, "srv")
        .await
        .unwrap()
        .unwrap();
    assert!(server.disabled_tools.is_empty());

    McpServerStore::register(
        &bench.pool,
        &bench.bus,
        "srv",
        "Srv",
        "cmd2",
        &[],
        &HashMap::new(),
        sam(),
    )
    .await
    .unwrap();
    let (_, change) = bench.last_change("McpServerRegistered").await;
    assert_eq!(change.undo, None, "a re-registration's old env is sealed");
    bench.done().await;
}

#[tokio::test]
async fn a_deleted_credential_comes_back_without_its_value() {
    let bench = Bench::new().await;
    let secret = "s3cret-value-never-in-events";
    CredentialStore::upsert(
        &bench.pool,
        &bench.bus,
        "github",
        &["https://api.github.com".to_string()],
        AuthType::Bearer,
        secret,
        Some("Authorization"),
        Some("GITHUB_TOKEN"),
        Some(device("sam-phone")),
    )
    .await
    .unwrap();
    let id = CredentialStore::id_of(&bench.pool, "github", AuthType::Bearer)
        .await
        .unwrap()
        .unwrap();
    CredentialStore::delete(&bench.pool, &bench.bus, id, Some(device("sam-phone")))
        .await
        .unwrap();

    let change = bench.undo_last("CredentialDeleted").await;
    assert!(!change.what.contains(secret));
    let back = CredentialStore::get(&bench.pool, "github")
        .await
        .unwrap()
        .expect("restored");
    assert_eq!(back.stored_value(), "", "the value is never restored");
    assert_eq!(back.base_urls, vec!["https://api.github.com".to_string()]);
    assert_eq!(back.env_var_name.as_deref(), Some("GITHUB_TOKEN"));

    let leaks: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE payload::text LIKE '%' || $1 || '%'")
            .bind(secret)
            .fetch_one(&bench.pool)
            .await
            .unwrap();
    assert_eq!(leaks, 0, "a secret value never reaches an event");
    bench.done().await;
}

#[tokio::test]
async fn an_added_credential_undoes_by_deleting_it() {
    let bench = Bench::new().await;
    CredentialStore::upsert(
        &bench.pool,
        &bench.bus,
        "openai",
        &[],
        AuthType::Bearer,
        "value",
        None,
        None,
        Some(device("sam-phone")),
    )
    .await
    .unwrap();
    bench.undo_last("CredentialCreated").await;
    assert!(CredentialStore::get(&bench.pool, "openai")
        .await
        .unwrap()
        .is_none());

    CredentialStore::upsert(
        &bench.pool,
        &bench.bus,
        "openai",
        &[],
        AuthType::Bearer,
        "rotated",
        None,
        None,
        Some(device("sam-phone")),
    )
    .await
    .unwrap();
    CredentialStore::upsert(
        &bench.pool,
        &bench.bus,
        "openai",
        &[],
        AuthType::Bearer,
        "rotated-again",
        None,
        None,
        Some(device("sam-phone")),
    )
    .await
    .unwrap();
    let (_, change) = bench.last_change("CredentialUpdated").await;
    assert_eq!(change.undo, None, "a value change cannot be undone");
    bench.done().await;
}

/// Undo the stored change `event_id` as Kim.
async fn undo_as_kim(bench: &Bench, event_id: Uuid) -> bool {
    let change = stored_change(&bench.pool, event_id).await.unwrap().unwrap();
    apply_undo(
        &bench.targets(),
        event_id,
        change.undo.expect("undoable"),
        device("kim-phone"),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn undo_refuses_once_a_later_change_touched_the_same_thing() {
    let bench = Bench::new().await;
    let writer = bench.targets().triggers;
    let sam = || Some(device("sam-phone"));
    writer
        .write(
            TriggerWrite::Created,
            "t-1",
            serde_json::json!({
                "trigger_id": "t-1", "name": "Inbox", "slug": "inbox", "schedule": [],
                "timezone": "UTC", "run": { "type": "intent", "intent": "triage" },
                "on": [{ "event_type": "MailReceived" }],
            }),
            sam(),
        )
        .await
        .unwrap();
    for name in ["Sam's name", "Kim's name"] {
        writer
            .write(
                TriggerWrite::Updated,
                "t-1",
                serde_json::json!({ "trigger_id": "t-1", "name": name }),
                sam(),
            )
            .await
            .unwrap();
    }
    let sams: Uuid = sqlx::query_scalar(
        "SELECT id FROM events WHERE event_type = 'TriggerUpdated' ORDER BY sequence LIMIT 1",
    )
    .fetch_one(&bench.pool)
    .await
    .unwrap();
    assert!(!undo_as_kim(&bench, sams).await, "Kim's later edit stands");
    assert_eq!(bench.trigger_name("t-1").as_deref(), Some("Kim's name"));
    bench.done().await;
}

#[tokio::test]
async fn a_restored_trigger_mints_a_fresh_slug_when_its_own_is_taken() {
    let bench = Bench::new().await;
    let writer = bench.targets().triggers;
    let sam = || Some(device("sam-phone"));
    let created = |id: &str| {
        serde_json::json!({
            "trigger_id": id, "name": "Daily digest", "slug": "daily-digest", "schedule": [],
            "timezone": "UTC", "run": { "type": "intent", "intent": "digest" },
            "on": [{ "event_type": "X" }],
        })
    };
    writer
        .write(TriggerWrite::Created, "t-1", created("t-1"), sam())
        .await
        .unwrap();
    writer
        .write(
            TriggerWrite::Deleted,
            "t-1",
            serde_json::json!({ "trigger_id": "t-1", "name": "Daily digest" }),
            sam(),
        )
        .await
        .unwrap();
    let (deleted, _) = bench.last_change("TriggerDeleted").await;
    writer
        .write(TriggerWrite::Created, "t-2", created("t-2"), sam())
        .await
        .unwrap();

    assert!(undo_as_kim(&bench, deleted).await);
    let slug = |id: &str| bench.configs.read().unwrap()[id].slug.clone();
    assert_eq!(slug("t-2"), "daily-digest");
    assert_ne!(
        slug("t-1"),
        "daily-digest",
        "two triggers never share a slug"
    );
    bench.done().await;
}

#[tokio::test]
async fn a_restored_credential_never_overwrites_a_live_one() {
    let bench = Bench::new().await;
    let save = |auth_type, value: &'static str| {
        CredentialStore::upsert(
            &bench.pool,
            &bench.bus,
            "github",
            &[],
            auth_type,
            value,
            None,
            None,
            Some(device("sam-phone")),
        )
    };
    save(AuthType::Bearer, "old").await.unwrap();
    let id = CredentialStore::id_of(&bench.pool, "github", AuthType::Bearer)
        .await
        .unwrap()
        .unwrap();
    CredentialStore::delete(&bench.pool, &bench.bus, id, Some(device("sam-phone")))
        .await
        .unwrap();
    let (deleted, change) = bench.last_change("CredentialDeleted").await;
    save(AuthType::ApiKey, "kims-real-token").await.unwrap();

    assert!(!undo_as_kim(&bench, deleted).await);
    let Some(ConfigUndo::RestoreCredential {
        service_name,
        metadata,
    }) = change.undo
    else {
        panic!("a restore");
    };
    assert!(
        !CredentialStore::restore(&bench.pool, &bench.bus, &service_name, &metadata, None)
            .await
            .unwrap(),
        "the store refuses too, whatever the type"
    );
    let live = CredentialStore::get(&bench.pool, "github")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.stored_value(), "kims-real-token");
    bench.done().await;
}
