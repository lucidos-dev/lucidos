//! User triggers - visible triggers that create notifications and artifacts

use crate::api::SharedEngine;
use crate::engine::thread_events::TriggerInvocation;
use crate::scheduler::trigger_id_to_uuid;
use crate::triggers::{TriggerConfig, TriggerRun};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

/// Suppress duplicate error notifications for the same task within this window.
const ERROR_DEDUP_MINUTES: i64 = 30;
const ERROR_TITLE_SUFFIX: &str = " failed";
/// Env var name injected into script triggers fired by an event.
const TRIGGER_EVENT_PAYLOAD_ENV: &str = "TRIGGER_EVENT_PAYLOAD";

tokio::task_local! {
    /// The trigger ID of the currently executing trigger.
    /// Read by `execute_send_notification` to auto-attach context to notifications.
    pub static ACTIVE_TRIGGER_ID: String;
    /// Current recursion depth for event-triggered chains.
    /// Set by `handle_domain_event` before spawning; read by `emit_domain_event`
    /// to propagate depth in the `DomainEvent` broadcast.
    pub static EVENT_TRIGGER_DEPTH: u32;
    /// Thread ID of the event that fired the currently executing trigger
    /// (set only when the trigger was matched on a `BusEvent::Thread`).
    /// Read by `execute_send_notification` so a push tap can deep-link back
    /// to the originating thread instead of the trigger's own LLM thread.
    pub static ORIGIN_THREAD_ID: uuid::Uuid;
}

/// True when the currently running trigger is deleting itself — i.e. the LLM
/// called `delete_trigger` with its own trigger ID. The scheduler's
/// `TriggerDeleted` handler reads this flag (carried in the event payload) to
/// skip cancellation, so the in-flight agentic loop finishes cleanly and emits
/// `ResponseGenerated`. Without this, the running task would be torn down
/// mid-tool and the thread would stay stuck "running" with no terminal event.
pub fn is_self_deleting_trigger(trigger_id: &str) -> bool {
    ACTIVE_TRIGGER_ID
        .try_with(|active| *active == trigger_id)
        .unwrap_or(false)
}

/// Emit a `NotificationCreated` for a trigger failure and send push to all devices.
async fn emit_failure_notification(
    engine: &SharedEngine,
    pool: &PgPool,
    config: &TriggerConfig,
    app_id: String,
    title: String,
    message: String,
) {
    let notification_id = uuid::Uuid::new_v4();
    if let Err(emit_err) = engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::NotificationCreated {
                id: notification_id.to_string(),
                title: title.clone(),
                message: message.clone(),
                task_id: Some(config.id.clone()),
                app_id: Some(app_id),
            },
        ))
        .await
    {
        log!(
            "[Scheduler] Failed to emit failure notification for '{}': {}",
            config.name,
            emit_err
        );
    }
    crate::scheduler::push::send_push_to_all(pool, &title, &message, Some(notification_id)).await;
}

pub async fn execute_user_task(
    engine: SharedEngine,
    pool: &PgPool,
    config: &TriggerConfig,
    invocation: TriggerInvocation,
    event_payload: Option<&serde_json::Value>,
    external_cancel: Option<CancellationToken>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match &config.run {
        TriggerRun::Script { path } => {
            if path.contains("..") || path.starts_with('/') || path.starts_with('\\') {
                return Err(format!("Invalid script path: {}", path).into());
            }
            execute_script_task(engine, pool, config, path, event_payload).await
        }
        TriggerRun::Intent { text, knowhow } => {
            let kh_dirs = engine.knowhow_dirs();
            let knowhow_context =
                crate::core::knowhow::load_knowhow_sections_merged(&kh_dirs, knowhow);
            let instructions = format!("{}{}", text, knowhow_context);
            execute_llm_task(
                engine,
                pool,
                config,
                &config.id,
                &instructions,
                invocation,
                event_payload,
                external_cancel,
            )
            .await
        }
    }
}

/// Execute a script trigger — runs the script file directly without LLM.
async fn execute_script_task(
    engine: SharedEngine,
    pool: &PgPool,
    config: &TriggerConfig,
    script_path: &str,
    event_payload: Option<&serde_json::Value>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let task_uuid = trigger_id_to_uuid(&config.id);

    log!(
        "[Scheduler] Running script trigger '{}': {}",
        config.name,
        script_path
    );

    // Resolve script path across taxonomy versions:
    // - New taxonomy: "triggers/oura-import/scripts/run.py" → data/{path}
    // - Legacy stored: "oura-import/run.py" → try data/triggers/{dir}/scripts/{file}
    // - Ancient legacy: → data/scripts/{path}
    let full_script_path = if std::path::Path::new(script_path).is_relative() {
        let new_path = format!("data/{}", script_path);
        if engine.workspace_path().join(&new_path).exists() {
            new_path
        } else {
            // Legacy "dirname/run.py" → try new taxonomy location
            let parts: Vec<&str> = script_path.splitn(2, '/').collect();
            let trigger_path = if parts.len() == 2 {
                format!("data/triggers/{}/scripts/{}", parts[0], parts[1])
            } else {
                String::new()
            };
            if !trigger_path.is_empty() && engine.workspace_path().join(&trigger_path).exists() {
                trigger_path
            } else {
                format!("data/scripts/{}", script_path)
            }
        }
    } else {
        script_path.to_string()
    };

    let extra_env = build_event_env(event_payload);

    match engine
        .execute_script(&full_script_path, &[], &extra_env)
        .await
    {
        Ok(output) => {
            let summary = if output.len() > 500 {
                format!("{}...", output.chars().take(497).collect::<String>())
            } else {
                output
            };
            engine
                .record_trigger_completed(task_uuid, &config.name, &summary, None)
                .await?;
            log!("[Scheduler] Script trigger '{}' completed", config.name);
            Ok(())
        }
        Err(e) => {
            let title = format!("{}{}", config.name, ERROR_TITLE_SUFFIX);
            let message = format!("[trigger: {}] {}", config.id, e);
            emit_failure_notification(&engine, pool, config, config.id.clone(), title, message)
                .await;
            Err(e)
        }
    }
}

/// Execute an LLM prompt trigger — sends instructions as prompt to the LLM.
#[allow(clippy::too_many_arguments)]
async fn execute_llm_task(
    engine: SharedEngine,
    pool: &PgPool,
    config: &TriggerConfig,
    trigger_id: &str,
    instructions: &str,
    invocation: TriggerInvocation,
    event_payload: Option<&serde_json::Value>,
    external_cancel: Option<CancellationToken>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let task_uuid = trigger_id_to_uuid(&config.id);
    let final_instructions = build_trigger_instructions(instructions, event_payload);

    log!(
        "[Scheduler] Executing LLM trigger '{}': {}",
        config.name,
        final_instructions.chars().take(50).collect::<String>()
    );

    let result = match ACTIVE_TRIGGER_ID
        .scope(
            trigger_id.to_string(),
            engine.process_trigger(
                task_uuid,
                &config.name,
                &final_instructions,
                invocation,
                external_cancel,
            ),
        )
        .await
    {
        Ok(r) => {
            // Broadcast "done" so the frontend immediately moves this thread to history.
            engine
                .event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id: r.thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                            text: String::new(),
                            images: vec![],
                            model: None,
                            reasoning_effort: None,
                        },
                        meta: crate::engine::thread_events::EventMeta {
                            channel: Some(crate::engine::thread_events::EventChannel::Trigger),
                            ..crate::engine::thread_events::EventMeta::NONE
                        },
                    },
                    "[Scheduler] trigger completion ResponseGenerated",
                )
                .await;
            r
        }
        Err(e) => {
            let err_str = e.to_string();
            let is_transient = crate::llm::is_transient_error(&err_str);

            // For transient errors, skip notification if we already notified recently
            let notification_task_id = uuid::Uuid::parse_str(&config.id).unwrap_or(task_uuid);
            if is_transient && has_recent_error_notification(pool, notification_task_id).await {
                log!(
                    "[Scheduler] Suppressing duplicate transient error notification for '{}': {}",
                    config.name,
                    err_str
                );
                return Err(e);
            }

            let title = format!("{}{}", config.name, ERROR_TITLE_SUFFIX);
            let message = if is_transient {
                format!("Transient error (will retry on next schedule): {}", err_str)
            } else {
                err_str
            };
            emit_failure_notification(
                &engine,
                pool,
                config,
                trigger_id.to_string(),
                title,
                message,
            )
            .await;
            return Err(e);
        }
    };

    let event_summary = if result.response.len() > 500 {
        format!(
            "{}...",
            result.response.chars().take(497).collect::<String>()
        )
    } else {
        result.response.clone()
    };
    engine
        .record_trigger_completed(
            task_uuid,
            &config.name,
            &event_summary,
            Some(result.thread_id),
        )
        .await?;

    log!("[Scheduler] Trigger '{}' completed", config.name);

    Ok(())
}

/// Build the final instruction string for an LLM trigger, optionally
/// appending the triggering event payload as structured context.
fn build_trigger_instructions(base: &str, event_payload: Option<&serde_json::Value>) -> String {
    match event_payload {
        Some(payload) => format!(
            "{}\n\n## Triggering Event\n\n```json\n{}\n```",
            base,
            serde_json::to_string_pretty(payload).unwrap_or_default()
        ),
        None => base.to_string(),
    }
}

/// Build extra environment variables for a script trigger fired by an event.
fn build_event_env(event_payload: Option<&serde_json::Value>) -> Vec<(String, String)> {
    match event_payload {
        Some(payload) => vec![(
            TRIGGER_EVENT_PAYLOAD_ENV.to_string(),
            serde_json::to_string(payload).unwrap_or_default(),
        )],
        None => vec![],
    }
}

/// Check if an error notification for this task already exists within the dedup window.
async fn has_recent_error_notification(pool: &PgPool, task_id: uuid::Uuid) -> bool {
    let cutoff = chrono::Utc::now() - chrono::Duration::minutes(ERROR_DEDUP_MINUTES);
    let pattern = format!("%{}", ERROR_TITLE_SUFFIX);
    let result: Result<Option<(i64,)>, _> = sqlx::query_as(
        "SELECT 1 FROM notifications WHERE task_id = $1 AND created_at > $2 AND title LIKE $3 LIMIT 1"
    )
    .bind(task_id)
    .bind(cutoff)
    .bind(&pattern)
    .fetch_optional(pool)
    .await;

    matches!(result, Ok(Some(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_trigger_instructions_without_payload() {
        let result = build_trigger_instructions("Do the thing", None);
        assert_eq!(result, "Do the thing");
    }

    #[test]
    fn build_trigger_instructions_with_payload() {
        let payload = json!({"path": "slides.md", "new_value": "hello"});
        let result = build_trigger_instructions("Edit the slide", Some(&payload));
        assert!(result.starts_with("Edit the slide"));
        assert!(result.contains("## Triggering Event"));
        assert!(result.contains("\"path\": \"slides.md\""));
        assert!(result.contains("\"new_value\": \"hello\""));
    }

    #[test]
    fn build_event_env_without_payload() {
        let env = build_event_env(None);
        assert!(env.is_empty());
    }

    #[test]
    fn build_event_env_with_payload() {
        let payload = json!({"sleep_score": 42});
        let env = build_event_env(Some(&payload));
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, TRIGGER_EVENT_PAYLOAD_ENV);
        let parsed: serde_json::Value = serde_json::from_str(&env[0].1).unwrap();
        assert_eq!(parsed["sleep_score"], 42);
    }

    #[tokio::test]
    async fn origin_thread_id_unset_outside_scope_is_observable() {
        // The push deep-link wiring relies on `try_with` returning `Err`
        // outside any scope so the caller can fall back to the current
        // thread. If this ever flips, every `send_notification` call from a
        // plain LLM context would silently inherit a stale thread.
        let result = ORIGIN_THREAD_ID.try_with(|t| *t);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn origin_thread_id_propagates_into_async_scope() {
        // Mirrors how `handle_domain_event` wraps trigger execution: when a
        // thread-scoped event fires the trigger, the originating thread must
        // be readable inside the spawned future so `execute_send_notification`
        // can deep-link the push to that thread.
        let tid = uuid::Uuid::new_v4();
        let observed = ORIGIN_THREAD_ID
            .scope(tid, async { ORIGIN_THREAD_ID.try_with(|t| *t).ok() })
            .await;
        assert_eq!(observed, Some(tid));
    }

    #[tokio::test]
    async fn is_self_deleting_trigger_true_when_ids_match() {
        // The fix relies on this being true exactly when delete_trigger is
        // invoked by the trigger's own running execution. Misidentification
        // either way is the bug — false positive cancels nothing and leaves
        // an orphaned task; false negative re-triggers the original "stuck
        // running" symptom.
        let trigger_id = "abc-123";
        let result = ACTIVE_TRIGGER_ID
            .scope(trigger_id.to_string(), async {
                is_self_deleting_trigger(trigger_id)
            })
            .await;
        assert!(result, "ACTIVE_TRIGGER_ID == arg → must be self-delete");
    }

    #[tokio::test]
    async fn is_self_deleting_trigger_false_when_ids_differ() {
        // Cross-deletion: one trigger's LLM deletes another trigger. Must
        // NOT be flagged as self-delete — that other trigger has no in-flight
        // execution to protect.
        let result = ACTIVE_TRIGGER_ID
            .scope("running-trigger".to_string(), async {
                is_self_deleting_trigger("other-trigger")
            })
            .await;
        assert!(!result);
    }

    #[tokio::test]
    async fn is_self_deleting_trigger_false_outside_scope() {
        // UI-driven deletion (HTTP handler thread, not a trigger task) has
        // no ACTIVE_TRIGGER_ID. Must be treated as external cancel so the
        // running task is signalled to exit.
        assert!(!is_self_deleting_trigger("any-id"));
    }
}
