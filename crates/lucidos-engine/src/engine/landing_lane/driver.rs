//! The engine task that runs the landing lane.
//!
//! One task owns the [`Lane`] and takes one message at a time, so the queue,
//! the batch and every pass agree without a lock. Apply hands a change in and
//! waits on a oneshot for the answer. The lane thread's completion, Discard,
//! Set aside, Cancel and a closed thread reach it over the same channel.
//! Anything that waits on the lane itself, or on the lane thread, runs in a
//! spawned task, or the lane would wait on itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::integration::{
    closed_lanes_opened_here, create_lane_branch, ensure_lane_worktree, land_integration,
    lane_worktrees, main_before_last_move, retire_lane_worktree, verify_integration,
    IntegrationLanding, LaneWorktree,
};
use super::landing::{
    find_landing_merge, publish_clean_pass, LandingInput, LandingMerge, LandingStep,
};
use super::lane_thread::{
    lane_thread_closures, lane_thread_state, lane_thread_title, lane_turn_reason, LaneThreadEnd,
    LANE_THREAD_CLOSED, LANE_THREAD_CLOSING_EVENT_TYPES,
};
use super::recovery::{batch_recovery, BatchRecovery, LaneRecord, LaneRestore};
use super::snapshot::LaneSnapshot;
use super::state::{Lane, LaneRemoval, Place, ResolvingBatch};
use super::LaneMember;
use crate::core::changes::{Change, ChangeStatus};
use crate::engine::agent_session::spawn_claim::SpawnClaim;
use crate::engine::agent_session::{build_lane_catch_up_prompt, build_lane_thread_prompt};
use crate::engine::apply_all_driver::ApplyAllDriveMsg;
use crate::engine::change_ops::apply_claim::ApplyClaim;
use crate::engine::event_bus::{BusEvent, EmittedEvent, LagReplay, ReplayScope, SystemEvent};
use crate::engine::git_ops::{
    branch_head_sha, commits_in_range, files_have_client_update, find_worktree_for_branch,
    lane_branch, push_main_in_background, PlanMarkerKind, WorktreeLookup, MAIN_LOCKS,
};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent, UnqueueReason};
use crate::engine::until_restored::UntilRestored;
use crate::engine::{ApplyResult, ApplyStatus, LucidosEngine};

/// A change an Apply hands to the lane, read under its claim.
pub(crate) struct Submission {
    pub(crate) change: Change,
    pub(crate) claim: ApplyClaim,
    pub(crate) actor: Option<MessageOrigin>,
    /// The commit the lane lands. A change already queued keeps the snapshot
    /// it was queued at, whatever this says.
    pub(crate) snapshot_sha: String,
}

/// What the lane answers an Apply. Every `Err` was already announced as a
/// `ChangeApplyFailed`, as `apply_change` promises.
pub(crate) type LaneAnswer = Result<ApplyResult, String>;

pub(crate) enum LaneMsg {
    Submit(Box<Submission>, oneshot::Sender<LaneAnswer>),
    /// A run on the lane thread ended, and its completion decided how.
    ThreadEnded {
        batch_id: Uuid,
        thread_id: Uuid,
        end: LaneThreadEnd,
    },
    /// The lane thread is free after a clean finish. The hold keeps a new run
    /// off it while the batch lands.
    Land {
        batch_id: Uuid,
        hold: Box<SpawnClaim>,
    },
    /// Cancel from the lane: the batch fails whole.
    Cancel {
        batch_id: Uuid,
        actor: Option<MessageOrigin>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// A thread was archived or deleted. When it is the open batch's lane
    /// thread, the batch fails whole, as after a Cancel.
    ThreadClosed {
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    },
    /// Discard or Set aside resolved the change.
    Withdrawn(Uuid, UnqueueReason),
    /// A waiting member's apply claim went free after a pass found it held,
    /// or a pass could not read its row. The waiting pass runs again.
    ClaimReleased(Uuid),
    /// The user removed a waiting change from the lane.
    Remove {
        change_id: Uuid,
        actor: Option<MessageOrigin>,
        reply: oneshot::Sender<Result<(), LaneRemoval>>,
    },
    /// Apply All was canceled: its members leave the lane. The reply names
    /// the members the lane held, so Apply All leaves their threads alone.
    CancelMembers {
        change_ids: Vec<Uuid>,
        actor: Option<MessageOrigin>,
        reply: oneshot::Sender<Vec<Uuid>>,
    },
    /// An Apply All press started handing changes over.
    PressOpened(Uuid),
    /// That press has nothing left to hand over.
    PressSubmitted(Uuid),
    /// Recovery rebuilt the lane from events. The driver handles nothing
    /// before it (`UntilRestored`).
    Restored(Box<LaneRestore>),
}

const QUEUED_BEHIND_BATCH: &str = "Queued in the landing lane. It would not merge cleanly onto \
     main while a batch resolves, so it resolves in the next batch.";
const GATHERING_FOR_PRESS: &str = "Queued in the landing lane. It would not merge cleanly onto \
     main, so it resolves in one batch with the rest of the Apply All.";
const RESOLVING_IN_BATCH: &str = "It would not merge cleanly onto main, so it resolves in a \
     lane batch. It lands when the batch does.";
const LANDING_FAILED: &str = "The landing lane could not land this change";
const CANCELED: &str = "Canceled from the landing lane.";
const REMOVED: &str = "Removed from the landing lane.";
/// Why a restart takes out a member whose failed batch never unqueued it.
const BATCH_FAILED_BEFORE_RESTART: &str = "Its lane batch failed before the engine restarted.";
/// Why the lane branch needs no plan of its own: each member cleared the
/// plan floor at its own Apply.
const LANE_PLAN_REASON: &str = "Lane batch: integrates changes whose own plans were \
     cleared at Apply.";

/// One member of a clean pass or of the resolving batch, with everything its
/// landing announces. It carries the member's apply claim.
struct PassMember {
    member: LaneMember,
    change: Change,
    claim: ApplyClaim,
    actor: Option<MessageOrigin>,
}

struct Driver {
    engine: Arc<LucidosEngine>,
    repo_root: PathBuf,
    workspace: PathBuf,
    lane: Lane,
    /// Who pressed Apply for each waiting member, so its landing names them.
    actors: HashMap<Uuid, Option<MessageOrigin>>,
    /// The resolving batch's members, each under the apply claim the lane
    /// holds from batch start until the batch lands or fails (ADR 0442).
    held: HashMap<Uuid, PassMember>,
    /// The batch being landed: from its lane thread's clean finish until it
    /// lands, fails, or goes back to its lane thread for a catch-up turn.
    landing: Option<Uuid>,
    /// Every message waits here until recovery restored the lane.
    until_restored: UntilRestored<LaneMsg>,
}

impl LucidosEngine {
    /// Hand a message to the lane. A closed channel drops it: the events
    /// already written let recovery rebuild the lane at the next start.
    pub(crate) fn notify_landing_lane(&self, msg: LaneMsg) {
        if self.landing_lane_tx.send(msg).is_err() {
            log!("[LandingLane] driver gone, dropping a message; recovery rebuilds the lane");
        }
    }

    /// Queue a change and wait for the lane's answer: `Applied` when it
    /// landed in the clean pass, `Queued` when it waits or resolves. A
    /// `Queued` change's Apply All press stops waiting on it, whoever queued it.
    pub(crate) async fn submit_to_landing_lane(&self, submission: Submission) -> LaneAnswer {
        let change_id = submission.change.id;
        let (reply, answer) = oneshot::channel();
        self.notify_landing_lane(LaneMsg::Submit(Box::new(submission), reply));
        let answer = answer.await.unwrap_or_else(|_| {
            Err("The landing lane stopped before it answered. Apply again.".into())
        });
        if matches!(&answer, Ok(r) if matches!(r.status, ApplyStatus::Queued)) {
            self.notify_apply_all(ApplyAllDriveMsg::Laned(change_id));
        }
        answer
    }

    /// Report how a run on a lane thread ended. Its completion calls this.
    pub(crate) fn report_lane_thread_end(
        &self,
        batch_id: Uuid,
        thread_id: Uuid,
        end: LaneThreadEnd,
    ) {
        self.notify_landing_lane(LaneMsg::ThreadEnded {
            batch_id,
            thread_id,
            end,
        });
    }

    /// Cancel the resolving batch `batch_id`: it fails whole, and its lane
    /// thread stops. `Err` when no such batch is resolving.
    pub(crate) async fn cancel_lane_batch(
        &self,
        batch_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), String> {
        let (reply, answer) = oneshot::channel();
        self.notify_landing_lane(LaneMsg::Cancel {
            batch_id,
            actor,
            reply,
        });
        answer
            .await
            .unwrap_or_else(|_| Err("The landing lane stopped before it answered.".into()))
    }

    /// Remove a waiting change from the lane. `Err` when the lane stopped
    /// before it answered; the inner `Err` when the change may not go.
    pub(crate) async fn remove_from_landing_lane(
        &self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<Result<(), LaneRemoval>, String> {
        let (reply, answer) = oneshot::channel();
        self.notify_landing_lane(LaneMsg::Remove {
            change_id,
            actor,
            reply,
        });
        answer
            .await
            .map_err(|_| "The landing lane stopped before it answered.".to_string())
    }

    /// Take canceled Apply All members out of the lane: a waiting one leaves,
    /// and the open batch fails whole when it holds one. Returns the members
    /// the lane held. Empty when the lane stopped before it answered.
    pub(crate) async fn cancel_lane_members(
        &self,
        change_ids: Vec<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Vec<Uuid> {
        let (reply, answer) = oneshot::channel();
        self.notify_landing_lane(LaneMsg::CancelMembers {
            change_ids,
            actor,
            reply,
        });
        answer.await.unwrap_or_default()
    }

    /// An Apply All press starts handing changes over. Until it is submitted
    /// the lane opens no batch, so the press's conflicts resolve in one.
    pub(crate) fn open_lane_press(&self, press_id: Uuid) {
        self.notify_landing_lane(LaneMsg::PressOpened(press_id));
    }

    /// The press has nothing left to hand over. Repeating it is harmless.
    pub(crate) fn submit_lane_press(&self, press_id: Uuid) {
        self.notify_landing_lane(LaneMsg::PressSubmitted(press_id));
    }

    /// The lane as clients see it, as the driver last published it.
    pub(crate) fn landing_lane_snapshot(&self) -> LaneSnapshot {
        match self.landing_lane_snapshot.read() {
            Ok(snapshot) => snapshot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Start the lane's task. Spawned once at engine startup.
    pub fn start_landing_lane_driver(self: &Arc<Self>) {
        let rx = crate::engine::LANDING_LANE_RX.with(|cell| cell.borrow_mut().take());
        let Some(mut rx) = rx else {
            log!("[LandingLane] driver receiver missing, lane not started");
            return;
        };
        let mut driver = Driver {
            engine: self.clone(),
            repo_root: self.repo_root().to_path_buf(),
            workspace: self.workspace_path().to_path_buf(),
            lane: Lane::default(),
            actors: HashMap::new(),
            held: HashMap::new(),
            landing: None,
            until_restored: UntilRestored::default(),
        };
        // Weak: the claims live on the engine, so a strong handle would cycle.
        let engine = Arc::downgrade(self);
        self.apply_claims.on_lane_wait_release(move |change_id| {
            if let Some(engine) = engine.upgrade() {
                engine.notify_landing_lane(LaneMsg::ClaimReleased(change_id));
            }
        });
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let is_restore = matches!(msg, LaneMsg::Restored(_));
                for msg in driver.until_restored.admit(msg, is_restore) {
                    driver.handle(msg).await;
                }
            }
            log!("[LandingLane] driver exiting, channel closed");
        });
        self.start_lane_thread_closure_feed();
    }

    /// Tell the lane of every archived or deleted thread. It listens on the
    /// bus, since the button, a parent's cascade, an agent tool and a delete
    /// each persist one of these events. An idle lane thread has no run to
    /// report it. A lag replays what it dropped.
    fn start_lane_thread_closure_feed(self: &Arc<Self>) {
        let (mut rx, mut replay) = LagReplay::subscribe(
            &self.event_bus,
            ReplayScope::Workspace,
            Some(LANE_THREAD_CLOSING_EVENT_TYPES.to_vec()),
        );
        let engine = self.clone();
        tokio::spawn(async move {
            while forward_lane_thread_closures(rx.recv().await, &mut replay, engine.pool(), |msg| {
                engine.notify_landing_lane(msg)
            })
            .await
            {}
        });
    }

    /// Rebuild the lane from events and git after a restart.
    ///
    /// Runs after agent recovery decided the lane thread's own fate (a switch
    /// resume queued, or a Continue offered) and before the queued resumes
    /// drain. It always sends `Restored`, since the driver handles nothing
    /// before it. When the events cannot be read, the lane starts empty and
    /// retires no lane worktree.
    pub async fn recover_landing_lane(self: &Arc<Self>) {
        let restore = match self.read_lane_restore().await {
            Ok(restore) => restore,
            Err(e) => {
                log!(
                    "[LandingLane] recovery could not read the lane: {}; it starts empty",
                    e
                );
                LaneRestore::default()
            }
        };
        log!(
            "[LandingLane] recovered {} waiting change(s), batch open: {}",
            restore.lane.waiting().len(),
            restore.lane.resolving().is_some()
        );
        self.notify_landing_lane(LaneMsg::Restored(Box::new(restore)));
    }

    async fn read_lane_restore(&self) -> Result<LaneRestore, sqlx::Error> {
        let records = load_lane_records(self.pool()).await?;
        if records.is_empty() {
            return Ok(LaneRestore::default());
        }
        let pending: std::collections::HashSet<Uuid> =
            sqlx::query_scalar("SELECT id FROM changes WHERE status = 'pending'")
                .fetch_all(self.pool())
                .await?
                .into_iter()
                .collect();
        Ok(LaneRestore::of(&records, |id| pending.contains(&id)))
    }
}

impl Driver {
    async fn handle(&mut self, msg: LaneMsg) {
        match msg {
            LaneMsg::Submit(submission, reply) => {
                let answer = self.submit(*submission).await;
                // A caller that stopped waiting still gets its change landed.
                let _ = reply.send(answer);
            }
            LaneMsg::ThreadEnded {
                batch_id,
                thread_id,
                end,
            } => self.thread_ended(batch_id, thread_id, end).await,
            LaneMsg::Land { batch_id, hold } => self.land_batch(batch_id, *hold).await,
            LaneMsg::Cancel {
                batch_id,
                actor,
                reply,
            } => {
                let answer = self.cancel(batch_id, actor).await;
                let _ = reply.send(answer);
            }
            LaneMsg::ThreadClosed { thread_id, actor } => {
                if let Some(batch_id) = self.lane.batch_of_lane_thread(thread_id) {
                    self.end_from_outside(batch_id, LANE_THREAD_CLOSED, actor)
                        .await;
                }
            }
            LaneMsg::Withdrawn(change_id, reason) => {
                self.withdraw(change_id, reason, None).await;
            }
            LaneMsg::ClaimReleased(change_id) => {
                log!("[LandingLane] {} is free to claim again", change_id);
                self.run_waiting().await;
            }
            LaneMsg::Remove {
                change_id,
                actor,
                reply,
            } => {
                let answer = self.remove(change_id, actor).await;
                let _ = reply.send(answer);
            }
            LaneMsg::CancelMembers {
                change_ids,
                actor,
                reply,
            } => {
                let held = self.cancel_members(&change_ids, actor).await;
                let _ = reply.send(held);
            }
            LaneMsg::PressOpened(press_id) => {
                self.lane.open_press(press_id);
                if !self.lane.waiting().is_empty() {
                    self.announce().await;
                }
            }
            LaneMsg::PressSubmitted(press_id) => {
                if self.lane.submit_press(press_id) {
                    log!("[LandingLane] press {} is in, the queue passes", press_id);
                    self.run_waiting().await;
                }
            }
            LaneMsg::Restored(restore) => self.restore(*restore).await,
        }
        self.publish();
    }

    /// Publish the lane as clients see it.
    fn publish(&self) {
        let snapshot = LaneSnapshot::of(&self.lane, self.landing);
        match self.engine.landing_lane_snapshot.write() {
            Ok(mut published) => *published = snapshot,
            Err(poisoned) => *poisoned.into_inner() = snapshot,
        }
    }

    /// Publish the lane, then tell every client the changes moved, so the
    /// frame carries the lane as it stands now.
    async fn announce(&self) {
        self.publish();
        self.engine.broadcast_changes_updated().await;
    }

    async fn submit(&mut self, sub: Submission) -> LaneAnswer {
        let change_id = sub.change.id;
        match self.lane.place(change_id) {
            Place::Resolving => {
                // The lane holds every member's claim, so this Apply got one
                // only because the lane lost it. The lane takes it back.
                let thread = sub.change_thread();
                let member = self
                    .lane
                    .resolving()
                    .and_then(|b| b.members().find(|m| m.change_id == change_id).cloned());
                if let Some(member) = member {
                    self.held.entry(change_id).or_insert(PassMember {
                        member,
                        change: sub.change,
                        claim: sub.claim,
                        actor: sub.actor,
                    });
                }
                Ok(queued(change_id, thread, RESOLVING_IN_BATCH))
            }
            Place::Waiting if self.lane.queue_held() => {
                Ok(queued(change_id, sub.change_thread(), self.why_waiting()))
            }
            Place::Waiting => {
                // A pass ran while this Apply held the claim and skipped it.
                let snapshot_sha = self.snapshot_of(change_id, &sub.snapshot_sha);
                let mut members = self.claim_waiting(Some(change_id)).await;
                let thread = sub.change_thread();
                members.push(PassMember {
                    member: LaneMember {
                        change_id,
                        snapshot_sha,
                    },
                    change: sub.change,
                    claim: sub.claim,
                    actor: sub.actor,
                });
                self.pass_answer(members, change_id, thread).await
            }
            Place::Absent => self.arrive(sub).await,
        }
    }

    async fn arrive(&mut self, sub: Submission) -> LaneAnswer {
        let change_id = sub.change.id;
        let thread = sub.change_thread();
        let event_thread = thread.unwrap_or(change_id);
        let member = LaneMember {
            change_id,
            snapshot_sha: sub.snapshot_sha.clone(),
        };
        self.engine
            .event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id: event_thread,
                    event: ThreadEvent::ChangeQueued {
                        change_id: change_id.to_string(),
                        snapshot_sha: member.snapshot_sha.clone(),
                    },
                    meta: EventMeta::with_actor(sub.actor.clone()),
                },
                "[LandingLane] ChangeQueued",
            )
            .await;
        // Every arrival tries `main` first, whatever batch resolves: the
        // merge decides, not the files it touches (ADR 0480).
        let mut members = self.claim_waiting(None).await;
        members.push(PassMember {
            member,
            change: sub.change,
            claim: sub.claim,
            actor: sub.actor,
        });
        self.pass_answer(members, change_id, thread).await
    }

    /// Run a pass over `members` and return `change_id`'s answer.
    async fn pass_answer(
        &mut self,
        members: Vec<PassMember>,
        change_id: Uuid,
        thread: Option<Uuid>,
    ) -> LaneAnswer {
        let answer = self.run_pass(members).await.remove(&change_id);
        answer.unwrap_or_else(|| Ok(queued(change_id, thread, self.why_waiting())))
    }

    /// Why a waiting change waits: behind the resolving batch, or for the rest
    /// of an Apply All press.
    fn why_waiting(&self) -> &'static str {
        if self.lane.gathering() {
            GATHERING_FOR_PRESS
        } else {
            QUEUED_BEHIND_BATCH
        }
    }

    /// The snapshot the lane holds for a waiting change, else `fallback`.
    fn snapshot_of(&self, change_id: Uuid, fallback: &str) -> String {
        self.lane
            .waiting()
            .iter()
            .find(|m| m.change_id == change_id)
            .map_or_else(|| fallback.to_string(), |m| m.snapshot_sha.clone())
    }

    /// Take the waiting members for a pass, claiming each. One whose claim
    /// another holds stays queued, and that claim's release runs the pass
    /// again ([`LaneMsg::ClaimReleased`]). One whose row could not be read
    /// stays queued for the next lane message. One no longer pending leaves with
    /// `ChangeUnqueued`, so no restart queues it again.
    /// `skip` is a member the caller already holds.
    async fn claim_waiting(&mut self, skip: Option<Uuid>) -> Vec<PassMember> {
        let mut members = Vec::new();
        for member in self.lane.take_pass() {
            if Some(member.change_id) == skip {
                continue;
            }
            let Some(claim) = self
                .engine
                .apply_claims
                .try_claim_for_lane(member.change_id)
            else {
                self.lane.wait(member);
                continue;
            };
            match self.engine.changes().get_by_id(member.change_id).await {
                Ok(Some(change)) if change.is_pending() => {
                    let actor = self.actors.remove(&member.change_id).flatten();
                    members.push(PassMember {
                        member,
                        change,
                        claim,
                        actor,
                    });
                }
                Ok(change) => {
                    self.actors.remove(&member.change_id);
                    self.leave_resolved(member, change).await;
                }
                Err(e) => {
                    log!(
                        "[LandingLane] could not read {}: {}; the next lane message tries again",
                        member.change_id,
                        e
                    );
                    self.lane.wait(member);
                }
            }
        }
        members
    }

    /// A waiting member something else resolved leaves the lane, announced
    /// with the reason its status gives. One that landed already left with
    /// its `ChangeApplied`. The path that resolved it owns its thread's work:
    /// settling here would bring a set-aside change back (ADR 0328).
    async fn leave_resolved(&mut self, member: LaneMember, change: Option<Change>) {
        let Some(reason) = unqueue_reason_for(change.as_ref().map(Change::status)) else {
            return;
        };
        log!(
            "[LandingLane] {} is no longer pending, leaving the lane",
            member.change_id
        );
        let thread = change
            .as_ref()
            .and_then(|c| c.thread_id)
            .unwrap_or(member.change_id);
        self.emit_unqueued(thread, member.change_id, reason, None)
            .await;
    }

    /// Land what merges cleanly, set the rest aside, and announce each
    /// member's outcome. Every member that leaves the lane settles its source
    /// thread. Returns each member's answer.
    async fn run_pass(&mut self, members: Vec<PassMember>) -> HashMap<Uuid, LaneAnswer> {
        let mut answers = HashMap::new();
        if members.is_empty() {
            return answers;
        }
        let inputs: Vec<LandingInput> = members.iter().map(landing_input).collect();
        let mut settle = Vec::new();
        let publication = match publish_clean_pass(&MAIN_LOCKS, &self.repo_root, &inputs).await {
            Ok(publication) => publication,
            Err(e) => {
                for m in members {
                    let id = m.member.change_id;
                    settle.push(m.settle(false));
                    answers.insert(id, Err(self.fail_member(m, &e).await));
                }
                self.settle_sources(settle);
                return answers;
            }
        };
        let mut members: HashMap<Uuid, PassMember> = members
            .into_iter()
            .map(|m| (m.member.change_id, m))
            .collect();
        let mut set_aside = Vec::new();
        for (change_id, step) in &publication.chain.steps {
            let Some(m) = members.remove(change_id) else {
                continue;
            };
            let outcome = PassOutcome::of(step, publication.failed.as_deref());
            if let Some(landed) = outcome.settles_source() {
                settle.push(m.settle(landed));
            }
            let answer = match outcome {
                PassOutcome::Lands(merge) => Ok(self.announce_landing(m, Some(merge)).await),
                PassOutcome::AlreadyOnMain => Ok(self.announce_landing(m, None).await),
                PassOutcome::Fails(e) => Err(self.fail_member(m, e).await),
                PassOutcome::SetAside => {
                    set_aside.push(m);
                    continue;
                }
            };
            answers.insert(*change_id, answer);
        }
        if publication.main_now() != publication.base {
            push_main_in_background(&self.repo_root);
        }
        self.take_in_set_aside(set_aside, publication.main_now(), &mut answers)
            .await;
        self.announce().await;
        self.settle_sources(settle);
        answers
    }

    /// Members a pass could not land form the resolving batch, or wait while
    /// the queue is held. A waiting member lets its claim go. A new batch
    /// keeps its members' claims and opens its lane thread.
    async fn take_in_set_aside(
        &mut self,
        set_aside: Vec<PassMember>,
        base: &str,
        answers: &mut HashMap<Uuid, LaneAnswer>,
    ) {
        let members: Vec<LaneMember> = set_aside.iter().map(|m| m.member.clone()).collect();
        let opened = self
            .lane
            .set_aside(Uuid::new_v4(), base.to_string(), Uuid::new_v4(), members)
            .cloned();
        let Some(batch) = opened else {
            let why = self.why_waiting();
            for m in set_aside {
                let id = m.member.change_id;
                answers.insert(id, Ok(queued(id, m.change.thread_id, why)));
                self.actors.insert(id, m.actor);
            }
            return;
        };
        log!(
            "[LandingLane] batch {} opened with {} member(s), lane thread {}",
            batch.batch_id,
            set_aside.len(),
            batch.lane_thread_id
        );
        self.engine
            .event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::LaneBatchStarted {
                    batch_id: batch.batch_id,
                    members: batch.members().cloned().collect(),
                    base_sha: batch.base_sha.clone(),
                    lane_thread_id: batch.lane_thread_id,
                }),
                "[LandingLane] LaneBatchStarted",
            )
            .await;
        let branch = lane_branch(batch.batch_id);
        if let Err(e) = create_lane_branch(&self.repo_root, &branch, &batch.base_sha).await {
            // Opening the lane thread tries again.
            log!("[LandingLane] could not create {} yet: {}", branch, e);
        }
        for m in set_aside {
            let id = m.member.change_id;
            answers.insert(id, Ok(queued(id, m.change.thread_id, RESOLVING_IN_BATCH)));
            self.held.insert(id, m);
        }
        self.open_lane_thread(&batch);
    }

    /// Create the lane worktree and send the lane thread its first prompt.
    /// Spawned: the worktree takes a few seconds, and a pass must not wait.
    fn open_lane_thread(&self, batch: &ResolvingBatch) {
        let briefs: Vec<OwnedBrief> = batch
            .unlanded()
            .filter_map(|m| self.held.get(&m.change_id))
            .map(OwnedBrief::of)
            .collect();
        let titles: Vec<String> = briefs.iter().map(|b| b.title.clone()).collect();
        let title = lane_thread_title(&titles);
        let engine = self.engine.clone();
        let repo_root = self.repo_root.clone();
        let workspace = self.workspace.clone();
        let batch_id = batch.batch_id;
        let thread_id = batch.lane_thread_id;
        let base = batch.base_sha.clone();
        tokio::spawn(async move {
            let lane = LaneWorktree::of(&workspace, batch_id);
            if let Err(e) = ensure_lane_worktree(&repo_root, &workspace, &lane, &base).await {
                engine.report_lane_thread_end(
                    batch_id,
                    thread_id,
                    LaneThreadEnd::Failed(format!("The lane could not create its worktree: {e}")),
                );
                return;
            }
            if let Err(e) = engine
                .record_planned(
                    &repo_root,
                    &lane.branch,
                    PlanMarkerKind::AcknowledgedSimple,
                    None,
                    Some(LANE_PLAN_REASON),
                    &[],
                    &base,
                )
                .await
            {
                log!("[LandingLane] plan marker for {}: {}", lane.branch, e);
            }
            let briefs: Vec<_> = briefs.iter().map(OwnedBrief::as_brief).collect();
            let prompt = build_lane_thread_prompt(&briefs);
            spawn_lane_turn(&engine, batch_id, thread_id, prompt, Some(title));
        });
    }

    async fn thread_ended(&mut self, batch_id: Uuid, thread_id: Uuid, end: LaneThreadEnd) {
        if self.lane.resolving_batch(batch_id).is_none() {
            // Canceled while it ran. Now that the run let go, its worktree goes.
            self.cleanup_lane_worktree(batch_id, thread_id, None);
            return;
        }
        match end {
            LaneThreadEnd::Completed => {
                self.landing = Some(batch_id);
                self.announce().await;
                self.land_when_free(batch_id, thread_id);
            }
            LaneThreadEnd::Stopped => {
                log!(
                    "[LandingLane] lane thread {} stopped; batch {} waits for its next turn",
                    thread_id,
                    batch_id
                );
                if self.landing == Some(batch_id) {
                    self.landing = None;
                    self.announce().await;
                }
            }
            LaneThreadEnd::Failed(reason) => self.fail_batch(batch_id, reason, None, None).await,
        }
    }

    /// Wait out the lane thread's teardown, then land under its hold. A run
    /// that started meanwhile is a new turn on the batch: its own end decides.
    fn land_when_free(&self, batch_id: Uuid, thread_id: Uuid) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            match engine.hold_thread_for_worktree_reset(thread_id).await {
                Ok(Some(hold)) => engine.notify_landing_lane(LaneMsg::Land {
                    batch_id,
                    hold: Box::new(hold),
                }),
                Ok(None) => log!(
                    "[LandingLane] a new run holds lane thread {}; its end lands batch {}",
                    thread_id,
                    batch_id
                ),
                Err(e) => log!(
                    "[LandingLane] could not hold lane thread {}: {}",
                    thread_id,
                    e
                ),
            }
        });
    }

    /// Land a batch whose lane thread finished cleanly: check the integration
    /// branch, catch it up with `main`, and move `main`. A catch-up conflict
    /// goes back to the same lane thread as a follow-up turn.
    async fn land_batch(&mut self, batch_id: Uuid, hold: SpawnClaim) {
        let Some(batch) = self.lane.resolving_batch(batch_id).cloned() else {
            return;
        };
        if let Err(reason) = self.hold_members(&batch).await {
            self.fail_batch(batch_id, reason, None, Some(hold)).await;
            return;
        }
        let lane = LaneWorktree::of(&self.workspace, batch_id);
        let unlanded: Vec<LaneMember> = batch.unlanded().cloned().collect();
        let merges = match verify_integration(&lane.path, &batch.base_sha, &unlanded).await {
            Ok(merges) => merges,
            Err(e) => {
                let reason = format!("The lane thread finished, but {e}.");
                self.fail_batch(batch_id, reason, None, Some(hold)).await;
                return;
            }
        };
        match land_integration(&MAIN_LOCKS, &self.repo_root, &lane.path).await {
            Ok(IntegrationLanding::Landed { pre, post }) => {
                let mut landed = Vec::new();
                for (change_id, merge) in merges {
                    if let Some(m) = self.held.remove(&change_id) {
                        landed.push(self.land_member(m, &merge).await);
                    }
                }
                self.close_landed(&batch, pre, post, Some(hold), landed)
                    .await;
            }
            Ok(IntegrationLanding::CatchUpConflict { main_sha, commits }) => {
                log!(
                    "[LandingLane] main moved under batch {} and conflicts with it; back to its lane thread",
                    batch_id
                );
                drop(hold);
                self.landing = None;
                self.announce().await;
                let prompt = build_lane_catch_up_prompt(&main_sha, &commits);
                spawn_lane_turn(&self.engine, batch_id, batch.lane_thread_id, prompt, None);
            }
            Err(e) => {
                let reason = format!("The lane could not land the batch: {e}");
                self.fail_batch(batch_id, reason, None, Some(hold)).await;
            }
        }
    }

    /// Hold the claim of every member still to land. The lane holds them from
    /// batch start, so this takes back only what a restart released. A member
    /// another apply holds, or one no longer pending, fails the batch.
    async fn hold_members(&mut self, batch: &ResolvingBatch) -> Result<(), String> {
        for member in batch.unlanded() {
            if self.held.contains_key(&member.change_id) {
                continue;
            }
            let Some(claim) = self.engine.apply_claims.try_claim(member.change_id) else {
                return Err(format!(
                    "change {} is being applied elsewhere",
                    member.change_id
                ));
            };
            let change = match self.engine.changes().get_by_id(member.change_id).await {
                Ok(Some(change)) if change.is_pending() => change,
                Ok(_) => return Err(format!("change {} is no longer pending", member.change_id)),
                Err(e) => return Err(format!("could not read change {}: {e}", member.change_id)),
            };
            self.held.insert(
                member.change_id,
                PassMember {
                    member: member.clone(),
                    change,
                    claim,
                    actor: None,
                },
            );
        }
        Ok(())
    }

    /// Announce one member's landing merge under its claim, and count it
    /// landed. Returns what settling its source thread needs.
    async fn land_member(&mut self, m: PassMember, merge: &LandingMerge) -> (Change, String) {
        let settle = (m.change.clone(), m.member.snapshot_sha.clone());
        self.lane.landed(m.member.change_id);
        self.announce_landing(m, Some(merge)).await;
        settle
    }

    /// Close a batch whose members all landed: announce it, release the
    /// lane thread's worktree, and start the next pass.
    async fn close_landed(
        &mut self,
        batch: &ResolvingBatch,
        pre_sha: String,
        post_sha: String,
        hold: Option<SpawnClaim>,
        landed: Vec<(Change, String)>,
    ) {
        log!("[LandingLane] batch {} landed", batch.batch_id);
        self.landing = None;
        self.engine
            .event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::LaneBatchLanded {
                    batch_id: batch.batch_id,
                    pre_sha,
                    post_sha,
                }),
                "[LandingLane] LaneBatchLanded",
            )
            .await;
        self.lane.end_batch(batch.batch_id);
        push_main_in_background(&self.repo_root);
        self.cleanup_lane_worktree(batch.batch_id, batch.lane_thread_id, hold);
        self.announce().await;
        for (change, snapshot) in landed {
            self.spawn_settle_source(change, snapshot, true);
        }
        self.actors_retain();
        self.run_waiting().await;
    }

    /// The batch fails whole: every member leaves the lane, still pending and
    /// apply-ready, and nothing queues it again by itself.
    async fn fail_batch(
        &mut self,
        batch_id: Uuid,
        reason: String,
        actor: Option<MessageOrigin>,
        hold: Option<SpawnClaim>,
    ) {
        let Some(batch) = self.lane.end_batch(batch_id) else {
            return;
        };
        self.landing = None;
        log!("[LandingLane] batch {} failed: {}", batch_id, reason);
        self.engine
            .event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::LaneBatchFailed {
                    batch_id,
                    reason: reason.clone(),
                    actor,
                }),
                "[LandingLane] LaneBatchFailed",
            )
            .await;
        for member in batch.unlanded() {
            self.unqueue_failed_member(member.change_id, &reason).await;
        }
        self.cleanup_lane_worktree(batch_id, batch.lane_thread_id, hold);
        self.announce().await;
        self.actors_retain();
        self.run_waiting().await;
    }

    /// A failed batch's member leaves the lane with `ChangeUnqueued`, even
    /// when its row cannot be read. Only a pending one also ends its Apply All
    /// turn and settles its source thread.
    async fn unqueue_failed_member(&mut self, change_id: Uuid, reason: &str) {
        // Kept until the member has left the lane: its claim goes with it.
        let held = self.held.remove(&change_id);
        let change = match &held {
            Some(m) => Some(m.change.clone()),
            None => match self.engine.changes().get_by_id(change_id).await {
                Ok(change) => change,
                Err(e) => {
                    log!("[LandingLane] could not read {}: {}", change_id, e);
                    None
                }
            },
        };
        let thread = match &change {
            Some(change) => Some(change.thread_id.unwrap_or(change_id)),
            None => match super::queued_thread(self.engine.pool(), change_id).await {
                Ok(thread) => thread,
                Err(e) => {
                    log!(
                        "[LandingLane] could not find where {} queued: {}",
                        change_id,
                        e
                    );
                    None
                }
            },
        };
        match thread {
            Some(thread) => {
                self.emit_unqueued(thread, change_id, UnqueueReason::BatchFailed, None)
                    .await;
            }
            None => log!(
                "[LandingLane] {} left the lane unannounced; the next start unqueues it",
                change_id
            ),
        }
        let Some(change) = change.filter(Change::is_pending) else {
            return;
        };
        self.engine
            .notify_apply_all(ApplyAllDriveMsg::Failed(change_id, reason.to_string()));
        let snapshot = branch_head_sha(Path::new(&change.repo_root), &change.branch_name)
            .await
            .unwrap_or_default();
        self.spawn_settle_source(change, snapshot, false);
        drop(held);
    }

    /// Remove a closed batch's worktree and branch once its lane thread holds
    /// no run. With a live run, that run's end comes back here.
    fn cleanup_lane_worktree(&self, batch_id: Uuid, thread_id: Uuid, hold: Option<SpawnClaim>) {
        let engine = self.engine.clone();
        let repo_root = self.repo_root.clone();
        let lane = LaneWorktree::of(&self.workspace, batch_id);
        tokio::spawn(async move {
            let hold = match hold {
                Some(hold) => Some(hold),
                None => engine
                    .hold_thread_for_worktree_reset(thread_id)
                    .await
                    .unwrap_or_else(|e| {
                        log!(
                            "[LandingLane] could not hold lane thread {}: {}",
                            thread_id,
                            e
                        );
                        None
                    }),
            };
            let Some(hold) = hold else {
                log!(
                    "[LandingLane] a run holds lane thread {}; its end removes {}",
                    thread_id,
                    lane.branch
                );
                return;
            };
            retire_lane_worktree(engine.pool(), &repo_root, &lane).await;
            drop(hold);
        });
    }

    async fn cancel(&mut self, batch_id: Uuid, actor: Option<MessageOrigin>) -> Result<(), String> {
        if self.lane.resolving_batch(batch_id).is_none() {
            return Err("No lane batch with that id is resolving.".into());
        }
        self.end_from_outside(batch_id, CANCELED, actor).await;
        Ok(())
    }

    /// Fail the resolving batch `batch_id` from outside its lane thread, and
    /// stop the lane thread's run if one is live. Its end then retires the
    /// worktree. Cancel, an archive and a delete all end a batch this way.
    async fn end_from_outside(
        &mut self,
        batch_id: Uuid,
        reason: &str,
        actor: Option<MessageOrigin>,
    ) {
        let Some(batch) = self.lane.resolving_batch(batch_id).cloned() else {
            return;
        };
        let running = self.engine.is_agent_running_for(batch.lane_thread_id).await;
        self.fail_batch(batch_id, reason.into(), actor.clone(), None)
            .await;
        if running {
            if let Err(e) = self
                .engine
                .interrupt_agent(
                    Some(batch.lane_thread_id),
                    actor,
                    crate::engine::claude_code::SettleTerminal::StuckProjection,
                )
                .await
            {
                log!(
                    "[LandingLane] stopping lane thread {}: {}",
                    batch.lane_thread_id,
                    e
                );
            }
        }
    }

    /// Announce a landed member under its claim. `None` means its snapshot
    /// was already on `main`, so nothing merged.
    async fn announce_landing(&self, m: PassMember, merge: Option<&LandingMerge>) -> ApplyResult {
        let engine = &self.engine;
        let change = &m.change;
        let thread = m.thread();
        let Some(merge) = merge else {
            engine
                .emit_change_applied(
                    &m.claim,
                    thread,
                    false,
                    false,
                    Vec::new(),
                    change.thread_title.clone(),
                    m.actor,
                    None,
                )
                .await;
            return ApplyResult::noop(
                change.id,
                change.thread_id,
                change.files.len(),
                "Change already present on main, marked applied.",
            );
        };
        let commits =
            commits_in_range(&self.repo_root, &merge.first_parent, &merge.merge_sha).await;
        engine
            .emit_change_applied(
                &m.claim,
                thread,
                change.requires_restart,
                files_have_client_update(&change.files),
                commits.clone(),
                change.thread_title.clone(),
                m.actor.clone(),
                Some(crate::engine::change_ops::AppliedMerge::landing(
                    &merge.first_parent,
                    &merge.merge_sha,
                )),
            )
            .await;
        engine
            .emit_entity_events_for_change_apply(
                &change.files,
                Some(&merge.first_parent),
                Some(&merge.merge_sha),
                m.actor.clone(),
                change.thread_id,
            )
            .await;
        ApplyResult::applied_with_merge(
            change.id,
            change.thread_id,
            change.requires_restart,
            merge.first_parent.clone(),
            merge.merge_sha.clone(),
            &commits,
            change.files.len(),
        )
    }

    /// The lane could not land a member: it leaves the lane, still pending,
    /// and the failure is announced. Returns the message for the caller.
    async fn fail_member(&self, m: PassMember, error: &str) -> String {
        let message = format!("{LANDING_FAILED}: {error}. It is still pending; apply it again.");
        log!("[LandingLane] {}: {}", m.member.change_id, message);
        self.emit_unqueued(
            m.thread(),
            m.member.change_id,
            UnqueueReason::LandingFailed,
            None,
        )
        .await;
        self.engine
            .emit_apply_failed(m.thread(), m.member.change_id, &message, m.actor)
            .await;
        message
    }

    async fn emit_unqueued(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        reason: UnqueueReason,
        actor: Option<MessageOrigin>,
    ) {
        self.engine
            .event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeUnqueued {
                        change_id: change_id.to_string(),
                        reason,
                    },
                    meta: EventMeta::with_actor(actor),
                },
                "[LandingLane] ChangeUnqueued",
            )
            .await;
    }

    /// Start the next pass once the queue is free.
    async fn run_waiting(&mut self) {
        if self.lane.queue_held() || self.lane.waiting().is_empty() {
            return;
        }
        let members = self.claim_waiting(None).await;
        self.run_pass(members).await;
    }

    /// Drop a waiting change, and return its row. A resolving member never
    /// gets here: the lane holds its claim, which refuses the Discard or Set
    /// aside, and Remove asks [`Lane::removal`] first.
    async fn withdraw(
        &mut self,
        change_id: Uuid,
        reason: UnqueueReason,
        actor: Option<MessageOrigin>,
    ) -> Option<Change> {
        if !self.lane.unqueue(change_id) {
            return None;
        }
        self.actors.remove(&change_id);
        let change = self
            .engine
            .changes()
            .get_by_id(change_id)
            .await
            .ok()
            .flatten();
        let thread = change
            .as_ref()
            .and_then(|c| c.thread_id)
            .unwrap_or(change_id);
        self.emit_unqueued(thread, change_id, reason, actor).await;
        self.announce().await;
        change
    }

    /// Take a waiting change out of the lane. It stays pending and
    /// apply-ready, and its thread's withheld work is decided again, as after
    /// a failed batch. A member of an Apply All press ends its turn there.
    async fn remove(
        &mut self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), LaneRemoval> {
        self.lane.removal(change_id)?;
        let change = self
            .withdraw(change_id, UnqueueReason::Removed, actor)
            .await;
        self.engine
            .notify_apply_all(ApplyAllDriveMsg::Failed(change_id, REMOVED.into()));
        if let Some(change) = change.filter(Change::is_pending) {
            let snapshot = branch_head_sha(Path::new(&change.repo_root), &change.branch_name)
                .await
                .unwrap_or_default();
            self.spawn_settle_source(change, snapshot, false);
        }
        Ok(())
    }

    /// Take the restored lane. The driver handled nothing before this, so no
    /// claim it holds is dropped and no submission met an empty lane. A member
    /// a crash left queued after its batch failed leaves now.
    async fn restore(&mut self, restore: LaneRestore) {
        self.lane = restore.lane;
        self.held.clear();
        self.remove_closed_lane_worktrees(&restore.opened_here)
            .await;
        for change_id in restore.stranded {
            self.unqueue_failed_member(change_id, BATCH_FAILED_BEFORE_RESTART)
                .await;
        }
        self.heal_landed_waiting().await;
        if let Some(batch) = self.lane.resolving().cloned() {
            self.restore_batch(batch).await;
        }
        self.run_waiting().await;
    }

    /// A batch closes before its worktree goes, so a restart can find one a
    /// closed batch left. No run is alive at startup, so it goes now. Only
    /// a batch this workspace opened: another workspace on the same checkout
    /// owns the rest.
    async fn remove_closed_lane_worktrees(&self, opened_here: &std::collections::HashSet<Uuid>) {
        let open = self.lane.resolving().map(|b| b.batch_id);
        match lane_worktrees(&self.repo_root, &self.workspace).await {
            Ok(lanes) => {
                for lane in closed_lanes_opened_here(&lanes, opened_here, open) {
                    retire_lane_worktree(self.engine.pool(), &self.repo_root, lane).await;
                }
            }
            Err(e) => log!("[LandingLane] could not list lane branches: {}", e),
        }
    }

    /// Canceled Apply All members leave the lane. A waiting one goes as
    /// Remove takes it, and the open batch fails whole when it holds one.
    /// Returns the members the lane held.
    async fn cancel_members(
        &mut self,
        change_ids: &[Uuid],
        actor: Option<MessageOrigin>,
    ) -> Vec<Uuid> {
        let held = self.lane.held_of(change_ids);
        for &change_id in &held.waiting {
            if let Err(e) = self.remove(change_id, actor.clone()).await {
                log!(
                    "[LandingLane] {} did not leave the lane: {:?}",
                    change_id,
                    e
                );
            }
        }
        if let Some(batch_id) = held.batch {
            if let Err(e) = self.cancel(batch_id, actor).await {
                log!("[LandingLane] batch {} not canceled: {}", batch_id, e);
            }
        }
        held.members()
    }

    /// Bring the open batch back after a restart: take its claims again, heal
    /// a landing a crash cut off, then do what its lane thread's state says.
    async fn restore_batch(&mut self, batch: ResolvingBatch) {
        let batch_id = batch.batch_id;
        if let Err(reason) = self.hold_members(&batch).await {
            self.fail_batch(batch_id, reason, None, None).await;
            return;
        }
        let healed = self.heal_landed_members(&batch).await;
        let Some(batch) = self.lane.resolving_batch(batch_id).cloned() else {
            return;
        };
        if batch.all_landed() {
            let post = branch_head_sha(&self.repo_root, "main")
                .await
                .unwrap_or_default();
            let pre = main_before_last_move(&self.repo_root)
                .await
                .unwrap_or_else(|| batch.base_sha.clone());
            self.close_landed(&batch, pre, post, None, healed).await;
            return;
        }
        let lane = LaneWorktree::of(&self.workspace, batch_id);
        match lane_thread_state(self.engine.pool(), batch.lane_thread_id, &lane.branch).await {
            Ok(state) => match batch_recovery(state) {
                BatchRecovery::OpenLaneThread => self.open_lane_thread(&batch),
                BatchRecovery::Land => {
                    self.landing = Some(batch_id);
                    self.land_when_free(batch_id, batch.lane_thread_id);
                }
                BatchRecovery::AwaitLaneThread => log!(
                    "[LandingLane] batch {} waits for lane thread {}",
                    batch_id,
                    batch.lane_thread_id
                ),
                BatchRecovery::Fail => {
                    self.fail_batch(batch_id, LANE_THREAD_CLOSED.into(), None, None)
                        .await;
                }
            },
            Err(e) => log!(
                "[LandingLane] could not read lane thread {}: {}; batch {} stays open",
                batch.lane_thread_id,
                e,
                batch_id
            ),
        }
    }

    /// A crash between moving `main` and announcing the batch's members left
    /// landing merges on `main` that no `ChangeApplied` names. Announce them.
    async fn heal_landed_members(&mut self, batch: &ResolvingBatch) -> Vec<(Change, String)> {
        let mut healed = Vec::new();
        let unlanded: Vec<LaneMember> = batch.unlanded().cloned().collect();
        for member in unlanded {
            let merge = match find_landing_merge(&self.repo_root, &member.snapshot_sha).await {
                Ok(Some(merge)) => merge,
                Ok(None) => continue,
                Err(e) => {
                    log!(
                        "[LandingLane] could not search main for {}: {}",
                        member.change_id,
                        e
                    );
                    continue;
                }
            };
            let Some(m) = self.held.remove(&member.change_id) else {
                continue;
            };
            log!(
                "[LandingLane] {} landed as {} before a restart, announcing it",
                member.change_id,
                merge.merge_sha
            );
            healed.push(self.land_member(m, &merge).await);
        }
        healed
    }

    /// A crash between moving `main` and announcing a landing left a waiting
    /// change whose landing merge is already on `main`. Announce it now.
    async fn heal_landed_waiting(&mut self) {
        let waiting = self.lane.waiting().to_vec();
        let mut healed = false;
        for member in waiting {
            let merge = match find_landing_merge(&self.repo_root, &member.snapshot_sha).await {
                Ok(Some(merge)) => merge,
                Ok(None) => continue,
                Err(e) => {
                    log!(
                        "[LandingLane] could not search main for {}: {}",
                        member.change_id,
                        e
                    );
                    continue;
                }
            };
            let Some(claim) = self.engine.apply_claims.try_claim(member.change_id) else {
                continue;
            };
            let Ok(Some(change)) = self.engine.changes().get_by_id(member.change_id).await else {
                continue;
            };
            self.lane.unqueue(member.change_id);
            if !change.is_pending() {
                continue;
            }
            log!(
                "[LandingLane] {} landed as {} before a restart, announcing it",
                member.change_id,
                merge.merge_sha
            );
            let snapshot = member.snapshot_sha.clone();
            let m = PassMember {
                member,
                change: change.clone(),
                claim,
                actor: None,
            };
            self.announce_landing(m, Some(&merge)).await;
            self.spawn_settle_source(change, snapshot, true);
            healed = true;
        }
        if healed {
            push_main_in_background(&self.repo_root);
        }
    }

    /// Settle the source thread after its change landed or its batch failed.
    fn spawn_settle_source(&self, change: Change, snapshot: String, landed: bool) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            engine
                .settle_source_after_lane(change, &snapshot, landed)
                .await;
        });
    }

    fn settle_sources(&self, settle: Vec<SourceSettle>) {
        for s in settle {
            self.spawn_settle_source(s.change, s.snapshot, s.landed);
        }
    }

    fn actors_retain(&mut self) {
        let lane = &self.lane;
        self.actors
            .retain(|id, _| lane.place(*id) == Place::Waiting);
    }
}

impl LucidosEngine {
    /// What a source thread's worktree and work become once the lane is done
    /// with its change. Runs outside the lane's task, since a sibling discard
    /// or a thread hold can wait.
    ///
    /// A landed change resets the worktree to `main` only while it is exactly
    /// what landed: its branch still at the snapshot, its tree clean, and no
    /// run holding the thread. A branch that moved keeps its commits as
    /// unproposed work, and that work is decided again once. So is a failed
    /// member's.
    pub(crate) async fn settle_source_after_lane(
        &self,
        change: Change,
        snapshot: &str,
        landed: bool,
    ) {
        let Some(thread_id) = change.thread_id else {
            return;
        };
        if landed {
            self.discard_orphaned_pending_siblings(thread_id, change.id, None)
                .await;
        }
        let repo_root = PathBuf::from(&change.repo_root);
        let branch = change.branch_name.as_str();
        let at_snapshot = branch_head_sha(&repo_root, branch).await.as_deref() == Some(snapshot);
        if landed && at_snapshot {
            self.reset_landed_worktree(thread_id, &repo_root, branch, snapshot)
                .await;
            return;
        }
        let in_flight = self
            .agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .is_some_and(|s| s.is_in_flight());
        if in_flight {
            // The running turn's own end decides the work.
            return;
        }
        let finished =
            crate::engine::agent_recovery::last_turn_ended_cleanly(self.pool(), thread_id).await;
        if let Err(e) = self
            .redecide_branch_work(thread_id, branch, &repo_root, None, finished)
            .await
        {
            log!(
                "[LandingLane] deciding the work on {} again failed: {}",
                branch,
                e
            );
        }
    }

    async fn reset_landed_worktree(
        &self,
        thread_id: Uuid,
        repo_root: &Path,
        branch: &str,
        snapshot: &str,
    ) {
        let WorktreeLookup::Found(worktree) = find_worktree_for_branch(repo_root, branch).await
        else {
            return;
        };
        let hold = match self.hold_thread_for_worktree_reset(thread_id).await {
            Ok(Some(hold)) => hold,
            Ok(None) => {
                log!(
                    "[LandingLane] a run holds thread {}, leaving its worktree as it is",
                    thread_id
                );
                return;
            }
            Err(e) => {
                log!("[LandingLane] could not hold thread {}: {}", thread_id, e);
                return;
            }
        };
        // Asked again under the hold: a commit may have landed meanwhile.
        if branch_head_sha(repo_root, branch).await.as_deref() != Some(snapshot) {
            return;
        }
        if let Err(e) =
            crate::engine::change_ops::reset_worktree_to_main_after_apply(&worktree).await
        {
            log!("[LandingLane] kept worktree {}: {}", worktree.display(), e);
        }
        drop(hold);
    }
}

impl Submission {
    fn change_thread(&self) -> Option<Uuid> {
        self.change.thread_id
    }
}

impl PassMember {
    /// The thread the change's events go on.
    fn thread(&self) -> Uuid {
        self.change.thread_id.unwrap_or(self.member.change_id)
    }

    fn settle(&self, landed: bool) -> SourceSettle {
        SourceSettle {
            change: self.change.clone(),
            snapshot: self.member.snapshot_sha.clone(),
            landed,
        }
    }
}

/// A member's part of the lane thread's prompt, owned so the lane thread can
/// open outside the lane's task.
struct OwnedBrief {
    title: String,
    summary: String,
    branch: String,
    snapshot_sha: String,
}

impl OwnedBrief {
    fn of(m: &PassMember) -> Self {
        Self {
            title: member_title(&m.change),
            summary: m.change.description.clone(),
            branch: m.change.branch_name.clone(),
            snapshot_sha: m.member.snapshot_sha.clone(),
        }
    }

    fn as_brief(&self) -> crate::engine::agent_session::LaneMemberBrief<'_> {
        crate::engine::agent_session::LaneMemberBrief {
            title: &self.title,
            summary: &self.summary,
            branch: &self.branch,
            snapshot_sha: &self.snapshot_sha,
        }
    }
}

/// Hand `notify` a [`LaneMsg::ThreadClosed`] for every thread one read of
/// the bus closes, including what a lag dropped. `false` once the bus closes.
pub(super) async fn forward_lane_thread_closures(
    received: Result<EmittedEvent, RecvError>,
    replay: &mut LagReplay,
    pool: &sqlx::PgPool,
    notify: impl Fn(LaneMsg),
) -> bool {
    let Some(frames) = replay.read(received, pool).await else {
        return false;
    };
    for frame in frames {
        for (thread_id, actor) in lane_thread_closures(&frame.typed) {
            notify(LaneMsg::ThreadClosed { thread_id, actor });
        }
    }
    true
}

/// Every event the lane's state depends on, oldest first.
pub(super) async fn load_lane_records(pool: &sqlx::PgPool) -> Result<Vec<LaneRecord>, sqlx::Error> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, payload FROM events \
         WHERE event_type IN ('ChangeQueued', 'ChangeUnqueued', 'LaneBatchStarted', \
                              'LaneBatchLanded', 'LaneBatchFailed') \
            OR (event_type = 'ChangeApplied' \
                AND payload->>'change_id' IN \
                    (SELECT payload->>'change_id' FROM events WHERE event_type = 'ChangeQueued')) \
         ORDER BY sequence",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .filter_map(|(event_type, payload)| lane_record(event_type, payload))
        .collect())
}

/// One stored event as a [`LaneRecord`]. Thread events store their fields
/// flat, system events under `data`.
fn lane_record(event_type: &str, payload: &serde_json::Value) -> Option<LaneRecord> {
    let field = |name: &str| payload.get(name).and_then(|v| v.as_str());
    let change_id = || field("change_id").and_then(|s| Uuid::parse_str(s).ok());
    let data = payload.get("data");
    let data_uuid = |name: &str| {
        data.and_then(|d| d.get(name))
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
    };
    Some(match event_type {
        "ChangeQueued" => LaneRecord::Queued(LaneMember {
            change_id: change_id()?,
            snapshot_sha: field("snapshot_sha")?.to_string(),
        }),
        "ChangeUnqueued" => LaneRecord::Unqueued(change_id()?),
        "ChangeApplied" => LaneRecord::Applied(change_id()?),
        "LaneBatchStarted" => LaneRecord::BatchStarted {
            batch_id: data_uuid("batch_id")?,
            base_sha: data?.get("base_sha")?.as_str()?.to_string(),
            lane_thread_id: data_uuid("lane_thread_id")?,
            members: serde_json::from_value(data?.get("members")?.clone()).ok()?,
        },
        "LaneBatchLanded" | "LaneBatchFailed" => LaneRecord::BatchEnded(data_uuid("batch_id")?),
        _ => return None,
    })
}

/// What names a member's landing merge and its place in the lane thread:
/// its thread title, else the first line of its description, else its branch.
fn member_title(change: &Change) -> String {
    change
        .thread_title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| {
            change
                .description
                .lines()
                .next()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| change.branch_name.clone())
}

/// Send the lane thread a turn. A turn that fails before its run answers
/// fails the batch whole, as any failure to start does. A run that did start
/// answered already, and the closed batch makes a second answer a no-op.
fn spawn_lane_turn(
    engine: &Arc<LucidosEngine>,
    batch_id: Uuid,
    thread_id: Uuid,
    prompt: String,
    title: Option<String>,
) {
    let reporter = engine.clone();
    engine.spawn_engine_coding_turn_or_else(
        thread_id,
        prompt,
        title,
        lane_turn_reason(),
        move |e| {
            reporter.report_lane_thread_end(
                batch_id,
                thread_id,
                LaneThreadEnd::Failed(format!("The lane thread's turn could not start: {e}")),
            );
        },
    );
}

/// The reason a waiting member leaves the lane when something else resolved
/// it, by its status. `None` for one that landed or is still pending: a
/// landing takes it out with its own `ChangeApplied`. A withdrawn or vanished
/// change never comes back under its id, so it leaves as discarded.
pub(super) fn unqueue_reason_for(status: Option<ChangeStatus>) -> Option<UnqueueReason> {
    match status {
        Some(ChangeStatus::Pending | ChangeStatus::Applied | ChangeStatus::Reverted) => None,
        Some(ChangeStatus::SetAside) => Some(UnqueueReason::SetAside),
        Some(ChangeStatus::Discarded | ChangeStatus::Withdrawn) | None => {
            Some(UnqueueReason::Discarded)
        }
    }
}

/// What a clean pass did with one member.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PassOutcome<'a> {
    Lands(&'a LandingMerge),
    /// Its snapshot was already on `main`: it lands with no merge.
    AlreadyOnMain,
    /// `main` could not move, so it leaves the lane, still pending.
    Fails(&'a str),
    /// It would conflict, so it joins a lane batch.
    SetAside,
}

impl<'a> PassOutcome<'a> {
    pub(super) fn of(step: &'a LandingStep, failed: Option<&'a str>) -> Self {
        match (step, failed) {
            (LandingStep::Landed(merge), None) => Self::Lands(merge),
            (LandingStep::Landed(_), Some(e)) => Self::Fails(e),
            (LandingStep::AlreadyOnMain, _) => Self::AlreadyOnMain,
            (LandingStep::SetAside, _) => Self::SetAside,
        }
    }

    /// Whether the member leaves the lane, which settles its source thread:
    /// `Some(true)` as landed, `Some(false)` as still pending. A member set
    /// aside stays in the lane, so its thread waits on.
    pub(super) fn settles_source(&self) -> Option<bool> {
        match self {
            Self::Lands(_) | Self::AlreadyOnMain => Some(true),
            Self::Fails(_) => Some(false),
            Self::SetAside => None,
        }
    }
}

/// A source thread to settle once the lane is done with its change.
struct SourceSettle {
    change: Change,
    snapshot: String,
    landed: bool,
}

/// The subject and body of a member's landing merge.
fn landing_input(m: &PassMember) -> LandingInput {
    LandingInput {
        change_id: m.member.change_id,
        snapshot_sha: m.member.snapshot_sha.clone(),
        subject: format!("Land: {}", member_title(&m.change)),
        body: format!(
            "Change: {}\nBranch: {}",
            m.member.change_id, m.change.branch_name
        ),
    }
}

fn queued(change_id: Uuid, thread_id: Option<Uuid>, message: &str) -> ApplyResult {
    ApplyResult::queued(change_id, thread_id, message)
}
