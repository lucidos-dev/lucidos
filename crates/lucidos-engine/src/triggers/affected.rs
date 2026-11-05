//! **Affected triggers**: triggers a plugin does not own that rely on a path
//! the plugin's update or uninstall removes. The match is heuristic, so the
//! result only ever warns (ADR 0131). Nothing here edits, pauses or deletes a
//! trigger (ADR 0019).
//!
//! Plan: `docs/plans/2026-10-21-warn-triggers-on-plugin-knowhow-removal.md`.

use super::config::{TriggerConfig, TriggerRun};
use serde::{Deserialize, Serialize};

/// A trigger, and the removed paths it relies on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedTrigger {
    pub trigger_id: String,
    pub trigger_name: String,
    pub paths: Vec<AffectedPath>,
}

/// One removed `data/`-relative path, and why the trigger relies on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedPath {
    pub path: String,
    pub reason: AffectedReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AffectedReason {
    /// The intent text or the script path names the path.
    NamedInRun,
    /// The path is knowhow under the trigger's owning app.
    OwningApp,
    /// The path is knowhow under the trigger's own `triggers/<slug>/` folder.
    TriggerKnowhow,
}

/// Every trigger that relies on one of `removed`, sorted by name. A trigger
/// owned by `plugin_id` is skipped: that plugin ships its intent and knowhow
/// together, and its own re-sync or delete handles it.
pub fn affected_triggers<'a>(
    removed: &[String],
    triggers: impl IntoIterator<Item = &'a TriggerConfig>,
    plugin_id: &str,
) -> Vec<AffectedTrigger> {
    let mut affected: Vec<AffectedTrigger> = triggers
        .into_iter()
        .filter(|t| t.plugin_id.as_deref() != Some(plugin_id))
        .filter_map(|trigger| {
            let paths: Vec<AffectedPath> = removed
                .iter()
                .filter_map(|path| {
                    reason_for(trigger, path).map(|reason| AffectedPath {
                        path: path.clone(),
                        reason,
                    })
                })
                .collect();
            (!paths.is_empty()).then(|| AffectedTrigger {
                trigger_id: trigger.id.clone(),
                trigger_name: trigger.name.clone(),
                paths,
            })
        })
        .collect();
    affected.sort_by(|a, b| {
        (a.trigger_name.as_str(), a.trigger_id.as_str())
            .cmp(&(b.trigger_name.as_str(), b.trigger_id.as_str()))
    });
    affected
}

fn reason_for(trigger: &TriggerConfig, path: &str) -> Option<AffectedReason> {
    let run_text = match &trigger.run {
        TriggerRun::Intent { intent } => intent.as_str(),
        TriggerRun::Script { path } => path.as_str(),
    };
    if names_path(run_text, path) {
        return Some(AffectedReason::NamedInRun);
    }
    if trigger
        .owning_app_id()
        .is_some_and(|app| is_app_knowhow(path, &app))
    {
        return Some(AffectedReason::OwningApp);
    }
    if is_trigger_knowhow(path, &trigger.slug) {
        return Some(AffectedReason::TriggerKnowhow);
    }
    None
}

/// The forms a run can name `path` in: itself, under `data/`, without `.md`,
/// and its `load_knowhow` id when that id has a slash. A one-word id is too
/// loose to count on its own.
fn path_forms(path: &str) -> Vec<String> {
    let mut forms = vec![path.to_string(), format!("data/{path}")];
    if let Some(stem) = path.strip_suffix(".md") {
        forms.push(stem.to_string());
        if let Some(id) = knowhow_id(stem).filter(|id| id.contains('/')) {
            forms.push(id);
        }
    }
    forms
}

/// The `load_knowhow` id of a knowhow file, given its path without `.md`.
fn knowhow_id(stem: &str) -> Option<String> {
    let segments: Vec<&str> = stem.split('/').collect();
    match segments.as_slice() {
        ["knowhow", rest @ ..] if !rest.is_empty() => Some(rest.join("/")),
        ["apps", app, "knowhow", rest @ ..] if !rest.is_empty() => {
            Some(format!("{app}/{}", rest.join("/")))
        }
        ["triggers", slug, "knowhow", rest @ ..] if !rest.is_empty() => {
            Some(format!("triggers/{slug}/{}", rest.join("/")))
        }
        _ => None,
    }
}

fn names_path(text: &str, path: &str) -> bool {
    path_forms(path)
        .iter()
        .any(|form| contains_token(text, form))
}

/// True when `needle` occurs in `text` with a path boundary on both sides, so
/// `x.md` never matches inside `x-v2.md` or `old/x.md`.
fn contains_token(text: &str, needle: &str) -> bool {
    let is_path_char = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '/');
    text.match_indices(needle).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let mut after = text[start + needle.len()..].chars();
        let open = before.is_none_or(|c| !is_path_char(c) && c != '.');
        let close = match after.next() {
            None => true,
            // A sentence-ending full stop closes, an extension does not.
            Some('.') => after.next().is_none_or(|c| !c.is_alphanumeric()),
            Some(c) => !is_path_char(c),
        };
        open && close
    })
}

/// True when `path` sits in a `knowhow/` folder under `apps/<app>/`.
fn is_app_knowhow(path: &str, app: &str) -> bool {
    let Some(rest) = path
        .strip_prefix("apps/")
        .and_then(|p| p.strip_prefix(app))
        .and_then(|p| p.strip_prefix('/'))
    else {
        return false;
    };
    let mut folders: Vec<&str> = rest.split('/').collect();
    folders.pop();
    folders.contains(&"knowhow")
}

fn is_trigger_knowhow(path: &str, slug: &str) -> bool {
    !slug.is_empty() && path.starts_with(&format!("triggers/{slug}/knowhow/"))
}

/// A removed path a trigger still relies on when it fires, and what removed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedKnowhow {
    pub path: String,
    /// "the Token Cost update to v0.6.0", or "uninstalling token-cost".
    pub removed_by: String,
}

/// The removed paths `trigger_id` relies on at fire time. A path counts when a
/// plugin event newer than the trigger's last edit lists it, and it is still
/// missing under `data_dir`. Derived from the events table and the disk only,
/// so it survives a restart.
pub async fn removed_knowhow(
    pool: &sqlx::PgPool,
    data_dir: &std::path::Path,
    trigger_id: &str,
) -> Result<Vec<RemovedKnowhow>, sqlx::Error> {
    // Two `TriggerUpdated` shapes do not count as an edit, since neither means
    // anyone looked at the know-how. Pause and resume carry only `paused`. A
    // plugin resync re-registers the trigger and carries `plugin_id`.
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, payload->'data' FROM events \
         WHERE event_type IN ('PluginInstalled', 'PluginUninstalled') \
           AND payload->'data'->'affected_triggers' \
               @> jsonb_build_array(jsonb_build_object('trigger_id', $1::text)) \
           AND sequence > COALESCE((SELECT max(u.sequence) FROM events u \
               WHERE u.event_type = 'TriggerUpdated' AND u.aggregate_id = $1 \
                 AND u.payload->'plugin_id' IS NULL \
                 AND EXISTS (SELECT 1 FROM jsonb_object_keys(u.payload) k \
                     WHERE k NOT IN ('trigger_id', 'paused', 'actor'))), 0) \
         ORDER BY sequence",
    )
    .bind(trigger_id)
    .fetch_all(pool)
    .await?;

    let mut removed = std::collections::BTreeMap::new();
    for (event_type, data) in rows {
        let removed_by = removal_label(&event_type, &data);
        let affected: Vec<AffectedTrigger> = data
            .get("affected_triggers")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let paths = affected
            .into_iter()
            .filter(|a| a.trigger_id == trigger_id)
            .flat_map(|a| a.paths);
        for AffectedPath { path, .. } in paths {
            removed.insert(path, removed_by.clone());
        }
    }
    Ok(removed
        .into_iter()
        .filter(|(path, _)| !data_dir.join(path).exists())
        .map(|(path, removed_by)| RemovedKnowhow { path, removed_by })
        .collect())
}

fn removal_label(event_type: &str, data: &serde_json::Value) -> String {
    let text = |pointer: &str| data.pointer(pointer).and_then(|v| v.as_str());
    if event_type == "PluginUninstalled" {
        return format!("uninstalling {}", text("/id").unwrap_or("a plugin"));
    }
    let name = text("/manifest/manifest/name")
        .or_else(|| text("/id"))
        .unwrap_or("a plugin");
    match text("/manifest/manifest/version") {
        Some(version) => format!("the {name} update to v{version}"),
        None => format!("the {name} update"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real Token Cost trigger's intent, which names no path.
    const TOKEN_COST_INTENT: &str = "Check the official model prices against my Token Cost \
        price table and tell me about any price that changed.";
    const DAILY: &str = "apps/token-cost/knowhow/daily-price-check.md";

    fn intent_trigger(id: &str, intent: &str) -> TriggerConfig {
        trigger(
            id,
            TriggerRun::Intent {
                intent: intent.to_string(),
            },
        )
    }

    fn trigger(id: &str, run: TriggerRun) -> TriggerConfig {
        TriggerConfig {
            id: id.to_string(),
            name: format!("Trigger {id}"),
            slug: format!("trigger-{id}"),
            schedule: vec!["0 0 8 * * *".to_string()],
            timezone: "UTC".to_string(),
            run,
            on: vec![],
            paused: false,
            last_run: None,
            last_run_status: None,
            app_id: None,
            go_to_review: false,
            group_id: None,
            side_effect_grant: vec![],
            mcp_servers: vec![],
            plugin_id: None,
            model: None,
            reasoning_effort: None,
            provider: None,
        }
    }

    fn removed(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    fn reasons(removed_paths: &[&str], t: &TriggerConfig) -> Vec<(String, AffectedReason)> {
        affected_triggers(&removed(removed_paths), [t], "token-cost")
            .into_iter()
            .flat_map(|a| a.paths)
            .map(|p| (p.path, p.reason))
            .collect()
    }

    #[test]
    fn the_token_cost_shape_is_detected_by_app_ownership() {
        let mut t = intent_trigger("a", TOKEN_COST_INTENT);
        t.app_id = Some("token-cost".to_string());
        let got = affected_triggers(&removed(&[DAILY]), [&t], "token-cost-plugin");
        assert_eq!(
            got,
            vec![AffectedTrigger {
                trigger_id: "a".to_string(),
                trigger_name: "Trigger a".to_string(),
                paths: vec![AffectedPath {
                    path: DAILY.to_string(),
                    reason: AffectedReason::OwningApp,
                }],
            }]
        );
    }

    #[test]
    fn every_path_form_in_the_intent_is_a_match() {
        for intent in [
            "Follow apps/token-cost/knowhow/daily-price-check.md each morning.",
            "Follow data/apps/token-cost/knowhow/daily-price-check.md.",
            "Follow `apps/token-cost/knowhow/daily-price-check`.",
            "Load token-cost/daily-price-check and run it.",
        ] {
            let t = intent_trigger("a", intent);
            assert_eq!(
                reasons(&[DAILY], &t),
                vec![(DAILY.to_string(), AffectedReason::NamedInRun)],
                "{intent}"
            );
        }
    }

    #[test]
    fn a_trigger_scoped_and_a_grouped_id_are_matches() {
        let t = intent_trigger("a", "Use triggers/other/steps for the run.");
        assert_eq!(reasons(&["triggers/other/knowhow/steps.md"], &t).len(), 1);
        let t = intent_trigger("a", "Use weather/api to fetch it.");
        assert_eq!(reasons(&["knowhow/weather/api.md"], &t).len(), 1);
    }

    #[test]
    fn a_path_never_matches_inside_a_longer_one() {
        for intent in [
            "Follow apps/token-cost/knowhow/daily-price-check-v2.md.",
            "Follow apps/token-cost/knowhow/daily-price-check.md.bak.",
            "Follow old/apps/token-cost/knowhow/daily-price-check.md.",
            "Load token-cost/daily-price-check-v2.",
        ] {
            let t = intent_trigger("a", intent);
            assert!(reasons(&[DAILY], &t).is_empty(), "{intent}");
        }
    }

    #[test]
    fn a_bare_top_level_id_is_not_a_match() {
        let t = intent_trigger("a", "Do the weather check.");
        assert!(reasons(&["knowhow/weather.md"], &t).is_empty());
    }

    #[test]
    fn an_app_asset_is_not_a_match_by_ownership() {
        let mut t = intent_trigger("a", TOKEN_COST_INTENT);
        t.app_id = Some("token-cost".to_string());
        assert!(reasons(&["apps/token-cost/assets/icon.svg"], &t).is_empty());
        assert!(reasons(&["apps/token-cost/knowhow.md"], &t).is_empty());
        assert!(reasons(&["apps/token-cost-v2/knowhow/x.md"], &t).is_empty());
    }

    #[test]
    fn the_trigger_s_own_knowhow_is_a_match() {
        let t = intent_trigger("a", TOKEN_COST_INTENT);
        assert_eq!(
            reasons(&["triggers/trigger-a/knowhow/steps.md"], &t),
            vec![(
                "triggers/trigger-a/knowhow/steps.md".to_string(),
                AffectedReason::TriggerKnowhow
            )]
        );
        assert!(reasons(&["triggers/trigger-ab/knowhow/steps.md"], &t).is_empty());
    }

    #[test]
    fn a_script_trigger_is_matched_by_path_and_by_its_app() {
        let t = trigger(
            "a",
            TriggerRun::Script {
                path: "apps/habit-tracker/scripts/check.py".to_string(),
            },
        );
        let got = reasons(
            &[
                "apps/habit-tracker/scripts/check.py",
                "apps/habit-tracker/knowhow/check.md",
            ],
            &t,
        );
        assert_eq!(
            got,
            vec![
                (
                    "apps/habit-tracker/scripts/check.py".to_string(),
                    AffectedReason::NamedInRun
                ),
                (
                    "apps/habit-tracker/knowhow/check.md".to_string(),
                    AffectedReason::OwningApp
                ),
            ]
        );
    }

    #[test]
    fn the_acting_plugin_s_trigger_is_skipped_and_another_plugin_s_is_not() {
        let mut own = intent_trigger("own", TOKEN_COST_INTENT);
        own.app_id = Some("token-cost".to_string());
        own.plugin_id = Some("token-cost".to_string());
        let mut other = own.clone();
        other.id = "other".to_string();
        other.plugin_id = Some("price-alerts".to_string());
        let got = affected_triggers(&removed(&[DAILY]), [&own, &other], "token-cost");
        let ids: Vec<&str> = got.iter().map(|a| a.trigger_id.as_str()).collect();
        assert_eq!(ids, vec!["other"]);
    }

    #[test]
    fn an_unrelated_trigger_is_not_reported() {
        let mut t = intent_trigger("a", TOKEN_COST_INTENT);
        t.app_id = Some("habit-tracker".to_string());
        assert!(reasons(&[DAILY], &t).is_empty());
    }

    #[test]
    fn the_reason_serializes_kebab_case() {
        let json = serde_json::to_value(AffectedReason::TriggerKnowhow).unwrap();
        assert_eq!(json, "trigger-knowhow");
    }

    mod removed_at_fire {
        use super::super::*;
        use super::DAILY;
        use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
        use crate::test_support::{setup_test_db, teardown_test_db};
        use serde_json::json;

        async fn emit(bus: &EventBus, event: SystemEvent) {
            bus.emit(BusEvent::System(event)).await.expect("emit");
        }

        fn update_removing_daily(trigger_id: &str) -> SystemEvent {
            SystemEvent::PluginInstalled {
                id: "token-cost".to_string(),
                manifest: json!({
                    "manifest": { "id": "token-cost", "name": "Token Cost", "version": "0.6.0" }
                }),
                files: vec![],
                files_removed: vec![DAILY.to_string()],
                affected_triggers: vec![AffectedTrigger {
                    trigger_id: trigger_id.to_string(),
                    trigger_name: "Daily price check".to_string(),
                    paths: vec![AffectedPath {
                        path: DAILY.to_string(),
                        reason: AffectedReason::OwningApp,
                    }],
                }],
                installed_at: "now".to_string(),
                source_type: "archive".to_string(),
                actor: None,
            }
        }

        fn trigger_updated(trigger_id: &str, payload: serde_json::Value) -> SystemEvent {
            SystemEvent::TriggerUpdated {
                trigger_id: trigger_id.to_string(),
                payload,
                actor: None,
            }
        }

        fn listed_daily() -> Vec<RemovedKnowhow> {
            vec![RemovedKnowhow {
                path: DAILY.to_string(),
                removed_by: "the Token Cost update to v0.6.0".to_string(),
            }]
        }

        #[tokio::test]
        async fn a_removal_is_listed_until_the_trigger_is_edited() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let data = tempfile::tempdir().unwrap();
            emit(
                &bus,
                trigger_updated("t1", json!({ "trigger_id": "t1", "name": "A" })),
            )
            .await;
            emit(&bus, update_removing_daily("t1")).await;

            let got = removed_knowhow(&pool, data.path(), "t1").await.unwrap();
            assert_eq!(got, listed_daily());
            let other = removed_knowhow(&pool, data.path(), "t2").await.unwrap();
            assert!(other.is_empty(), "another trigger's removal is not listed");

            emit(
                &bus,
                trigger_updated("t1", json!({ "trigger_id": "t1", "name": "B" })),
            )
            .await;
            let got = removed_knowhow(&pool, data.path(), "t1").await.unwrap();
            assert!(got.is_empty(), "an edit after the removal ends the warning");
            teardown_test_db(&db_name).await;
        }

        #[tokio::test]
        async fn a_pause_or_resume_does_not_end_the_warning() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let data = tempfile::tempdir().unwrap();
            emit(&bus, update_removing_daily("t1")).await;
            for paused in [true, false] {
                let pause = json!({ "trigger_id": "t1", "paused": paused });
                emit(&bus, trigger_updated("t1", pause)).await;
            }

            let got = removed_knowhow(&pool, data.path(), "t1").await.unwrap();
            assert_eq!(got, listed_daily());
            teardown_test_db(&db_name).await;
        }

        #[tokio::test]
        async fn a_plugin_resync_does_not_end_the_warning() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let data = tempfile::tempdir().unwrap();
            emit(&bus, update_removing_daily("t1")).await;
            let resync = json!({ "trigger_id": "t1", "name": "A", "plugin_id": "price-alerts" });
            emit(&bus, trigger_updated("t1", resync)).await;

            let got = removed_knowhow(&pool, data.path(), "t1").await.unwrap();
            assert_eq!(got, listed_daily());
            teardown_test_db(&db_name).await;
        }

        #[tokio::test]
        async fn a_recreated_file_ends_the_warning() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let data = tempfile::tempdir().unwrap();
            emit(&bus, update_removing_daily("t1")).await;
            let recreated = data.path().join(DAILY);
            std::fs::create_dir_all(recreated.parent().unwrap()).unwrap();
            std::fs::write(&recreated, "back").unwrap();

            let got = removed_knowhow(&pool, data.path(), "t1").await.unwrap();
            assert!(got.is_empty(), "a path that exists again is not listed");
            teardown_test_db(&db_name).await;
        }

        #[tokio::test]
        async fn an_uninstall_names_the_plugin_it_removed() {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let data = tempfile::tempdir().unwrap();
            let SystemEvent::PluginInstalled {
                affected_triggers, ..
            } = update_removing_daily("t1")
            else {
                unreachable!()
            };
            let uninstall = SystemEvent::PluginUninstalled {
                id: "token-cost".to_string(),
                version: "0.6.0".to_string(),
                files: vec![DAILY.to_string()],
                files_deleted: vec![DAILY.to_string()],
                files_missing: vec![],
                affected_triggers,
                actor: None,
            };
            emit(&bus, uninstall).await;

            let got = removed_knowhow(&pool, data.path(), "t1").await.unwrap();
            let by: Vec<&str> = got.iter().map(|r| r.removed_by.as_str()).collect();
            assert_eq!(by, vec!["uninstalling token-cost"]);
            teardown_test_db(&db_name).await;
        }
    }
}
