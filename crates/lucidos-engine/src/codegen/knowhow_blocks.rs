//! Cached fragments contain serialized wire values, never handwritten prose.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use crate::engine::thread_events::{self as events, ts_codegen};

macro_rules! samples {
    ($ty:ident; $($variant:ident $({$($field:ident: $value:expr),* $(,)?})?),* $(,)?) => {
        vec![$((stringify!($variant), events::$ty::$variant $({$($field: $value),*})?)),*]
    };
}

fn values<T: Serialize>(root: &Path, name: &str, samples: Vec<(&str, T)>) -> String {
    let declared = ts_codegen::enum_variant_names(root, name);
    let sampled: BTreeSet<_> = samples
        .iter()
        .map(|(variant, _)| variant.to_string())
        .collect();
    assert_eq!(declared, sampled, "{name} samples must cover every variant");
    assert_eq!(samples.len(), sampled.len(), "duplicate {name} sample");
    let wire: Vec<_> = samples
        .into_iter()
        .map(|(_, sample)| {
            let value = serde_json::to_value(sample).expect("enum sample serializes");
            let name = value
                .as_str()
                .or_else(|| value.get("kind").and_then(|kind| kind.as_str()))
                .expect("documented enum is a string or internally tagged with kind");
            format!("`{name}`")
        })
        .collect();
    assert_eq!(wire.iter().collect::<BTreeSet<_>>().len(), wire.len());
    format!("\n{name} wire values:\n\n- {}\n", wire.join("\n- "))
}

pub(super) fn render(root: &Path) -> String {
    let mut fragments = BTreeMap::new();
    macro_rules! block {
        ($ty:ident; $($sample:tt)*) => {
            fragments.insert(stringify!($ty), values(root, stringify!($ty), samples!($ty; $($sample)*)));
        };
    }
    block!(SessionEndReason; Shutdown, Panic, Closed, StaleResume, LegacyNonTerminal);
    block!(CancelCause; UserStop, UserAction, SupersededByFollowup, EffortRestart, Unknown);
    block!(AbortCause; EngineShutdown, SafetyNet, RecoveryAfterRestart, ProcessKilled, StaleSettle, SessionDropped, Unknown);
    block!(UnproposedReason; PlanMissing, PlanAwaitingApproval, OutsideBound, HardeningMissing,
        TurnIncomplete, AwaitingLanding, IntegratedIntoParent, CarriedBySuccessor);
    block!(EngineReason;
        ContinuationStarted, OrphanRecovery,
        Scheduler { trigger_id: String::new(), trigger_name: None },
        HardenRetrigger, StaleSession, ArchivedBranchWork, UnmergedIntegratedWork,
        SuccessorResidue, SuccessorSession, MergeConflict, MainCheckRed, MainCheckFlaky,
        MissingHardening, MissingPlan,
        PluginAutoUpdate { plugin_id: String::new(), marketplace_id: String::new(), marketplace_name: String::new() },
        PluginSetup { plugin_id: String::new(), plugin_name: String::new(), version: String::new(),
            occasion: events::PluginSetupOccasion::FreshInstall, confirmed_on_device_id: None },
        PluginUpstreamProposal { plugin_id: String::new(), plugin_name: String::new(), version: String::new(),
            patch_path: String::new(), confirmed_on_device_id: None },
        EventWait { outcome: events::EventWaitOutcome::Delivered, watched: Vec::new(), wait_reason: String::new() },
        CacheKeepAlive);
    block!(SessionContinuity; First, Resumed,
        Fresh { reason: events::FreshSessionReason::ChangeApplied },
        Handoff { checkpoint: None, progress: None, opening: events::HandoffOpening::Narrative, message: String::new() });
    block!(FreshSessionReason; ChangeApplied, ChangeDiscarded, SessionEnded, BranchGone, ResumeFailed, NoResumableSession);
    block!(HandoffOpening; Narrative, Brief);
    let manifest = serde_json::json!({
        "schema_version": 1,
        "documents": { "system-knowhow/thread-events.md": fragments },
    });
    serde_json::to_string_pretty(&manifest).expect("block fragments serialize") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_use_serde_and_cover_the_declared_enums() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest: serde_json::Value = serde_json::from_str(&render(&root)).unwrap();
        let fragments = manifest["documents"]["system-knowhow/thread-events.md"]
            .as_object()
            .unwrap();
        for (name, serialized) in [
            (
                "SessionEndReason",
                serde_json::to_value(events::SessionEndReason::StaleResume).unwrap(),
            ),
            (
                "CancelCause",
                serde_json::to_value(events::CancelCause::UserStop).unwrap(),
            ),
            (
                "AbortCause",
                serde_json::to_value(events::AbortCause::SessionDropped).unwrap(),
            ),
            (
                "UnproposedReason",
                serde_json::to_value(events::UnproposedReason::AwaitingLanding).unwrap(),
            ),
            (
                "EngineReason",
                serde_json::to_value(events::EngineReason::CacheKeepAlive).unwrap(),
            ),
            (
                "SessionContinuity",
                serde_json::to_value(events::SessionContinuity::First).unwrap(),
            ),
            (
                "FreshSessionReason",
                serde_json::to_value(events::FreshSessionReason::ResumeFailed).unwrap(),
            ),
            (
                "HandoffOpening",
                serde_json::to_value(events::HandoffOpening::Brief).unwrap(),
            ),
        ] {
            let wire = serialized
                .as_str()
                .or_else(|| serialized["kind"].as_str())
                .unwrap();
            assert!(fragments[name]
                .as_str()
                .unwrap()
                .contains(&format!("\n- `{wire}`\n")));
        }
    }

    #[test]
    fn serde_rename_changes_fragment_without_changing_variant_coverage() {
        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        enum Renamed {
            #[serde(rename = "fixture-stop")]
            UserStop,
            UserAction,
            SupersededByFollowup,
            EffortRestart,
            Unknown,
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fragment = values(
            &root,
            "CancelCause",
            vec![
                ("UserStop", Renamed::UserStop),
                ("UserAction", Renamed::UserAction),
                ("SupersededByFollowup", Renamed::SupersededByFollowup),
                ("EffortRestart", Renamed::EffortRestart),
                ("Unknown", Renamed::Unknown),
            ],
        );
        // Pin the fixture's explicit serde rename, not a production spelling.
        assert!(fragment.contains("`fixture-stop`"));
        let manifest: serde_json::Value = serde_json::from_str(&render(&root)).unwrap();
        assert_ne!(
            fragment,
            manifest["documents"]["system-knowhow/thread-events.md"]["CancelCause"]
        );
    }

    #[test]
    #[should_panic(expected = "samples must cover every variant")]
    fn new_variant_cannot_be_silently_omitted() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        values(
            &root,
            "SessionEndReason",
            samples!(SessionEndReason; Shutdown),
        );
    }
}
