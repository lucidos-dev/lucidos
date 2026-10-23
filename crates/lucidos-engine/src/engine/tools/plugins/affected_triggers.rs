//! Where a plugin update or uninstall reports its *affected triggers*: the
//! staged preview, and one notification after the confirm. The match itself
//! is [`crate::triggers::affected::affected_triggers`], the one function both
//! the staging and the confirm call (ADR 0494).
//!
//! Plan: `docs/plans/2026-10-21-warn-triggers-on-plugin-knowhow-removal.md`.

use super::{PLUGIN_INSTALL_REQUEST_PREFIX, PLUGIN_UNINSTALL_REQUEST_PREFIX};
use crate::engine::event_bus::{BusEvent, EventBusEmitter, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;
use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};
use crate::triggers::affected::{affected_triggers, AffectedTrigger};
use crate::triggers::TriggerConfig;

/// Every trigger the engine knows, cloned so no lock is held across a write.
pub(crate) fn snapshot_triggers(engine: &LucidosEngine) -> Vec<TriggerConfig> {
    engine
        .trigger_configs
        .read()
        .unwrap()
        .values()
        .cloned()
        .collect()
}

/// Add `affected_triggers` to a staged install or uninstall sentinel, matched
/// over the paths the confirm would delete. Any other string, an `Error:` line
/// included, passes through untouched.
pub(crate) fn annotate_staged(staged: String, triggers: &[TriggerConfig]) -> String {
    let (prefix, removed_key) = if staged.starts_with(PLUGIN_INSTALL_REQUEST_PREFIX) {
        (PLUGIN_INSTALL_REQUEST_PREFIX, "removes")
    } else if staged.starts_with(PLUGIN_UNINSTALL_REQUEST_PREFIX) {
        (PLUGIN_UNINSTALL_REQUEST_PREFIX, "files_present")
    } else {
        return staged;
    };
    let Ok(serde_json::Value::Object(mut preview)) =
        serde_json::from_str::<serde_json::Value>(&staged[prefix.len()..])
    else {
        return staged;
    };
    let removed: Vec<String> = preview
        .get(removed_key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let plugin_id = preview
        .get("plugin_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let affected = affected_triggers(&removed, triggers, plugin_id);
    preview.insert("affected_triggers".into(), serde_json::json!(affected));
    format!("{prefix}{}", serde_json::Value::Object(preview))
}

/// What removed the files: a version update, or the uninstall.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Removal<'a> {
    Update { version: &'a str },
    Uninstall,
}

/// The notification a confirm raises, `None` when no trigger is affected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AffectedNotice {
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) tap: Tap,
}

pub(crate) fn notice(
    plugin_name: &str,
    removal: Removal<'_>,
    affected: &[AffectedTrigger],
) -> Option<AffectedNotice> {
    let first = affected.first()?;
    let uses = match affected.len() {
        1 => "a trigger uses".to_string(),
        n => format!("{n} triggers use"),
    };
    let (title, remover) = match removal {
        Removal::Update { version } => (
            format!("{plugin_name} update removed know-how {uses}"),
            format!("v{version}"),
        ),
        Removal::Uninstall => (
            format!("Removing {plugin_name} removed know-how {uses}"),
            "the uninstall".to_string(),
        ),
    };
    let (message, target, id) = match affected {
        [only] => {
            let paths: Vec<&str> = only.paths.iter().map(|p| p.path.as_str()).collect();
            (
                format!(
                    "\"{}\" used {}, which {remover} removed. Edit the trigger so it follows \
                     current know-how.",
                    only.trigger_name,
                    join_and(&paths)
                ),
                NavigateTarget::Trigger,
                Some(first.trigger_id.clone()),
            )
        }
        many => {
            let names: Vec<String> = many
                .iter()
                .map(|t| format!("\"{}\"", t.trigger_name))
                .collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            (
                format!(
                    "{} used know-how that {remover} removed. Edit them so they follow current \
                     know-how.",
                    join_and(&names)
                ),
                NavigateTarget::Triggers,
                None,
            )
        }
    };
    Some(AffectedNotice {
        title,
        message,
        tap: Tap::Navigate {
            to: Box::new(NavigateUi {
                target,
                id,
                ..Default::default()
            }),
        },
    })
}

/// Raise the [`notice`] for a confirm, when there is one. Never sends a web
/// push: the user just confirmed in the panel. A failed emit is logged, since
/// the files are already removed by now.
pub(crate) async fn notify(
    bus: &dyn EventBusEmitter,
    plugin_name: &str,
    removal: Removal<'_>,
    affected: &[AffectedTrigger],
    actor: Option<MessageOrigin>,
) {
    let Some(notice) = notice(plugin_name, removal, affected) else {
        return;
    };
    let emitted = bus
        .emit(BusEvent::System(SystemEvent::NotificationCreated {
            id: uuid::Uuid::new_v4().to_string(),
            title: notice.title,
            message: notice.message,
            task_id: None,
            app_id: None,
            thread_id: None,
            event_id: None,
            tap: notice.tap,
            actor,
        }))
        .await;
    if let Err(e) = emitted {
        log!(@Plugins, "affected-trigger notification failed: {}", e);
    }
}

fn join_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triggers::affected::{AffectedPath, AffectedReason};

    fn affected(id: &str, name: &str, paths: &[&str]) -> AffectedTrigger {
        AffectedTrigger {
            trigger_id: id.to_string(),
            trigger_name: name.to_string(),
            paths: paths
                .iter()
                .map(|p| AffectedPath {
                    path: p.to_string(),
                    reason: AffectedReason::OwningApp,
                })
                .collect(),
        }
    }

    fn tap_target(tap: &Tap) -> (NavigateTarget, Option<String>) {
        match tap {
            Tap::Navigate { to } => (to.target, to.id.clone()),
            Tap::Modal => panic!("the notice must navigate"),
        }
    }

    #[test]
    fn one_trigger_names_its_path_and_opens_it() {
        let got = notice(
            "Token Cost",
            Removal::Update { version: "0.6.0" },
            &[affected(
                "t1",
                "Token Cost daily price check",
                &["apps/token-cost/knowhow/daily-price-check.md"],
            )],
        )
        .unwrap();
        assert_eq!(
            got.title,
            "Token Cost update removed know-how a trigger uses"
        );
        assert_eq!(
            got.message,
            "\"Token Cost daily price check\" used \
             apps/token-cost/knowhow/daily-price-check.md, which v0.6.0 removed. Edit the \
             trigger so it follows current know-how."
        );
        assert_eq!(
            tap_target(&got.tap),
            (NavigateTarget::Trigger, Some("t1".to_string()))
        );
    }

    #[test]
    fn several_triggers_open_the_trigger_list() {
        let got = notice(
            "Token Cost",
            Removal::Uninstall,
            &[
                affected("t1", "A", &["x.md"]),
                affected("t2", "B", &["y.md"]),
            ],
        )
        .unwrap();
        assert_eq!(
            got.title,
            "Removing Token Cost removed know-how 2 triggers use"
        );
        assert_eq!(
            got.message,
            "\"A\" and \"B\" used know-how that the uninstall removed. Edit them so they \
             follow current know-how."
        );
        assert_eq!(tap_target(&got.tap), (NavigateTarget::Triggers, None));
    }

    #[test]
    fn no_affected_trigger_raises_no_notice() {
        assert_eq!(notice("Token Cost", Removal::Uninstall, &[]), None);
    }

    #[test]
    fn an_error_line_passes_through_annotation() {
        let line = "Error: nope".to_string();
        assert_eq!(annotate_staged(line.clone(), &[]), line);
    }
}
