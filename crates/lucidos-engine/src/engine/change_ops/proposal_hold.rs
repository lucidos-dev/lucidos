//! The implementation-plan floor and the proposal hold: the one decision on
//! whether a branch's work may become a change at all.
//!
//! Apply asks [`LucidosEngine::plan_floor`], and every proposal asks
//! [`read_proposal_hold`]: the same decision, plus the bounded-fix bound and
//! the hardening a merge would need. So a turn end never offers an Apply that
//! Apply would refuse or harden first. Why:
//! `docs/plans/2026-10-09-never-propose-without-plan-marker.md` and
//! `docs/plans/2026-10-10-plan-only-branch-skips-hardening.md`.

use super::{load_apply_kind_context, ApplyKindContext};
use crate::engine::git_ops::{
    bounded_fix_refusal_for, harden_marker, has_full_harden_marker, lands_plan_files_only,
    needs_hardening, plan_marker_files, plan_marker_state, BoundedFixInputs, HardenMarkerState,
    HardenTier, PlanMarkerKind, PlanMarkerState,
};
use crate::engine::LucidosEngine;
use std::path::Path;
use uuid::Uuid;

/// What the plan floor says about a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanFloor {
    /// Not Lucidos source. App and external-repo threads do not plan.
    Exempt,
    /// The marker satisfies the gate.
    Clear(PlanMarkerKind),
    /// Neither propose nor apply until the marker satisfies the gate.
    Held(PlanHold),
}

/// Why the floor holds a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanHold {
    /// No marker at all.
    Missing,
    /// A plan is recorded but the user has not approved it.
    AwaitingApproval,
    /// A plan awaits approval and the branch lands plan files alone. The
    /// decision is the user's, so the agent is not sent back to ask again.
    PlanOnlyAwaitingApproval,
    /// A bounded security fix whose work reaches past the files it named.
    OutsideBound,
}

impl PlanFloor {
    pub(crate) fn decide(kind: &ApplyKindContext, state: PlanMarkerState) -> Self {
        if !kind.is_lucidos_source() {
            return PlanFloor::Exempt;
        }
        match state {
            PlanMarkerState::Present(k) if k.satisfies_gate() => PlanFloor::Clear(k),
            PlanMarkerState::Present(_) => PlanFloor::Held(PlanHold::AwaitingApproval),
            PlanMarkerState::Missing => PlanFloor::Held(PlanHold::Missing),
        }
    }
}

impl PlanFloor {
    /// Re-judge an awaiting-approval hold against the files that would land.
    /// An empty list keeps the hold: it is no evidence of a plan-only branch.
    pub(crate) fn with_landing(self, landing: &[String]) -> Self {
        match self {
            PlanFloor::Held(PlanHold::AwaitingApproval) if lands_plan_files_only(landing) => {
                PlanFloor::Held(PlanHold::PlanOnlyAwaitingApproval)
            }
            floor => floor,
        }
    }
}

/// Whether Apply must harden a branch before it merges. Both Apply routes ask
/// it. Apps own their hardening, and a `None` landing list (git could not
/// answer) counts as work that needs it.
pub(crate) fn apply_must_harden(
    kind: &ApplyKindContext,
    hardened: bool,
    landing: Option<&[String]>,
) -> bool {
    !kind.is_app() && needs_hardening(hardened, landing.unwrap_or_default())
}

impl PlanHold {
    /// What a refused Apply tells the user.
    pub(crate) fn apply_refusal(self) -> &'static str {
        match self {
            PlanHold::AwaitingApproval | PlanHold::PlanOnlyAwaitingApproval => {
                "The implementation plan on this branch is awaiting approval. The user must \
                 approve the plan, after which the coding agent runs `lucidos planned approve` \
                 to unblock implementation. Approve the plan, then apply."
            }
            PlanHold::Missing => {
                "No implementation-plan marker on this branch. Before applying, the coding \
                 agent must run the `implementation-plan` skill (records a plan for the user to \
                 approve) or `lucidos planned mark --simple \"<reason>\"` (acknowledges a local \
                 fix). Re-run the session to set the marker, then apply."
            }
            PlanHold::OutsideBound => {
                "This branch carries a bounded security-fix marker and changes files outside the \
                 bound it named. Drop the extra files, re-run `lucidos planned mark --security-fix` \
                 with the full list, or write a plan and have the user approve it."
            }
        }
    }

    /// What the engine tells a live agent whose turn-end proposal it held.
    pub(crate) fn agent_nudge(self) -> &'static str {
        match self {
            PlanHold::AwaitingApproval | PlanHold::PlanOnlyAwaitingApproval => {
                "Lucidos did not propose this branch's change. Its implementation plan still \
                 awaits the user's approval, so Apply would refuse it. Ask for approval with your \
                 question tool, `Approve` first. Once the user approves, run \
                 `lucidos planned approve` and end your turn: Lucidos then proposes the change. \
                 If nobody can be asked (an unattended run), do not ask: end your turn with the \
                 `BLOCKED ON PLAN DECISION:` line the `implementation-plan` skill describes."
            }
            PlanHold::Missing => {
                "Lucidos did not propose this branch's change. The branch has commits but no \
                 implementation-plan marker, so Apply would refuse it. A marker is consumed when a \
                 change applies, so work after an Apply needs a new one. If the work is complex, \
                 run the `implementation-plan` skill and ask for approval with your question tool. \
                 If it is a genuinely local fix, run `lucidos planned mark --simple \"<one-line \
                 reason>\"`. Then end your turn: Lucidos then proposes the change. If nobody can \
                 be asked (an unattended run), follow that skill's unattended lanes instead."
            }
            PlanHold::OutsideBound => {
                "Lucidos did not propose this branch's change. Its bounded security-fix marker \
                 names fewer files than the branch changes, so Apply would refuse it. Drop the \
                 extra files, or re-run `lucidos planned mark --security-fix` with the full list. \
                 If the fix is wider than the lane allows, take the blocked lane in the \
                 `implementation-plan` skill. Then end your turn."
            }
        }
    }
}

/// Why a turn end's work may not become a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProposalHold {
    /// The plan floor holds the branch.
    Plan(PlanHold),
    /// The work never ran `/harden`, so Apply would harden it first.
    HardeningMissing,
    /// An integrated sub-thread's hardened work. It hands its branch up to
    /// its parent, whose agent merges it, so it never becomes a change.
    IntegratedIntoParent,
    /// The thread handed off to a legacy successor thread. Its branch work rides on
    /// the successor's branch, so this thread never proposes it.
    CarriedBySuccessor,
}

impl ProposalHold {
    /// What a refused Apply tells the user.
    pub(crate) fn apply_refusal(self) -> &'static str {
        match self {
            ProposalHold::Plan(hold) => hold.apply_refusal(),
            ProposalHold::HardeningMissing => {
                "This branch's work has not been hardened, so Lucidos did not propose it. \
                 Press Harden on the thread: the hardening run proposes the change when it \
                 finishes."
            }
            ProposalHold::IntegratedIntoParent => INTEGRATED_HAS_NO_APPLY,
            ProposalHold::CarriedBySuccessor => CARRIED_HAS_NO_APPLY,
        }
    }

    /// What the engine tells a live agent whose turn-end proposal it held.
    pub(crate) fn agent_nudge(self) -> &'static str {
        match self {
            ProposalHold::Plan(hold) => hold.agent_nudge(),
            ProposalHold::HardeningMissing => {
                "Lucidos did not propose this branch's change. The branch never ran `/harden`, \
                 so Apply would have to harden it first. Run `/harden` now over the whole \
                 batch of commits, then end your turn: Lucidos then proposes the change."
            }
            ProposalHold::IntegratedIntoParent => INTEGRATED_HAS_NO_APPLY,
            ProposalHold::CarriedBySuccessor => CARRIED_HAS_NO_APPLY,
        }
    }
}

/// Why an integrated sub-thread's work has no Apply.
const INTEGRATED_HAS_NO_APPLY: &str =
    "This thread is integrated into its parent. Lucidos never proposes its work: the parent's \
     agent merges its branch, and the parent's change carries it.";

/// Why a thread that handed off has no Apply.
const CARRIED_HAS_NO_APPLY: &str =
    "This thread continued in a legacy successor thread, whose branch carries its work. Lucidos \
     never proposes it here: the successor's change carries it to Apply.";

/// The plan floor for a branch whose thread kind is already loaded. Skips the
/// marker read for an exempt thread.
pub(crate) async fn read_plan_floor(
    pool: &sqlx::PgPool,
    kind: &ApplyKindContext,
    repo_root: &Path,
    branch_name: &str,
) -> PlanFloor {
    if !kind.is_lucidos_source() {
        return PlanFloor::Exempt;
    }
    PlanFloor::decide(kind, plan_marker_state(pool, repo_root, branch_name).await)
}

/// The plan floor for `thread_id`'s branch, loading the thread's kind first.
async fn read_plan_floor_for_thread(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    repo_root: &Path,
    branch_name: &str,
) -> PlanFloor {
    let kind = load_apply_kind_context(pool, Some(thread_id)).await;
    read_plan_floor(pool, &kind, repo_root, branch_name).await
}

/// The floor a proposal of `files` must clear: the plan floor, plus the bound a
/// bounded security fix named. Apply runs the same bound check on what lands.
async fn read_proposal_floor(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    repo_root: &Path,
    branch_name: &str,
    files: &[String],
) -> PlanFloor {
    let floor = read_plan_floor_for_thread(pool, thread_id, repo_root, branch_name)
        .await
        .with_landing(files);
    let PlanFloor::Clear(kind) = floor else {
        return floor;
    };
    if !kind.is_file_bounded() {
        return floor;
    }
    let refusal = bounded_fix_refusal_for(BoundedFixInputs {
        bound: plan_marker_files(pool, repo_root, branch_name).await,
        committed: Ok(files.to_vec()),
        dirty: Ok(Vec::new()),
    });
    match refusal {
        Some(_) => PlanFloor::Held(PlanHold::OutsideBound),
        None => floor,
    }
}

/// Why a proposal of `files` is held, or `None` when it may become a change.
/// Lucidos-source work the plan floor clears is held until it is hardened,
/// unless it lands plan files only.
///
/// An integrated sub-thread's work is never proposed, unless it lands plan
/// files only. A fresh marker of either tier hands it up to the parent;
/// without one it waits for hardening. Every other branch counts only a full
/// marker, fresh or stale.
pub(crate) async fn read_proposal_hold(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    repo_root: &Path,
    branch_name: &str,
    files: &[String],
) -> Option<ProposalHold> {
    if crate::engine::successor::handed_off(pool, thread_id).await {
        return Some(ProposalHold::CarriedBySuccessor);
    }
    match read_proposal_floor(pool, thread_id, repo_root, branch_name, files).await {
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

/// An integrated sub-thread's hold: handed up once a fresh marker covers its
/// work, held for hardening until then. A branch of plan files alone needs no
/// hardening and has no marker to hand up, so it proposes like any other.
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
    pub(crate) async fn plan_floor(
        &self,
        kind: &ApplyKindContext,
        repo_root: &Path,
        branch_name: &str,
    ) -> PlanFloor {
        read_plan_floor(&self.pool, kind, repo_root, branch_name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::agent_session::CodingAgentKind;

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

    /// Both Apply routes ask this. An app owns its hardening, a plan has none,
    /// and an unanswered landing read never skips it.
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

    /// An integrated sub-thread proposes no code: hardened work is handed
    /// up, and unhardened work waits for hardening. A plan alone needs no
    /// hardening and leaves no marker to hand up, so it proposes normally.
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

    /// The read propose and Apply share: the thread's kind from its
    /// `SessionStarted`, the marker from `planned_branches`.
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
        let floor = || read_plan_floor_for_thread(&pool, lucidos_thread, repo.path(), "work");

        assert_eq!(floor().await, PlanFloor::Held(PlanHold::Missing));
        let mark = |k| record_planned(&pool, repo.path(), "work", k, None, None, &[], "abc");
        mark(PlanMarkerKind::Proposed).await.expect("mark proposed");
        assert_eq!(floor().await, PlanFloor::Held(PlanHold::AwaitingApproval));
        let propose = |files: &'static [&'static str]| {
            let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
            let pool = pool.clone();
            let repo = repo.path().to_path_buf();
            async move { read_proposal_floor(&pool, lucidos_thread, &repo, "work", &files).await }
        };
        assert_eq!(
            propose(&["docs/plans/x.md"]).await,
            PlanFloor::Held(PlanHold::PlanOnlyAwaitingApproval)
        );
        assert_eq!(
            propose(&["docs/plans/x.md", "src/main.rs"]).await,
            PlanFloor::Held(PlanHold::AwaitingApproval)
        );
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
            read_plan_floor_for_thread(&pool, app_thread, repo.path(), "app-work").await,
            PlanFloor::Exempt
        );

        teardown_test_db(&db).await;
    }

    /// A bounded security fix clears the proposal floor only while the files
    /// it would propose stay inside the bound it named, as Apply requires.
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
            read_proposal_floor(&pool, thread, repo.path(), "fix", &bound).await,
            PlanFloor::Clear(PlanMarkerKind::BoundedSecurityFix)
        );
        assert_eq!(
            read_proposal_floor(&pool, thread, repo.path(), "fix", &wider).await,
            PlanFloor::Held(PlanHold::OutsideBound)
        );

        teardown_test_db(&db).await;
    }

    /// The nudge gets a held branch moving again. So it names both ways to a
    /// satisfying marker, or the approve step for a waiting plan.
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
        // An unattended run cannot ask, so each nudge names its way out.
        assert!(waiting.contains("BLOCKED ON PLAN DECISION:"), "{waiting}");
        assert!(missing.contains("unattended"), "{missing}");
        let outside = PlanHold::OutsideBound.agent_nudge();
        assert!(
            outside.contains("lucidos planned mark --security-fix"),
            "{outside}"
        );
    }

    /// A plan-only hold is the user's decision. Apply refuses it like any
    /// waiting plan, with the same wire reason.
    #[test]
    fn a_plan_only_hold_refuses_apply_like_a_waiting_plan() {
        assert_eq!(
            PlanHold::PlanOnlyAwaitingApproval.apply_refusal(),
            PlanHold::AwaitingApproval.apply_refusal()
        );
    }
}
