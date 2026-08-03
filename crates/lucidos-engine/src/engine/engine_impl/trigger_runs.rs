//! Off-schedule trigger runs: firing an existing trigger once, right now.
//!
//! Part of the `LucidosEngine` inherent impl, split from engine_impl.rs.
//!
//! An **off-schedule run** is deliberately *indistinguishable* downstream from
//! a scheduled fire. It submits the same [`ThreadQueueRequest::Cron`] the
//! scheduler submits, so it inherits `ACTIVE_TRIGGER_ID`, `ACTIVE_TASK_COUNT`,
//! the trigger event channel, `go_to_review`, the side-effect grant, the
//! per-trigger concurrency cap, and `record_trigger_executed` without adding a
//! `TriggerInvocation` variant, an event type, or an actor stamp. Nothing
//! downstream has to learn a third case: in particular `catch_up_decision`
//! reads `last_run` as "did this work happen", not "did the schedule fire", so
//! an off-schedule run correctly suppresses a redundant restart-time catch-up.
//!
//! What is *not* indistinguishable is the answer given to the caller. Four
//! situations would otherwise report a run that never happened, and each is
//! refused or reported instead.

use super::super::*;
use crate::triggers::TriggerConfig;

/// Why an off-schedule run request cannot proceed. Each variant is a case where
/// submitting anyway would report a run that does not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunRefusal {
    /// No trigger with this id is registered.
    NotFound { trigger_id: String },
    /// The caller is itself a trigger fire. Refused even when the target is the
    /// firing trigger's own id: self-pause and self-delete terminate, self-run
    /// recurses, and the per-trigger concurrency cap of 1 turns that into a
    /// queue that grows by one entry per fire rather than stopping it.
    InsideTriggerFire {
        active_id: String,
        active_name: String,
    },
    /// The trigger is paused. Submitting would be dropped by the queue executor
    /// with only a log line, which is a quieter version of the bug this whole
    /// operation exists to fix.
    Paused { name: String },
    /// The trigger has no cron schedule, so a payload-less fire is a shape it
    /// has never had: an intent run would find no `## Triggering Event` block
    /// and a script run would get none of the `TRIGGER_EVENT_*` env vars.
    /// Emitting the subscribed event reproduces a real fire faithfully.
    EventOnly {
        name: String,
        event_types: Vec<String>,
    },
}

impl RunRefusal {
    /// User- and agent-facing explanation. Names the trigger and, where there
    /// is one, the action that does work instead.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::NotFound { trigger_id } => {
                format!("No trigger found with ID {}", trigger_id)
            }
            Self::InsideTriggerFire {
                active_id,
                active_name,
            } => format!(
                "Off-schedule runs are disabled during trigger fires. You are currently \
                 executing trigger '{}' (id: {}). Execute the trigger's steps directly \
                 instead; running a trigger from inside a fire recurses.",
                active_name, active_id
            ),
            Self::Paused { name } => format!(
                "Trigger '{}' is paused, so it fires nothing. Resume it first if it should \
                 run now (resuming restores the schedule; it does not itself run anything).",
                name
            ),
            Self::EventOnly { name, event_types } => {
                let subscriptions = if event_types.is_empty() {
                    "no event subscriptions".to_string()
                } else {
                    event_types.join(", ")
                };
                format!(
                    "Trigger '{}' has no cron schedule, so there is no scheduled fire to \
                     reproduce. It runs on events ({}). Emit one of those events instead, \
                     with a payload that passes any per-entry condition.",
                    name, subscriptions
                )
            }
        }
    }
}

/// What an accepted off-schedule run did. `AlreadyRunning` is a real outcome,
/// not an error: cron admission coalesces a redundant fire, and reporting it as
/// started would be a lie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    /// Admitted and running now.
    Started { name: String },
    /// Over capacity; runs when capacity frees. `position` is the 1-based queue
    /// slot when the queue reported one, and `None` when it did not: the
    /// `Overflow` arm of `submit` enqueues the entry but returns `position: 0`,
    /// and 0 is not a slot. Unreachable for a cron entry today (overflow needs
    /// 25 already queued, and the second one would have coalesced), but the
    /// outcome must not invent a slot number if that ever changes.
    Queued {
        name: String,
        position: Option<usize>,
    },
    /// A fire of this trigger was already active or queued, so this request was
    /// coalesced away and nothing new started.
    AlreadyRunning { name: String },
}

impl RunOutcome {
    /// Machine-readable discriminator for API consumers, kebab-case per the
    /// public-API convention. The frontend picks its toast style from this;
    /// `already-running` must never be styled as a successful start.
    pub(crate) fn status(&self) -> &'static str {
        match self {
            Self::Started { .. } => "started",
            Self::Queued { .. } => "queued",
            Self::AlreadyRunning { .. } => "already-running",
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::Started { name } => {
                format!("Started an off-schedule run of trigger '{}'.", name)
            }
            Self::Queued { name, position } => {
                let slot = match position {
                    Some(p) => format!(" at position {}", p),
                    None => String::new(),
                };
                format!(
                    "Queued an off-schedule run of trigger '{}'{} (system at capacity); \
                     it runs when capacity frees.",
                    name, slot
                )
            }
            Self::AlreadyRunning { name } => format!(
                "Trigger '{}' is already running (or queued), so nothing new was started. \
                 Scheduled fires coalesce to at most one pending run per trigger.",
                name
            ),
        }
    }
}

/// Pure precondition check for an off-schedule run, ordered so the most
/// specific refusal wins. `config` is `None` when no trigger has the id;
/// `active_trigger_id` / `active_trigger_name` come from `ACTIVE_TRIGGER_ID` at
/// the call site and are `None` outside a trigger fire.
///
/// Kept pure and separate from the submit so every refusal is unit-testable
/// without an engine, a queue, or a database.
pub(crate) fn check_off_schedule_run(
    trigger_id: &str,
    config: Option<&TriggerConfig>,
    active_trigger_id: Option<&str>,
    active_trigger_name: Option<&str>,
) -> Option<RunRefusal> {
    // The recursion guard comes first: it does not depend on the target
    // existing, and "you cannot do this here" is the more useful answer than
    // "that id is unknown" when a trigger's intent asks to run something.
    if let Some(active_id) = active_trigger_id {
        return Some(RunRefusal::InsideTriggerFire {
            active_id: active_id.to_string(),
            active_name: active_trigger_name.unwrap_or("(unknown)").to_string(),
        });
    }
    let Some(config) = config else {
        return Some(RunRefusal::NotFound {
            trigger_id: trigger_id.to_string(),
        });
    };
    if config.paused {
        return Some(RunRefusal::Paused {
            name: config.name.clone(),
        });
    }
    if config.schedule.is_empty() {
        return Some(RunRefusal::EventOnly {
            name: config.name.clone(),
            event_types: config.on.iter().map(|s| s.event_type.clone()).collect(),
        });
    }
    None
}

impl LucidosEngine {
    /// Fire an existing trigger once, right now, outside its schedule.
    ///
    /// Submits the same [`ThreadQueueRequest::Cron`] the scheduler submits, with
    /// no actor and no cancel token, so the resulting run is indistinguishable
    /// from a scheduled one in every event, projection, and API response.
    ///
    /// Returns as soon as admission is decided; it never awaits
    /// `SubmitOutcome::completion`, because a trigger run is unbounded in
    /// duration and the callers are a chat turn, a CLI invocation, and an HTTP
    /// request.
    pub(crate) async fn run_trigger_off_schedule(
        &self,
        trigger_id: &str,
    ) -> Result<RunOutcome, RunRefusal> {
        let active_id = crate::scheduler::user_tasks::ACTIVE_TRIGGER_ID
            .try_with(|id| id.clone())
            .ok();
        let (config, active_name) = {
            let configs = self.trigger_configs.read().unwrap();
            (
                configs.get(trigger_id).cloned(),
                active_id
                    .as_deref()
                    .and_then(|id| configs.get(id).map(|c| c.name.clone())),
            )
        };

        if let Some(refusal) = check_off_schedule_run(
            trigger_id,
            config.as_ref(),
            active_id.as_deref(),
            active_name.as_deref(),
        ) {
            return Err(refusal);
        }
        let name = config
            .expect("check_off_schedule_run rejects a missing config")
            .name;

        log!(
            "[Triggers] Off-schedule run requested for '{}' ({})",
            name,
            trigger_id
        );
        let outcome = self
            .thread_queue
            .submit(
                crate::engine::thread_queue::ThreadQueueRequest::Cron {
                    trigger_id: trigger_id.to_string(),
                },
                None,
                None,
            )
            .await;

        Ok(if outcome.coalesced {
            RunOutcome::AlreadyRunning { name }
        } else if outcome.admitted {
            RunOutcome::Started { name }
        } else {
            RunOutcome::Queued {
                name,
                // 0 is `submit`'s "no slot reported" (the Overflow arm), not a
                // queue position; Queue reports 1-based.
                position: (outcome.position > 0).then_some(outcome.position),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triggers::{EventSubscription, TriggerRun};

    fn config(schedule: &[&str], on: &[&str], paused: bool) -> TriggerConfig {
        TriggerConfig {
            id: "t-1".to_string(),
            name: "Nightly e2e".to_string(),
            slug: "nightly-e2e".to_string(),
            schedule: schedule.iter().map(|s| s.to_string()).collect(),
            timezone: "UTC".to_string(),
            run: TriggerRun::Intent {
                intent: "run the nightly e2e".to_string(),
            },
            on: on
                .iter()
                .map(|t| EventSubscription {
                    event_type: t.to_string(),
                    condition: None,
                })
                .collect(),
            paused,
            last_run: None,
            last_run_status: None,
            app_id: None,
            go_to_review: false,
            group_id: None,
            side_effect_grant: vec![],
            plugin_id: None,
        }
    }

    #[test]
    fn cron_trigger_passes_every_precondition() {
        let c = config(&["0 0 2 * * *"], &[], false);
        assert_eq!(check_off_schedule_run("t-1", Some(&c), None, None), None);
    }

    #[test]
    fn cron_plus_event_trigger_is_allowed() {
        // It already experiences payload-less scheduled fires, so an
        // off-schedule run exposes no shape it has not had before.
        let c = config(&["0 0 2 * * *"], &["EmailReceived"], false);
        assert_eq!(check_off_schedule_run("t-1", Some(&c), None, None), None);
    }

    #[test]
    fn unknown_id_is_not_found() {
        assert_eq!(
            check_off_schedule_run("nope", None, None, None),
            Some(RunRefusal::NotFound {
                trigger_id: "nope".to_string()
            })
        );
    }

    #[test]
    fn paused_trigger_is_refused_rather_than_silently_dropped() {
        // The queue executor would drop a Cron entry for a paused trigger with
        // only a log line, reporting success to a caller that got nothing.
        let c = config(&["0 0 2 * * *"], &[], true);
        assert_eq!(
            check_off_schedule_run("t-1", Some(&c), None, None),
            Some(RunRefusal::Paused {
                name: "Nightly e2e".to_string()
            })
        );
    }

    #[test]
    fn event_only_trigger_is_refused_and_names_its_events() {
        let c = config(&[], &["UserQuestionAsked", "EmailReceived"], false);
        let refusal = check_off_schedule_run("t-1", Some(&c), None, None).expect("refused");
        assert_eq!(
            refusal,
            RunRefusal::EventOnly {
                name: "Nightly e2e".to_string(),
                event_types: vec!["UserQuestionAsked".to_string(), "EmailReceived".to_string()],
            }
        );
        let msg = refusal.message();
        assert!(msg.contains("UserQuestionAsked"), "{msg}");
        assert!(
            msg.contains("Emit"),
            "must name the action that does work: {msg}"
        );
    }

    /// The recursion guard is unconditional, unlike pause/resume/delete which
    /// permit self-action. A trigger that runs itself queues one entry per fire.
    #[test]
    fn inside_a_fire_is_refused_even_for_a_different_trigger() {
        let c = config(&["0 0 2 * * *"], &[], false);
        assert_eq!(
            check_off_schedule_run("t-1", Some(&c), Some("other-id"), Some("Other")),
            Some(RunRefusal::InsideTriggerFire {
                active_id: "other-id".to_string(),
                active_name: "Other".to_string(),
            })
        );
    }

    #[test]
    fn inside_a_fire_is_refused_for_the_firing_trigger_itself() {
        let c = config(&["0 0 2 * * *"], &[], false);
        assert_eq!(
            check_off_schedule_run("t-1", Some(&c), Some("t-1"), Some("Nightly e2e")),
            Some(RunRefusal::InsideTriggerFire {
                active_id: "t-1".to_string(),
                active_name: "Nightly e2e".to_string(),
            })
        );
    }

    #[test]
    fn refusal_messages_name_the_trigger() {
        assert!(RunRefusal::Paused {
            name: "Nightly e2e".to_string()
        }
        .message()
        .contains("Nightly e2e"));
        assert!(RunRefusal::NotFound {
            trigger_id: "abc".to_string()
        }
        .message()
        .contains("abc"));
    }

    /// The coalesced outcome must never read as "started". That is the exact
    /// lie the `coalesced` flag was added to prevent.
    #[test]
    fn already_running_does_not_claim_a_run_started() {
        let msg = RunOutcome::AlreadyRunning {
            name: "Nightly e2e".to_string(),
        }
        .message();
        assert!(msg.contains("nothing new was started"), "{msg}");
        assert!(!msg.contains("Started an off-schedule run"), "{msg}");
    }

    #[test]
    fn queued_outcome_reports_its_position() {
        let msg = RunOutcome::Queued {
            name: "Nightly e2e".to_string(),
            position: Some(3),
        }
        .message();
        assert!(msg.contains("position 3"), "{msg}");
    }

    /// `submit`'s Overflow arm enqueues the entry but reports `position: 0`,
    /// which is not a slot. The outcome must stay silent about the position
    /// rather than announce "position 0".
    #[test]
    fn queued_outcome_invents_no_position_when_the_queue_reported_none() {
        let msg = RunOutcome::Queued {
            name: "Nightly e2e".to_string(),
            position: None,
        }
        .message();
        assert!(!msg.contains("position"), "{msg}");
        assert!(msg.contains("Queued an off-schedule run"), "{msg}");
    }
}

/// Does a trigger write reach the precondition check that reads it?
///
/// The tests above pin the pure checker against a config handed to it
/// directly. These pin the step before that one: the config the checker will
/// read has to already carry the write that just returned. It did not, and the
/// gap was a real off-schedule fire of a trigger the user had just paused (the
/// intermittent `run_refuses_a_paused_trigger_instead_of_silently_dropping_it`).
///
/// **No scheduler subscriber runs here**, deliberately. That is what makes
/// these fail against the old code for the right reason instead of racing it:
/// under the subscriber-only design the registry is never written at all, so
/// the assertion is about the write path rather than about who wins.
#[cfg(test)]
mod read_your_writes_tests {
    use super::*;
    use crate::engine::event_bus::EventBus;
    use crate::engine::trigger_writes::{TriggerRegistryWriter, TriggerWrite};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::RwLock;

    type Registry = RwLock<HashMap<String, TriggerConfig>>;

    fn created_payload(id: &str) -> serde_json::Value {
        json!({
            "trigger_id": id,
            "name": "Nightly e2e",
            "slug": "nightly-e2e",
            "schedule": ["0 0 2 * * *"],
            "timezone": "UTC",
            "run": { "type": "intent", "intent": "run the nightly e2e" },
        })
    }

    /// What the run endpoint would answer for this id, right now.
    fn refusal(configs: &Registry, trigger_id: &str) -> Option<RunRefusal> {
        let config = configs.read().unwrap().get(trigger_id).cloned();
        check_off_schedule_run(trigger_id, config.as_ref(), None, None)
    }

    #[tokio::test]
    async fn a_pause_is_refused_by_the_very_next_run_request() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let ws = tempfile::tempdir().unwrap();
        let configs: Registry = RwLock::new(HashMap::new());
        let write_lock = tokio::sync::Mutex::new(());
        let writer = TriggerRegistryWriter {
            event_bus: &bus,
            trigger_configs: &configs,
            workspace_path: ws.path(),
            write_lock: &write_lock,
        };

        writer
            .write(TriggerWrite::Created, "t-1", created_payload("t-1"), None)
            .await
            .expect("create");
        // A live trigger runs off-schedule; that is the baseline the refusal
        // has to be measured against.
        assert_eq!(refusal(&configs, "t-1"), None);

        writer
            .write(
                TriggerWrite::Updated,
                "t-1",
                json!({ "trigger_id": "t-1", "paused": true }),
                None,
            )
            .await
            .expect("pause");

        assert_eq!(
            refusal(&configs, "t-1"),
            Some(RunRefusal::Paused {
                name: "Nightly e2e".to_string()
            }),
            "the pause returned, so the next run request must already see it"
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_delete_is_not_found_by_the_very_next_run_request() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let ws = tempfile::tempdir().unwrap();
        let configs: Registry = RwLock::new(HashMap::new());
        let write_lock = tokio::sync::Mutex::new(());
        let writer = TriggerRegistryWriter {
            event_bus: &bus,
            trigger_configs: &configs,
            workspace_path: ws.path(),
            write_lock: &write_lock,
        };

        writer
            .write(TriggerWrite::Created, "t-1", created_payload("t-1"), None)
            .await
            .expect("create");
        writer
            .write(
                TriggerWrite::Deleted,
                "t-1",
                json!({ "trigger_id": "t-1" }),
                None,
            )
            .await
            .expect("delete");

        assert_eq!(
            refusal(&configs, "t-1"),
            Some(RunRefusal::NotFound {
                trigger_id: "t-1".to_string()
            }),
            "a deleted trigger must not still be runnable"
        );
        teardown_test_db(&db).await;
    }

    /// The write is durable, not just fast. If the apply ever ran without the
    /// event landing, the registry would be ahead of the log and a restart
    /// would silently undo the user's change.
    #[tokio::test]
    async fn the_event_is_persisted_alongside_the_registry_apply() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let ws = tempfile::tempdir().unwrap();
        let configs: Registry = RwLock::new(HashMap::new());
        let write_lock = tokio::sync::Mutex::new(());

        TriggerRegistryWriter {
            event_bus: &bus,
            trigger_configs: &configs,
            workspace_path: ws.path(),
            write_lock: &write_lock,
        }
        .write(TriggerWrite::Created, "t-1", created_payload("t-1"), None)
        .await
        .expect("create");

        let persisted: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = 'TriggerCreated' AND aggregate_id = $1",
        )
        .bind("t-1")
        .fetch_one(&pool)
        .await
        .expect("count events");
        assert_eq!(
            persisted, 1,
            "the write must be in the log, not only in memory"
        );
        teardown_test_db(&db).await;
    }

    /// Emit first, then apply. A failed emit must apply nothing: a resume live
    /// in memory but absent from the log would come back paused after a
    /// restart, with nothing to explain it.
    #[tokio::test]
    async fn a_failed_emit_leaves_the_registry_untouched() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let ws = tempfile::tempdir().unwrap();
        let configs: Registry = RwLock::new(HashMap::new());
        let write_lock = tokio::sync::Mutex::new(());
        let writer = TriggerRegistryWriter {
            event_bus: &bus,
            trigger_configs: &configs,
            workspace_path: ws.path(),
            write_lock: &write_lock,
        };

        writer
            .write(TriggerWrite::Created, "t-1", created_payload("t-1"), None)
            .await
            .expect("create");

        // Closing the pool is the cheapest real emit failure: the INSERT can no
        // longer be issued, so the emit returns Err before anything is written.
        pool.close().await;
        let result = writer
            .write(
                TriggerWrite::Updated,
                "t-1",
                json!({ "trigger_id": "t-1", "paused": true }),
                None,
            )
            .await;

        assert!(result.is_err(), "a closed pool must fail the emit");
        assert!(
            !configs.read().unwrap()["t-1"].paused,
            "a pause that was never persisted must not be live in memory"
        );
        teardown_test_db(&db).await;
    }
}
