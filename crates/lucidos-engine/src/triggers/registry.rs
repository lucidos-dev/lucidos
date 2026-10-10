//! The **trigger registry**: the in-memory map of live [`TriggerConfig`]s, plus
//! its derived on-disk `trigger.toml` read-model (ADR 0019).
//!
//! This module owns the one place a trigger lifecycle event *becomes* trigger
//! state. Both halves live here together on purpose: a slug rename can only
//! delete the stale `trigger.toml` if the applier still holds the pre-image
//! slug, so splitting the map from the disk projection would leak an orphan
//! file on every rename.
//!
//! **Exactly one caller applies a live event**:
//! [`crate::engine::trigger_writes::emit_trigger_write`], the write chokepoint,
//! right after the emit commits and before the writing caller regains control.
//! That is what makes the trigger surface read-your-writes: a `PUT` that
//! answers `paused: true` is followed by a run request that sees a paused
//! trigger. The only other applier is startup replay
//! ([`super::replay::replay_trigger_events`]), which rebuilds the whole map
//! from the log before the subscriber exists.
//!
//! The scheduler's EventBus subscriber deliberately does **not** apply. Having
//! it re-apply as a redundant safety net reads as harmless and is not: a
//! `TriggerCreated` re-apply rebuilds the config from its original payload, so
//! a create followed straight away by a pause would be transiently un-paused
//! when the subscriber reached the older event. That is the same class of stale
//! read this module exists to close, reintroduced from the other side. The
//! subscriber reads the registry instead, and only arms or disarms cron jobs.
//!
//! One applier also makes ordering tractable: `LucidosEngine::trigger_write_lock`
//! serializes each emit with its own apply, so the map is written in the order
//! the log records.

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use serde_json::Value;

use super::config::TriggerConfig;
use super::definition;

/// The five events that mutate trigger state. `TriggerExecuted` and
/// `TriggerStarted` are run-history stamps written by the runner that just ran,
/// not lifecycle changes, so they are deliberately absent.
///
/// Read by the scheduler subscriber, to skip the trigger events it does no work
/// for, and by the tests that hold the writer side and this one in sync (the
/// `TriggerWrite` cross-check and the chokepoint's source-scan tripwire).
pub(crate) const TRIGGER_LIFECYCLE_EVENTS: [&str; 5] = [
    "TriggerCreated",
    "TriggerUpdated",
    "TriggerDeleted",
    "TriggerEnabled",
    "TriggerDisabled",
];

/// What an applied lifecycle event did to the registry. Callers use it to drive
/// their own side effects without re-reading the map (and without needing the
/// pre-image, which only the apply holds).
#[derive(Debug, Clone)]
pub(crate) enum TriggerRegistryChange {
    /// The trigger is present with this config. `retired_slug` is `Some` only
    /// when the apply changed the slug, and carries the one that is now gone.
    Upserted {
        config: Box<TriggerConfig>,
        retired_slug: Option<String>,
    },
    /// The trigger was removed; the config is its final state.
    Removed { config: Box<TriggerConfig> },
    /// Nothing changed: an update/delete for an id the registry does not hold,
    /// a create whose payload would not parse, or a non-lifecycle event.
    None,
}

/// Apply one trigger lifecycle event to the registry: mutate the in-memory map
/// and re-project the trigger's `trigger.toml`.
///
/// Idempotent. Best-effort on the disk half (a write failure is logged inside
/// [`definition`], never propagated) because the file is a derived read-model
/// and the authoritative state is the event log.
pub(crate) fn materialize_trigger_event(
    configs: &RwLock<HashMap<String, TriggerConfig>>,
    workspace_path: &Path,
    event_type: &str,
    trigger_id: &str,
    payload: &Value,
) -> TriggerRegistryChange {
    let change = apply_to_map(configs, event_type, trigger_id, payload);
    // Pause and resume are the exception: `TriggerDefinition` deliberately
    // excludes `paused` as runtime state, so flipping it cannot change the
    // file, and rewriting identical bytes would only churn its mtime.
    if !matches!(event_type, "TriggerEnabled" | "TriggerDisabled") {
        project_to_disk(workspace_path, &change);
    }
    change
}

/// The in-memory half. Split out so the lock is released before any file I/O.
fn apply_to_map(
    configs: &RwLock<HashMap<String, TriggerConfig>>,
    event_type: &str,
    trigger_id: &str,
    payload: &Value,
) -> TriggerRegistryChange {
    let mut configs = configs.write().unwrap();
    match event_type {
        "TriggerCreated" => match TriggerConfig::from_created_payload(payload) {
            Ok(config) => {
                configs.insert(trigger_id.to_string(), config.clone());
                TriggerRegistryChange::Upserted {
                    config: Box::new(config),
                    retired_slug: None,
                }
            }
            Err(e) => {
                crate::log!(
                    "[TriggerRegistry] TriggerCreated payload for {} did not parse: {}",
                    trigger_id,
                    e
                );
                TriggerRegistryChange::None
            }
        },
        "TriggerUpdated" => match configs.get_mut(trigger_id) {
            Some(config) => {
                let old_slug = config.slug.clone();
                config.apply_update(payload);
                let retired_slug = (old_slug != config.slug).then_some(old_slug);
                TriggerRegistryChange::Upserted {
                    config: Box::new(config.clone()),
                    retired_slug,
                }
            }
            None => TriggerRegistryChange::None,
        },
        "TriggerEnabled" | "TriggerDisabled" => match configs.get_mut(trigger_id) {
            Some(config) => {
                config.paused = event_type == "TriggerDisabled";
                TriggerRegistryChange::Upserted {
                    config: Box::new(config.clone()),
                    retired_slug: None,
                }
            }
            None => TriggerRegistryChange::None,
        },
        "TriggerDeleted" => match configs.remove(trigger_id) {
            Some(config) => TriggerRegistryChange::Removed {
                config: Box::new(config),
            },
            None => TriggerRegistryChange::None,
        },
        _ => TriggerRegistryChange::None,
    }
}

/// The on-disk half: keep `data/triggers/<slug>/trigger.toml` in step with the
/// map. A rename drops the file under the retired slug first, so exactly one
/// definition survives.
fn project_to_disk(workspace_path: &Path, change: &TriggerRegistryChange) {
    match change {
        TriggerRegistryChange::Upserted {
            config,
            retired_slug,
        } => {
            if let Some(old) = retired_slug {
                definition::remove_trigger_definition(workspace_path, old);
            }
            definition::write_trigger_definition(workspace_path, config);
        }
        TriggerRegistryChange::Removed { config } => {
            definition::remove_trigger_definition(workspace_path, &config.slug);
        }
        TriggerRegistryChange::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triggers::TriggerRun;
    use serde_json::json;

    fn registry() -> RwLock<HashMap<String, TriggerConfig>> {
        RwLock::new(HashMap::new())
    }

    fn created(id: &str, slug: &str) -> Value {
        json!({
            "trigger_id": id,
            "name": "Nightly e2e",
            "slug": slug,
            "schedule": ["0 0 2 * * *"],
            "timezone": "UTC",
            "run": { "type": "intent", "intent": "run the nightly e2e" },
        })
    }

    /// Materialize against a throwaway workspace dir, so the disk half is
    /// exercised rather than stubbed.
    fn apply(
        configs: &RwLock<HashMap<String, TriggerConfig>>,
        ws: &Path,
        event_type: &str,
        id: &str,
        payload: &Value,
    ) -> TriggerRegistryChange {
        materialize_trigger_event(configs, ws, event_type, id, payload)
    }

    fn toml_path(ws: &Path, slug: &str) -> std::path::PathBuf {
        ws.join("data/triggers").join(slug).join("trigger.toml")
    }

    #[test]
    fn create_inserts_and_writes_the_definition() {
        let ws = tempfile::tempdir().unwrap();
        let configs = registry();
        let change = apply(
            &configs,
            ws.path(),
            "TriggerCreated",
            "t-1",
            &created("t-1", "nightly-e2e"),
        );
        assert!(matches!(
            change,
            TriggerRegistryChange::Upserted { ref config, retired_slug: None }
                if config.slug == "nightly-e2e"
        ));
        assert!(configs.read().unwrap().contains_key("t-1"));
        assert!(toml_path(ws.path(), "nightly-e2e").exists());
    }

    #[test]
    fn pause_is_visible_in_the_map_immediately() {
        // The whole point of the module: an apply is synchronous, so the very
        // next read sees it. No subscriber, no broadcast, no sleep.
        let ws = tempfile::tempdir().unwrap();
        let configs = registry();
        apply(
            &configs,
            ws.path(),
            "TriggerCreated",
            "t-1",
            &created("t-1", "nightly-e2e"),
        );
        assert!(!configs.read().unwrap()["t-1"].paused);

        apply(
            &configs,
            ws.path(),
            "TriggerUpdated",
            "t-1",
            &json!({ "trigger_id": "t-1", "paused": true }),
        );
        assert!(configs.read().unwrap()["t-1"].paused);
    }

    #[test]
    fn enabled_and_disabled_flip_paused() {
        let ws = tempfile::tempdir().unwrap();
        let configs = registry();
        apply(
            &configs,
            ws.path(),
            "TriggerCreated",
            "t-1",
            &created("t-1", "nightly-e2e"),
        );
        apply(
            &configs,
            ws.path(),
            "TriggerDisabled",
            "t-1",
            &json!({ "trigger_id": "t-1" }),
        );
        assert!(configs.read().unwrap()["t-1"].paused);
        apply(
            &configs,
            ws.path(),
            "TriggerEnabled",
            "t-1",
            &json!({ "trigger_id": "t-1" }),
        );
        assert!(!configs.read().unwrap()["t-1"].paused);
    }

    #[test]
    fn delete_removes_the_entry_and_its_definition() {
        let ws = tempfile::tempdir().unwrap();
        let configs = registry();
        apply(
            &configs,
            ws.path(),
            "TriggerCreated",
            "t-1",
            &created("t-1", "nightly-e2e"),
        );
        let change = apply(
            &configs,
            ws.path(),
            "TriggerDeleted",
            "t-1",
            &json!({ "trigger_id": "t-1" }),
        );
        assert!(matches!(change, TriggerRegistryChange::Removed { .. }));
        assert!(configs.read().unwrap().is_empty());
        assert!(!toml_path(ws.path(), "nightly-e2e").exists());
    }

    #[test]
    fn a_slug_rename_leaves_exactly_one_definition() {
        let ws = tempfile::tempdir().unwrap();
        let configs = registry();
        apply(
            &configs,
            ws.path(),
            "TriggerCreated",
            "t-1",
            &created("t-1", "old-slug"),
        );
        // A sibling under the old slug proves the prune only takes the empty dir.
        let sibling = ws.path().join("data/triggers/old-slug/knowhow/notes.md");
        std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        std::fs::write(&sibling, "keep me").unwrap();

        let change = apply(
            &configs,
            ws.path(),
            "TriggerUpdated",
            "t-1",
            &json!({ "trigger_id": "t-1", "slug": "new-slug" }),
        );
        assert!(matches!(
            change,
            TriggerRegistryChange::Upserted {
                retired_slug: Some(ref s),
                ..
            } if s == "old-slug"
        ));
        assert!(!toml_path(ws.path(), "old-slug").exists());
        assert!(toml_path(ws.path(), "new-slug").exists());
        assert!(sibling.exists(), "sibling knowhow must survive the rename");
    }

    /// Only the chokepoint applies a live event, so a double apply should not
    /// happen. Pinned anyway: startup replay and a chokepoint apply can both
    /// touch the same event across a restart, and idempotence is what makes
    /// that a non-event rather than a duplicated subscription list.
    #[test]
    fn applying_the_same_event_twice_is_a_no_op() {
        let ws = tempfile::tempdir().unwrap();
        let configs = registry();
        let create = created("t-1", "nightly-e2e");
        let update = json!({
            "trigger_id": "t-1",
            "paused": true,
            "on": [{ "event_type": "EmailReceived" }],
        });

        apply(&configs, ws.path(), "TriggerCreated", "t-1", &create);
        apply(&configs, ws.path(), "TriggerUpdated", "t-1", &update);
        let once = configs.read().unwrap()["t-1"].clone();

        apply(&configs, ws.path(), "TriggerCreated", "t-1", &create);
        apply(&configs, ws.path(), "TriggerUpdated", "t-1", &update);
        apply(&configs, ws.path(), "TriggerUpdated", "t-1", &update);
        let twice = configs.read().unwrap()["t-1"].clone();

        assert_eq!(once.paused, twice.paused);
        assert_eq!(
            once.on.len(),
            twice.on.len(),
            "subscriptions must not duplicate"
        );
        assert_eq!(once.slug, twice.slug);
        assert_eq!(configs.read().unwrap().len(), 1);
    }

    #[test]
    fn a_repeated_delete_does_not_resurrect_or_error() {
        let ws = tempfile::tempdir().unwrap();
        let configs = registry();
        apply(
            &configs,
            ws.path(),
            "TriggerCreated",
            "t-1",
            &created("t-1", "nightly-e2e"),
        );
        let del = json!({ "trigger_id": "t-1" });
        apply(&configs, ws.path(), "TriggerDeleted", "t-1", &del);
        let second = apply(&configs, ws.path(), "TriggerDeleted", "t-1", &del);
        assert!(matches!(second, TriggerRegistryChange::None));
        assert!(configs.read().unwrap().is_empty());
    }

    #[test]
    fn an_update_for_an_unknown_id_changes_nothing() {
        let ws = tempfile::tempdir().unwrap();
        let configs = registry();
        let change = apply(
            &configs,
            ws.path(),
            "TriggerUpdated",
            "ghost",
            &json!({ "trigger_id": "ghost", "paused": true }),
        );
        assert!(matches!(change, TriggerRegistryChange::None));
        assert!(configs.read().unwrap().is_empty());
    }

    /// Applying is last-write-wins per field, so a sequence of updates ends on
    /// the last one applied. That is the property the chokepoint's write lock
    /// relies on: serialize the applies into log order and the registry lands
    /// where the log does. This pins the merge half only; the serialization
    /// half is the lock's, and cannot be shown by calling `apply` in a chosen
    /// order (which is all this test can do).
    #[test]
    fn the_last_update_applied_is_the_one_that_sticks() {
        let ws = tempfile::tempdir().unwrap();
        let configs = registry();
        apply(
            &configs,
            ws.path(),
            "TriggerCreated",
            "t-1",
            &created("t-1", "nightly-e2e"),
        );
        let first = json!({ "trigger_id": "t-1", "name": "First" });
        let second = json!({ "trigger_id": "t-1", "name": "Second" });

        apply(&configs, ws.path(), "TriggerUpdated", "t-1", &first);
        apply(&configs, ws.path(), "TriggerUpdated", "t-1", &second);
        assert_eq!(configs.read().unwrap()["t-1"].name, "Second");

        // And the other way round, so this pins the order dependence rather
        // than a coincidence about which payload happens to win.
        apply(&configs, ws.path(), "TriggerUpdated", "t-1", &first);
        assert_eq!(configs.read().unwrap()["t-1"].name, "First");
    }

    #[test]
    fn a_non_lifecycle_event_is_ignored() {
        let ws = tempfile::tempdir().unwrap();
        let configs = registry();
        apply(
            &configs,
            ws.path(),
            "TriggerCreated",
            "t-1",
            &created("t-1", "nightly-e2e"),
        );
        let change = apply(
            &configs,
            ws.path(),
            "TriggerExecuted",
            "t-1",
            &json!({ "trigger_id": "t-1", "status": "ok" }),
        );
        assert!(matches!(change, TriggerRegistryChange::None));
        assert!(matches!(
            configs.read().unwrap()["t-1"].run,
            TriggerRun::Intent { .. }
        ));
    }

    /// Every name in the enumeration must reach an arm that does work, or the
    /// list has drifted from the dispatch it is supposed to describe.
    #[test]
    fn every_enumerated_event_reaches_an_arm_that_does_work() {
        for event_type in TRIGGER_LIFECYCLE_EVENTS {
            let ws = tempfile::tempdir().unwrap();
            let configs = registry();
            apply(
                &configs,
                ws.path(),
                "TriggerCreated",
                "t-1",
                &created("t-1", "nightly-e2e"),
            );
            // A create needs its full payload; every other arm reads the
            // registry entry the create above put there.
            let payload = if event_type == "TriggerCreated" {
                created("t-1", "renamed-e2e")
            } else {
                json!({ "trigger_id": "t-1" })
            };
            let change = apply(&configs, ws.path(), event_type, "t-1", &payload);
            assert!(
                !matches!(change, TriggerRegistryChange::None),
                "{event_type} is enumerated as a lifecycle event but changes nothing"
            );
        }
    }
}
