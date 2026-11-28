use serde_json::Value;
use uuid::Uuid;

use crate::core::credentials::{AuthType, CredentialMetadata, CredentialStore};
use crate::core::repositories::RepositoryStore;
use crate::engine::event_bus::SystemEvent;
use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::engine::trigger_writes::{TriggerWrite, PREVIOUS_KEY};
use crate::engine::LucidosEngine;
use crate::gateway_users::UserSnapshot;
use crate::scheduler::notifications::Tap;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub const CONFIG_CHANGE_EVENTS: [&str; 14] = [
    "RepositoryAdded",
    "RepositoryRemoved",
    "TriggerCreated",
    "TriggerUpdated",
    "TriggerDeleted",
    "TriggerEnabled",
    "TriggerDisabled",
    "McpServerRegistered",
    "McpServerUpdated",
    "McpServerDisabledToolsChanged",
    "McpServerRemoved",
    "CredentialCreated",
    "CredentialUpdated",
    "CredentialDeleted",
];

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigUndo {
    RemoveRepository {
        repo_id: Uuid,
    },
    RestoreRepository {
        repo_id: Uuid,
        name: String,
        path: String,
        description: Option<String>,
    },
    WriteTrigger {
        write: TriggerWrite,
        trigger_id: String,
        payload: Value,
    },
    RemoveMcpServer {
        server_id: String,
    },
    SetMcpAutoApprove {
        server_id: String,
        auto_approve: bool,
    },
    SetMcpToolDisabled {
        server_id: String,
        wire_name: String,
        disabled: bool,
    },
    DeleteCredential {
        service_name: String,
        auth_type: AuthType,
    },
    RestoreCredential {
        service_name: String,
        metadata: CredentialMetadata,
    },
}

impl ConfigUndo {
    pub fn needs_the_owner(&self, current: Option<&crate::triggers::TriggerConfig>) -> bool {
        let ConfigUndo::WriteTrigger { write, payload, .. } = self else {
            return false;
        };
        if !matches!(write, TriggerWrite::Created | TriggerWrite::Updated) {
            return false;
        }
        match current {
            None => {
                payload.get("coding_agent_grant").and_then(Value::as_bool) == Some(true)
                    || !crate::triggers::config::parse_side_effect_grant(
                        payload.get("side_effect_grant"),
                    )
                    .is_empty()
                    || !crate::triggers::config::parse_program_grants(payload).is_empty()
                    || payload
                        .get("folder_grants")
                        .and_then(Value::as_array)
                        .is_some_and(|grants| !grants.is_empty())
            }
            Some(current) => {
                let mut next = current.clone();
                next.apply_update(payload);
                current.widens_a_grant(&next)
                    || next.folder_grants != current.folder_grants
                    || next.program_denies != current.program_denies
                    || next.folder_denies != current.folder_denies
                    || current.unowned_edit_clears_grants(&next)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigChange {
    pub title: &'static str,
    pub what: String,
    pub actor: Option<MessageOrigin>,
    pub undo: Option<ConfigUndo>,
}

pub fn change_of(event_type: &str, payload: &Value) -> Option<ConfigChange> {
    if event_type.starts_with("Trigger") {
        return trigger_change(event_type, payload);
    }
    let event = SystemEvent::from_stored(payload.clone()).ok()?;
    let (title, what, actor, undo) = match event {
        SystemEvent::RepositoryAdded {
            repo_id,
            name,
            actor,
            ..
        } => (
            "Repository added",
            format!("added the repository \"{name}\""),
            actor,
            Uuid::parse_str(&repo_id)
                .ok()
                .map(|repo_id| ConfigUndo::RemoveRepository { repo_id }),
        ),
        SystemEvent::RepositoryRemoved {
            repo_id,
            name: Some(name),
            root_path: Some(path),
            description,
            actor,
        } => (
            "Repository removed",
            format!("removed the repository \"{name}\""),
            actor,
            Uuid::parse_str(&repo_id)
                .ok()
                .map(|repo_id| ConfigUndo::RestoreRepository {
                    repo_id,
                    name,
                    path,
                    description,
                }),
        ),
        SystemEvent::McpServerRegistered {
            server_id,
            name,
            created,
            actor,
        } => (
            if created {
                "MCP server added"
            } else {
                "MCP server changed"
            },
            format!(
                "{} the MCP server \"{name}\"",
                if created { "added" } else { "changed" }
            ),
            actor,
            created.then_some(ConfigUndo::RemoveMcpServer { server_id }),
        ),
        SystemEvent::McpServerUpdated {
            server_id,
            auto_approve,
            actor,
        } => (
            "MCP server auto-approve changed",
            format!(
                "switched auto-approve {} for the MCP server \"{server_id}\"",
                on_off(auto_approve)
            ),
            actor,
            Some(ConfigUndo::SetMcpAutoApprove {
                server_id,
                auto_approve: !auto_approve,
            }),
        ),
        SystemEvent::McpServerDisabledToolsChanged {
            server_id,
            disabled_tools,
            previous,
            actor,
        } => {
            let toggled = previous.and_then(|previous| one_toggled(&previous, &disabled_tools));
            let what = match &toggled {
                Some((tool, disabled)) => format!(
                    "switched {} the tool {tool} of the MCP server \"{server_id}\"",
                    on_off(!disabled)
                ),
                None => format!("changed which tools of the MCP server \"{server_id}\" are off"),
            };
            (
                "MCP tools changed",
                what,
                actor,
                toggled.map(|(wire_name, disabled)| ConfigUndo::SetMcpToolDisabled {
                    server_id,
                    wire_name,
                    disabled: !disabled,
                }),
            )
        }
        SystemEvent::McpServerRemoved { server_id, actor } => (
            "MCP server removed",
            format!("removed the MCP server \"{server_id}\""),
            actor,
            None,
        ),
        SystemEvent::CredentialCreated {
            service_name,
            auth_type,
            actor,
        } => (
            "Credential added",
            format!("added the credential \"{service_name}\""),
            actor,
            Some(ConfigUndo::DeleteCredential {
                service_name,
                auth_type,
            }),
        ),
        SystemEvent::CredentialUpdated {
            service_name,
            actor,
        } => (
            "Credential changed",
            format!("changed the credential \"{service_name}\""),
            actor,
            None,
        ),
        SystemEvent::CredentialDeleted {
            service_name,
            metadata,
            actor,
        } => (
            "Credential deleted",
            format!("deleted the credential \"{service_name}\""),
            actor,
            metadata.map(|metadata| ConfigUndo::RestoreCredential {
                service_name,
                metadata,
            }),
        ),
        _ => return None,
    };
    Some(ConfigChange {
        title,
        what,
        actor,
        undo,
    })
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

fn one_toggled(previous: &[String], now: &[String]) -> Option<(String, bool)> {
    let switched_off = now.iter().filter(|t| !previous.contains(t));
    let switched_on = previous.iter().filter(|t| !now.contains(t));
    let mut toggled = switched_off
        .map(|t| (t.clone(), true))
        .chain(switched_on.map(|t| (t.clone(), false)));
    let first = toggled.next()?;
    toggled.next().is_none().then_some(first)
}

fn trigger_change(event_type: &str, payload: &Value) -> Option<ConfigChange> {
    if payload["self_deleting"].as_bool() == Some(true) {
        return None;
    }
    let trigger_id = payload["trigger_id"].as_str()?.to_string();
    let previous = payload.get(PREVIOUS_KEY).filter(|p| p.is_object());
    let name = payload["name"]
        .as_str()
        .or_else(|| previous.and_then(|p| p["name"].as_str()))
        .unwrap_or(&trigger_id)
        .to_string();
    let actor = payload
        .get(SystemEvent::ACTOR_KEY)
        .and_then(|a| serde_json::from_value(a.clone()).ok());
    let write = |write, payload: Value| ConfigUndo::WriteTrigger {
        write,
        trigger_id: trigger_id.clone(),
        payload,
    };
    let (title, verb, undo) = match event_type {
        "TriggerCreated" => (
            "Trigger created",
            "created",
            Some(write(
                TriggerWrite::Deleted,
                serde_json::json!({ "trigger_id": trigger_id, "name": name }),
            )),
        ),
        "TriggerUpdated" => (
            "Trigger changed",
            "changed",
            previous.map(|p| write(TriggerWrite::Updated, p.clone())),
        ),
        "TriggerDeleted" => (
            "Trigger deleted",
            "deleted",
            previous.map(|p| write(TriggerWrite::Created, p.clone())),
        ),
        "TriggerDisabled" => (
            "Trigger paused",
            "paused",
            Some(write(
                TriggerWrite::Updated,
                serde_json::json!({ "trigger_id": trigger_id, "paused": false }),
            )),
        ),
        "TriggerEnabled" => (
            "Trigger resumed",
            "resumed",
            Some(write(
                TriggerWrite::Disabled,
                serde_json::json!({ "trigger_id": trigger_id, "paused": true }),
            )),
        ),
        _ => return None,
    };
    Some(ConfigChange {
        title,
        what: format!("{verb} the trigger \"{name}\""),
        actor,
        undo,
    })
}

fn who(actor: &MessageOrigin, snapshot: &UserSnapshot) -> String {
    match snapshot.user_of_origin(actor) {
        Some(user) => user.name,
        None if actor.mode() == ActorMode::Agent => "An agent".to_string(),
        None => "Someone".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigNotice {
    pub title: String,
    pub message: String,
    pub tap: Tap,
    pub audience: Option<Vec<String>>,
}

pub fn notice(
    change: &ConfigChange,
    event_id: Uuid,
    snapshot: &UserSnapshot,
) -> Option<ConfigNotice> {
    if snapshot.user_count() < 2 {
        return None;
    }
    let actor = change.actor.as_ref()?;
    let audience = match snapshot.user_of_origin(actor) {
        Some(maker) => {
            let others: std::collections::BTreeSet<String> = snapshot
                .user_ids()
                .into_iter()
                .filter(|id| *id != maker.id)
                .flat_map(|id| snapshot.devices_of(id))
                .map(str::to_string)
                .collect();
            if others.is_empty() {
                return None;
            }
            Some(others.into_iter().collect())
        }
        None => None,
    };
    Some(ConfigNotice {
        title: change.title.to_string(),
        message: format!("{} {}.", who(actor, snapshot), change.what),
        tap: match change.undo {
            Some(_) => Tap::UndoConfigChange { event_id },
            None => Tap::Modal,
        },
        audience,
    })
}

pub async fn stored_change(
    pool: &sqlx::PgPool,
    event_id: Uuid,
) -> Result<Option<ConfigChange>, sqlx::Error> {
    let row: Option<(String, Value)> =
        sqlx::query_as("SELECT event_type, payload FROM events WHERE id = $1")
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(event_type, payload)| change_of(&event_type, &payload)))
}

fn subject_key(event_type: &str) -> Option<&'static str> {
    [
        ("Trigger", "trigger_id"),
        ("Repository", "repo_id"),
        ("McpServer", "server_id"),
        ("Credential", "service_name"),
    ]
    .into_iter()
    .find(|(prefix, _)| event_type.starts_with(prefix))
    .map(|(_, key)| key)
}

/// INVARIANT: Whether a config change after event `event_id` touched the same thing.
/// Undo then refuses, so it never overwrites a later edit. A trigger's
/// payload holds its fields at the top level, every other event under `data`.
async fn changed_since(pool: &sqlx::PgPool, event_id: Uuid) -> Result<bool, sqlx::Error> {
    let row: Option<(String, Value, i64)> =
        sqlx::query_as("SELECT event_type, payload, sequence FROM events WHERE id = $1")
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
    let Some((event_type, payload, sequence)) = row else {
        return Ok(false);
    };
    let Some(key) = subject_key(&event_type) else {
        return Ok(false);
    };
    let Some(subject) = payload
        .get("data")
        .unwrap_or(&payload)
        .get(key)
        .and_then(Value::as_str)
    else {
        return Ok(false);
    };
    let family: Vec<&str> = CONFIG_CHANGE_EVENTS
        .into_iter()
        .filter(|t| subject_key(t) == Some(key))
        .collect();
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events WHERE sequence > $1 AND event_type = ANY($2) \
         AND COALESCE(payload->'data'->>$3, payload->>$3) = $4)",
    )
    .bind(sequence)
    .bind(&family)
    .bind(key)
    .bind(subject)
    .fetch_one(pool)
    .await
}

fn trigger_write_applies(
    write: TriggerWrite,
    current: Option<&crate::triggers::TriggerConfig>,
    payload: &Value,
) -> bool {
    match (write, current) {
        (TriggerWrite::Created, current) => current.is_none(),
        (_, None) => false,
        (TriggerWrite::Deleted, Some(_)) => true,
        (TriggerWrite::Disabled, Some(config)) => !config.paused,
        (TriggerWrite::Updated, Some(config)) => {
            let mut next = config.clone();
            next.apply_update(payload);
            next.to_created_payload() != config.to_created_payload()
        }
    }
}

pub struct UndoTargets<'a> {
    pub pool: &'a sqlx::PgPool,
    pub bus: &'a crate::engine::event_bus::EventBus,
    pub triggers: crate::engine::trigger_writes::TriggerRegistryWriter<'a>,
    pub mcp: &'a crate::mcp::McpManager,
}

impl LucidosEngine {
    pub fn undo_targets(&self) -> UndoTargets<'_> {
        UndoTargets {
            pool: self.pool(),
            bus: &self.event_bus,
            triggers: self.trigger_registry_writer(),
            mcp: &self.mcp_manager,
        }
    }
}

pub async fn apply_undo(
    targets: &UndoTargets<'_>,
    event_id: Uuid,
    undo: ConfigUndo,
    actor: MessageOrigin,
) -> Result<bool, BoxError> {
    let UndoTargets {
        pool,
        bus,
        triggers,
        mcp,
    } = targets;
    if changed_since(pool, event_id).await? {
        return Ok(false);
    }
    let actor = Some(actor);
    match undo {
        ConfigUndo::RemoveRepository { repo_id } => {
            Ok(RepositoryStore::unregister(pool, bus, repo_id, actor).await?)
        }
        ConfigUndo::RestoreRepository {
            repo_id,
            name,
            path,
            description,
        } => {
            let root = std::path::Path::new(&path);
            let taken = RepositoryStore::list(pool)
                .await?
                .iter()
                .any(|r| r.id == repo_id || r.path == path);
            if !root.exists() || taken {
                return Ok(false);
            }
            let root_commit_sha = crate::engine::git_ops::root_commit_sha(root).await;
            RepositoryStore::register(
                pool,
                bus,
                &name,
                &path,
                description.as_deref(),
                root_commit_sha.as_deref(),
                actor,
            )
            .await?;
            Ok(true)
        }
        ConfigUndo::WriteTrigger {
            write,
            trigger_id,
            payload,
        } => Ok(triggers
            .write_if(write, &trigger_id, payload, actor, |current, payload| {
                trigger_write_applies(write, current, payload)
            })
            .await?),
        ConfigUndo::RemoveMcpServer { server_id } => {
            Ok(mcp.remove_server(&server_id, actor).await?)
        }
        ConfigUndo::SetMcpAutoApprove {
            server_id,
            auto_approve,
        } => {
            let Some(server) = crate::core::McpServerStore::get(pool, &server_id).await? else {
                return Ok(false);
            };
            if server.auto_approve == auto_approve {
                return Ok(false);
            }
            mcp.set_auto_approve(&server_id, auto_approve, actor)
                .await?;
            Ok(true)
        }
        ConfigUndo::SetMcpToolDisabled {
            server_id,
            wire_name,
            disabled,
        } => {
            let Some(server) = crate::core::McpServerStore::get(pool, &server_id).await? else {
                return Ok(false);
            };
            if server.disabled_tools.contains(&wire_name) == disabled {
                return Ok(false);
            }
            mcp.set_tool_disabled(&server_id, &wire_name, disabled, actor)
                .await?;
            Ok(true)
        }
        ConfigUndo::DeleteCredential {
            service_name,
            auth_type,
        } => match CredentialStore::id_of(pool, &service_name, auth_type).await? {
            Some(id) => Ok(CredentialStore::delete(pool, bus, id, actor).await?),
            None => Ok(false),
        },
        ConfigUndo::RestoreCredential {
            service_name,
            metadata,
        } => Ok(CredentialStore::restore(pool, bus, &service_name, &metadata, actor).await?),
    }
}

#[cfg(test)]
#[path = "config_change_tests.rs"]
mod tests;
