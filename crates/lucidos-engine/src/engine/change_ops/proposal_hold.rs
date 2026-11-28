use super::{load_apply_kind_context, ApplyKindContext};
use crate::core::event_hooks::{PendingEvent, APPLY_CHECK_EVENT, PROPOSAL_CHECK_EVENT};
use crate::engine::event_bus::EventBus;
use crate::engine::git_ops::{
    bounded_fix_refusal_for, harden_marker, has_full_harden_marker, lands_plan_files_only,
    needs_hardening, plan_marker_snapshot, BoundedFixInputs, HardenMarkerState, HardenTier,
    PlanMarkerKind, PlanMarkerSnapshot, PlanMarkerState,
};
use crate::engine::thread_events::CodingAgentKind;
use crate::engine::LucidosEngine;
use serde_json::{json, Value};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanFloor {
    Exempt,
    Clear(PlanMarkerKind),
    Held(PlanHold),
}

pub use crate::engine::thread_events::change_hold::{PlanHold, ProposalHold};

impl PlanFloor {
    pub(crate) fn decide(kind: &ApplyKindContext, state: PlanMarkerState) -> Self {
        if !kind.is_lucidos_source() {
            return PlanFloor::Exempt;
        }
        match state {
            PlanMarkerState::Present(PlanMarkerKind::PlanOnly) => {
                PlanFloor::Held(PlanHold::PlanOnlyCode)
            }
            PlanMarkerState::Present(k) if k.satisfies_gate() => PlanFloor::Clear(k),
            PlanMarkerState::Present(_) => PlanFloor::Held(PlanHold::AwaitingApproval),
            PlanMarkerState::Changed => PlanFloor::Held(PlanHold::Changed),
            PlanMarkerState::Missing => PlanFloor::Held(PlanHold::Missing),
            PlanMarkerState::Unreadable => PlanFloor::Held(PlanHold::Unreadable),
        }
    }
}

impl PlanFloor {
    /// INVARIANT: Re-judge an awaiting-approval hold against the files that would land.
    /// An empty list keeps the hold: it is no evidence of a plan-only branch.
    pub fn with_landing(self, landing: &[String]) -> Self {
        match self {
            PlanFloor::Held(PlanHold::PlanOnlyCode)
            | PlanFloor::Clear(PlanMarkerKind::PlanOnly) => {
                if lands_plan_files_only(landing) {
                    PlanFloor::Clear(PlanMarkerKind::PlanOnly)
                } else {
                    PlanFloor::Held(PlanHold::PlanOnlyCode)
                }
            }
            PlanFloor::Held(PlanHold::AwaitingApproval) if lands_plan_files_only(landing) => {
                PlanFloor::Held(PlanHold::PlanOnlyAwaitingApproval)
            }
            floor => floor,
        }
    }
}

/// INVARIANT: Whether Apply must harden a branch before it merges. Both Apply routes ask
/// it. Apps own their hardening, and a `None` landing list (git could not
/// answer) counts as work that needs it.
pub fn apply_must_harden(
    kind: &ApplyKindContext,
    hardened: bool,
    landing: Option<&[String]>,
) -> bool {
    !kind.is_app() && needs_hardening(hardened, landing.unwrap_or_default())
}

pub fn change_plan_context(
    kind: CodingAgentKind,
    marker: &PlanMarkerSnapshot,
    landing: Option<&[String]>,
) -> Value {
    let (state, marker_kind) = match marker.state {
        PlanMarkerState::Present(kind) => ("present", Some(kind.as_db())),
        PlanMarkerState::Missing => ("missing", None),
        PlanMarkerState::Changed => ("changed", None),
        PlanMarkerState::Unreadable => ("unreadable", None),
    };
    json!({
        "coding_agent_kind": kind.as_str(),
        "landing_files": landing,
        "plan_marker": {
            "state": state,
            "kind": marker_kind,
            "file_bound": marker.file_bound,
        },
    })
}

async fn observe_plan(
    bus: &EventBus,
    thread_id: Uuid,
    event_type: &str,
    context: Value,
    refused: bool,
) {
    bus.observe_change_hooks(
        &PendingEvent {
            event_type: event_type.to_string(),
            payload: json!({ "thread_id": thread_id, "change_context": context }),
        },
        thread_id,
        refused,
    )
    .await;
}

async fn read_proposal_floor(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    repo_root: &Path,
    branch_name: &str,
    files: &[String],
    bus: Option<&EventBus>,
) -> PlanFloor {
    let context = load_apply_kind_context(pool, Some(thread_id)).await;
    let marker = if context.is_lucidos_source() {
        plan_marker_snapshot(pool, repo_root, branch_name).await
    } else {
        PlanMarkerSnapshot {
            state: PlanMarkerState::Missing,
            file_bound: None,
        }
    };
    let floor = PlanFloor::decide(&context, marker.state).with_landing(files);
    let floor = if matches!(floor, PlanFloor::Clear(kind) if kind.is_file_bounded())
        && bounded_fix_refusal_for(BoundedFixInputs {
            bound: Ok(marker.file_bound.clone().unwrap_or_default()),
            committed: Ok(files.to_vec()),
            dirty: Ok(Vec::new()),
        })
        .is_some()
    {
        PlanFloor::Held(PlanHold::OutsideBound)
    } else {
        floor
    };
    if let Some(bus) = bus {
        observe_plan(
            bus,
            thread_id,
            PROPOSAL_CHECK_EVENT,
            change_plan_context(context.kind, &marker, Some(files)),
            matches!(floor, PlanFloor::Held(_)),
        )
        .await;
    }
    floor
}

pub async fn read_proposal_hold(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    repo_root: &Path,
    branch_name: &str,
    files: &[String],
) -> Option<ProposalHold> {
    if crate::engine::handoff_state::handed_off(pool, thread_id).await {
        return Some(ProposalHold::CarriedBySuccessor);
    }
    match read_proposal_floor(pool, thread_id, repo_root, branch_name, files, Some(bus)).await {
        PlanFloor::Held(hold) => Some(ProposalHold::Plan(hold)),
        PlanFloor::Exempt => None,
        PlanFloor::Clear(_) => {
            if crate::engine::integrated::thread_is_integrated(pool, thread_id).await {
                let fresh = harden_marker(pool, repo_root, branch_name)
                    .await
                    .state_for(HardenTier::Fast)
                    == HardenMarkerState::Fresh;
                return integrated_hold(fresh, files);
            }
            let hardened = has_full_harden_marker(pool, repo_root, branch_name).await;
            needs_hardening(hardened, files).then_some(ProposalHold::HardeningMissing)
        }
    }
}

pub async fn held_plan(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<PlanHold> {
    use crate::engine::thread_events::UnproposedReason;
    let reason: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'reason' FROM events \
         WHERE thread_id = $1 AND event_type = 'ProposalWithheld' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log_legacy!("[Changes] Could not read the plan hold of {thread_id}: {e}");
        None
    })
    .flatten();
    match reason.as_deref().and_then(UnproposedReason::parse)? {
        UnproposedReason::PlanChanged => Some(PlanHold::Changed),
        UnproposedReason::PlanMissing => Some(PlanHold::Missing),
        UnproposedReason::PlanUnreadable => Some(PlanHold::Unreadable),
        UnproposedReason::PlanAwaitingApproval => Some(PlanHold::AwaitingApproval),
        UnproposedReason::OutsideBound => Some(PlanHold::OutsideBound),
        UnproposedReason::PlanOnlyCode => Some(PlanHold::PlanOnlyCode),
        _ => None,
    }
}

fn integrated_hold(fresh_marker: bool, files: &[String]) -> Option<ProposalHold> {
    if lands_plan_files_only(files) {
        None
    } else if fresh_marker {
        Some(ProposalHold::IntegratedIntoParent)
    } else {
        Some(ProposalHold::HardeningMissing)
    }
}

impl LucidosEngine {
    pub async fn plan_floor(
        &self,
        kind: &ApplyKindContext,
        repo_root: &Path,
        branch_name: &str,
    ) -> (PlanFloor, PlanMarkerSnapshot) {
        let marker = if kind.is_lucidos_source() {
            plan_marker_snapshot(&self.pool, repo_root, branch_name).await
        } else {
            PlanMarkerSnapshot {
                state: PlanMarkerState::Missing,
                file_bound: None,
            }
        };
        (PlanFloor::decide(kind, marker.state), marker)
    }

    pub async fn observe_apply_plan(
        &self,
        thread_id: Uuid,
        kind: CodingAgentKind,
        marker: &PlanMarkerSnapshot,
        landing: Option<&[String]>,
        refused: bool,
    ) {
        observe_plan(
            &self.event_bus,
            thread_id,
            APPLY_CHECK_EVENT,
            change_plan_context(kind, marker, landing),
            refused,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::CodingAgentKind;

    fn kind(kind: CodingAgentKind) -> ApplyKindContext {
        ApplyKindContext { kind, app_id: None }
    }

    #[test]
    fn lucidos_source_is_held_until_the_marker_satisfies_the_gate() {
        let lucidos = kind(CodingAgentKind::Lucidos);
        assert_eq!(
            PlanFloor::decide(&lucidos, PlanMarkerState::Missing),
            PlanFloor::Held(PlanHold::Missing)
        );
        assert_eq!(
            PlanFloor::decide(&lucidos, PlanMarkerState::Present(PlanMarkerKind::Proposed)),
            PlanFloor::Held(PlanHold::AwaitingApproval)
        );
        assert_eq!(
            PlanFloor::decide(&lucidos, PlanMarkerState::Changed),
            PlanFloor::Held(PlanHold::Changed)
        );
        for k in [
            PlanMarkerKind::Planned,
            PlanMarkerKind::AcknowledgedSimple,
            PlanMarkerKind::BoundedSecurityFix,
        ] {
            assert_eq!(
                PlanFloor::decide(&lucidos, PlanMarkerState::Present(k)),
                PlanFloor::Clear(k)
            );
        }
    }

    #[test]
    fn plan_only_approval_never_opens_the_edit_gate_or_lands_code() {
        let marker = PlanMarkerKind::parse("plan_only");
        assert!(
            !marker.satisfies_gate(),
            "approval for later cannot permit source edits"
        );
        let floor = PlanFloor::decide(
            &kind(CodingAgentKind::Lucidos),
            PlanMarkerState::Present(marker),
        );
        let plan = vec!["docs/plans/example.md".to_string()];
        assert_eq!(floor.with_landing(&plan), PlanFloor::Clear(marker));
        assert!(matches!(floor.with_landing(&[]), PlanFloor::Held(_)));
        let mixed = vec![plan[0].clone(), "src/main.rs".to_string()];
        assert!(matches!(floor.with_landing(&mixed), PlanFloor::Held(_)));
        assert!(matches!(
            floor.with_landing(&plan).with_landing(&mixed),
            PlanFloor::Held(_)
        ));
    }

    #[test]
    fn an_unapproved_plan_only_branch_is_held_as_a_plan_only_hold() {
        let waiting = PlanFloor::Held(PlanHold::AwaitingApproval);
        let plan = vec!["docs/plans/2026-10-09-x.md".to_string()];
        let plan_only = PlanFloor::Held(PlanHold::PlanOnlyAwaitingApproval);
        assert_eq!(waiting.with_landing(&plan), plan_only);
        assert_eq!(
            plan_only.with_landing(&plan),
            plan_only,
            "re-judging a plan-only hold keeps it"
        );
        let mixed = vec![plan[0].clone(), "src/main.rs".to_string()];
        assert_eq!(waiting.with_landing(&mixed), waiting);
        assert_eq!(
            waiting.with_landing(&[]),
            waiting,
            "no files is no evidence"
        );
        let missing = PlanFloor::Held(PlanHold::Missing);
        assert_eq!(
            missing.with_landing(&plan),
            missing,
            "a plan file without a recorded plan is still unplanned"
        );
    }

    #[test]
    fn apply_hardens_unhardened_lucidos_work_beyond_a_plan() {
        let lucidos = kind(CodingAgentKind::Lucidos);
        let app = kind(CodingAgentKind::App);
        let plan = vec!["docs/plans/2026-10-10-x.md".to_string()];
        let code = vec!["src/main.rs".to_string()];
        assert!(apply_must_harden(&lucidos, false, Some(&code)));
        assert!(!apply_must_harden(&lucidos, true, Some(&code)));
        assert!(!apply_must_harden(&lucidos, false, Some(&plan)));
        assert!(
            apply_must_harden(&lucidos, false, None),
            "unknown is not a plan"
        );
        assert!(!apply_must_harden(&app, false, Some(&code)));
        assert!(!apply_must_harden(&app, false, None));
    }

    #[test]
    fn an_integrated_hold_proposes_only_a_plan() {
        let code = vec!["src/main.rs".to_string()];
        let plan = vec!["docs/plans/2026-10-13-x.md".to_string()];
        let mixed = vec![plan[0].clone(), code[0].clone()];
        assert_eq!(
            integrated_hold(true, &code),
            Some(ProposalHold::IntegratedIntoParent)
        );
        assert_eq!(
            integrated_hold(false, &code),
            Some(ProposalHold::HardeningMissing)
        );
        assert_eq!(
            integrated_hold(false, &mixed),
            Some(ProposalHold::HardeningMissing)
        );
        assert_eq!(integrated_hold(false, &plan), None);
        assert_eq!(integrated_hold(true, &plan), None);
        assert_eq!(
            integrated_hold(false, &[]),
            Some(ProposalHold::HardeningMissing),
            "no files is no evidence of a plan"
        );
    }

    #[test]
    fn app_and_external_threads_are_exempt_whatever_the_marker() {
        for k in [CodingAgentKind::App, CodingAgentKind::External] {
            assert_eq!(
                PlanFloor::decide(&kind(k), PlanMarkerState::Missing),
                PlanFloor::Exempt
            );
        }
    }

    #[tokio::test]
    async fn the_floor_reads_the_thread_kind_and_the_branch_marker() {
        use crate::engine::event_bus::{BusEvent, EventBus};
        use crate::engine::git_ops::record_planned;
        use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
        use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};

        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let repo = tempfile::tempdir().expect("repo dir");
        let lucidos_thread = Uuid::new_v4();
        start_cc_session(&bus, lucidos_thread, "work", None).await;
        let floor = || read_proposal_floor(&pool, lucidos_thread, repo.path(), "work", &[], None);

        assert_eq!(floor().await, PlanFloor::Held(PlanHold::Missing));
        let mark = |k| record_planned(&pool, repo.path(), "work", k, None, None, &[], "abc");
        mark(PlanMarkerKind::Proposed).await.expect("mark proposed");
        assert_eq!(floor().await, PlanFloor::Held(PlanHold::AwaitingApproval));
        let propose = |files: &'static [&'static str]| {
            let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
            let pool = pool.clone();
            let repo = repo.path().to_path_buf();
            async move { read_proposal_floor(&pool, lucidos_thread, &repo, "work", &files, None).await }
        };
        assert_eq!(
            propose(&["docs/plans/x.md"]).await,
            PlanFloor::Held(PlanHold::PlanOnlyAwaitingApproval)
        );
        assert_eq!(
            propose(&["docs/plans/x.md", "src/main.rs"]).await,
            PlanFloor::Held(PlanHold::AwaitingApproval)
        );
        mark(PlanMarkerKind::PlanOnly)
            .await
            .expect("mark plan-only");
        assert_eq!(floor().await, PlanFloor::Held(PlanHold::PlanOnlyCode));
        assert_eq!(
            propose(&["docs/plans/x.md"]).await,
            PlanFloor::Clear(PlanMarkerKind::PlanOnly)
        );
        assert_eq!(
            propose(&["docs/plans/x.md", "src/main.rs"]).await,
            PlanFloor::Held(PlanHold::PlanOnlyCode)
        );
        assert_eq!(
            crate::engine::git_ops::plan_marker_state(&pool, repo.path(), "work").await,
            PlanMarkerState::Present(PlanMarkerKind::PlanOnly)
        );
        sqlx::query("UPDATE planned_branches SET kind = 'design' WHERE branch_name = 'work'")
            .execute(&pool)
            .await
            .unwrap();
        crate::engine::git_ops::carry_planned(&pool, repo.path(), "work", "next")
            .await
            .unwrap();
        let copied_kind: String =
            sqlx::query_scalar("SELECT kind FROM planned_branches WHERE branch_name = 'next'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(copied_kind, "design");
        mark(PlanMarkerKind::AcknowledgedSimple)
            .await
            .expect("mark simple");
        assert_eq!(
            floor().await,
            PlanFloor::Clear(PlanMarkerKind::AcknowledgedSimple)
        );

        let app_thread = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id: app_thread,
            event: ThreadEvent::SessionStarted {
                sandbox: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: String::new(),
                branch: "app-work".into(),
                repo_id: None,
                coding_agent_kind: CodingAgentKind::App,
                coding_agent_folder: "/home/u/ws/data/apps/habit-tracker".into(),
                app_id: Some("habit-tracker".into()),
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("start the app session");
        assert_eq!(
            read_proposal_floor(&pool, app_thread, repo.path(), "app-work", &[], None).await,
            PlanFloor::Exempt
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_bounded_fix_outside_its_bound_is_held() {
        use crate::engine::event_bus::EventBus;
        use crate::engine::git_ops::record_planned;
        use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};

        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let repo = tempfile::tempdir().expect("repo dir");
        let thread = Uuid::new_v4();
        start_cc_session(&bus, thread, "fix", None).await;
        let bound = vec!["src/a.rs".to_string()];
        record_planned(
            &pool,
            repo.path(),
            "fix",
            PlanMarkerKind::BoundedSecurityFix,
            None,
            Some("finding"),
            &bound,
            "abc",
        )
        .await
        .expect("mark the bounded fix");

        let wider = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
        assert_eq!(
            read_proposal_floor(&pool, thread, repo.path(), "fix", &bound, None).await,
            PlanFloor::Clear(PlanMarkerKind::BoundedSecurityFix)
        );
        assert_eq!(
            read_proposal_floor(&pool, thread, repo.path(), "fix", &wider, None).await,
            PlanFloor::Held(PlanHold::OutsideBound)
        );

        teardown_test_db(&db).await;
    }

    #[test]
    fn each_nudge_names_the_command_that_clears_its_hold() {
        let missing = PlanHold::Missing.agent_nudge();
        assert!(missing.contains("implementation-plan"), "{missing}");
        assert!(
            missing.contains("lucidos planned mark --simple"),
            "{missing}"
        );
        let waiting = PlanHold::AwaitingApproval.agent_nudge();
        assert!(waiting.contains("lucidos planned approve"), "{waiting}");
        assert!(waiting.contains("question tool"), "{waiting}");
        assert!(waiting.contains("BLOCKED ON PLAN DECISION:"), "{waiting}");
        assert!(missing.contains("unattended"), "{missing}");
        let outside = PlanHold::OutsideBound.agent_nudge();
        assert!(
            outside.contains("lucidos planned mark --security-fix"),
            "{outside}"
        );
    }

    #[test]
    fn a_plan_only_hold_refuses_apply_like_a_waiting_plan() {
        assert_eq!(
            PlanHold::PlanOnlyAwaitingApproval.apply_refusal(),
            PlanHold::AwaitingApproval.apply_refusal()
        );
    }
}
