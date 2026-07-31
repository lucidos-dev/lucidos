//! Handler for the grouped `notifications` tool — the agent-facing path to the
//! notification inbox (list / mark_read / mark_all_read). Schema is built from
//! the capability parity manifest (`capability_manifest::build_llm_tool`); this
//! is the in-process handler the schema dispatches to.
//!
//! Sending a notification stays on the separate `send_notification` tool (its
//! rich `tap` schema is a poor fit for the grouped union). The pre-consolidation
//! flat `read_notifications` tool name still dispatches here (action defaults to
//! `list`) so cached prompts/threads keep working — see `Domain::llm_aliases`.
//!
//! Mutating actions emit `NotificationRead` / `NotificationsAllRead` with
//! `actor: None`: the agent is the actor and attribution flows via the
//! surrounding `MessageReceived` / `ToolCalled` events, exactly as
//! `execute_emit_event` / `execute_manage_models` do.

use uuid::Uuid;

use super::{LucidosEngine, ToolOutcome};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::llm::tool_names as tn;
use crate::scheduler::NotificationStore;

impl LucidosEngine {
    /// Dispatch a grouped `notifications` tool call. `name` is the invoked tool
    /// name so the legacy `read_notifications` alias can default `action` to
    /// `list`.
    pub(crate) async fn execute_notifications(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> ToolOutcome {
        // The retired flat tool only ever listed, and callers pass no `action`.
        let default_action = if name == tn::READ_NOTIFICATIONS {
            "list"
        } else {
            ""
        };
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(default_action);

        match action {
            "list" => self.notifications_list(args).await,
            "mark_read" => self.notifications_mark_read(args).await,
            "mark_all_read" => self.notifications_mark_all_read().await,
            "" => Err(
                "Error: action is required (one of: list, mark_read, mark_all_read)".to_string(),
            ),
            other => Err(format!(
                "Error: unknown action '{}'. Use one of: list, mark_read, mark_all_read.",
                other
            )),
        }
    }

    async fn notifications_list(&self, args: &serde_json::Value) -> ToolOutcome {
        let filter = args
            .get("filter")
            .and_then(|v| v.as_str())
            .unwrap_or("unread");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(20)
            .clamp(1, 50);

        let notifications =
            match NotificationStore::get_filtered(&self.pool, filter, limit, None).await {
                Ok(n) => n,
                Err(e) => return Err(format!("Error: failed to read notifications: {}", e)),
            };

        if notifications.is_empty() {
            return Ok(format!("No {} notifications.", filter));
        }

        let items: Vec<serde_json::Value> = notifications
            .iter()
            .map(|n| {
                serde_json::json!({
                    "id": n.id.to_string(),
                    "title": n.title,
                    "message": n.message,
                    "read": n.read,
                    "created_at": n.created_at.to_rfc3339(),
                    "task_id": n.task_id.map(|id| id.to_string()),
                })
            })
            .collect();

        serde_json::to_string_pretty(&items)
            .map_err(|e| format!("Error: failed to serialise notifications: {}", e))
    }

    async fn notifications_mark_read(&self, args: &serde_json::Value) -> ToolOutcome {
        let raw = args
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let id = match raw {
            Some(s) => match Uuid::parse_str(s) {
                Ok(id) => id,
                Err(_) => {
                    return Err(format!(
                    "Error: 'id' must be a notification UUID (from the 'list' action), got '{}'.",
                    s
                ))
                }
            },
            None => {
                return Err(
                    "Error: 'id' is required for mark_read (a notification UUID from 'list')."
                        .to_string(),
                )
            }
        };

        let marked = match NotificationStore::mark_read(&self.pool, id).await {
            Ok(b) => b,
            Err(e) => return Err(format!("Error: failed to mark notification read: {}", e)),
        };
        if !marked {
            return Ok(format!(
                "Notification {} was already read or does not exist.",
                id
            ));
        }
        self.event_bus
            .emit(BusEvent::System(SystemEvent::NotificationRead {
                id: id.to_string(),
                actor: None,
            }))
            .await
            .map_err(|e| format!("Error: failed to emit NotificationRead: {}", e))?;
        Ok(format!(
            "[ACTION COMPLETED] Notification {} marked read.",
            id
        ))
    }

    async fn notifications_mark_all_read(&self) -> ToolOutcome {
        let count = match NotificationStore::mark_all_read(&self.pool).await {
            Ok(c) => c,
            Err(e) => {
                return Err(format!(
                    "Error: failed to mark all notifications read: {}",
                    e
                ))
            }
        };
        if count == 0 {
            return Ok("No unread notifications to mark read.".to_string());
        }
        self.event_bus
            .emit(BusEvent::System(SystemEvent::NotificationsAllRead {
                actor: None,
            }))
            .await
            .map_err(|e| format!("Error: failed to emit NotificationsAllRead: {}", e))?;
        Ok(format!(
            "[ACTION COMPLETED] Marked {} notification(s) read; inbox is clear.",
            count
        ))
    }
}

/// Action discriminator values the grouped handler recognises. A unit test
/// asserts this matches the notifications domain's manifest actions, so adding a
/// manifest operation without a handler arm (or vice-versa) fails `cargo test`.
#[cfg(test)]
pub(crate) fn recognized_actions() -> Vec<&'static str> {
    vec!["list", "mark_read", "mark_all_read"]
}

#[cfg(test)]
mod tests {
    use crate::capability_manifest::domain_for_tool;

    #[test]
    fn handler_actions_match_manifest() {
        let domain = domain_for_tool("notifications").expect("notifications domain");
        let mut manifest = domain.actions();
        manifest.sort();
        let mut handled = super::recognized_actions();
        handled.sort();
        assert_eq!(
            manifest, handled,
            "notifications handler actions drifted from the manifest \
             (capability_manifest::DOMAINS) — update both together"
        );
    }
}
