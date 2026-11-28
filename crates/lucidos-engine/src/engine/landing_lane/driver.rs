//! INVARIANT: The engine task that runs the landing lane.
//!
//! One task owns the [`Lane`] and takes one message at a time, so the queue,
//! the batch and every pass agree without a lock. Apply hands a change in and
//! waits on a oneshot for the answer. The lane thread's completion, Discard,
//! Set aside, Cancel and a closed thread reach it over the same channel.
//! Anything that waits on the lane itself, or on the lane thread, runs in a
//! spawned task, or the lane would wait on itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::integration::{
    closed_lanes_opened_here, create_lane_branch, ensure_lane_worktree, land_integration,
    lane_worktrees, main_before_last_move, retire_lane_worktree, verify_integration,
    CertificateCarry, IntegrationCertification, IntegrationLanding, LaneWorktree,
};
use super::landing::{
    find_landing_merge, publish_clean_pass, CleanPass, LandingInput, LandingMerge, LandingStep,
};
use super::lane_thread::{
    lane_thread_closures, lane_thread_state, lane_thread_title, lane_turn_reason, LaneThreadEnd,
    LANE_THREAD_CLOSED, LANE_THREAD_CLOSING_EVENT_TYPES,
};
use super::recovery::{batch_recovery, BatchFailure, BatchRecovery, LaneRecord, LaneRestore};
use super::remote::{self, QueueNote};
use super::snapshot::LaneSnapshot;
use super::state::{Lane, LaneRemoval, Place, ResolvingBatch};
use super::{handoff_started, LaneHoldRelease, LaneMember};
use crate::core::changes::{Change, ChangeStatus};
use crate::core::prefs::LANDING_SETTING;
use crate::engine::apply_all_driver::ApplyAllDriveMsg;
use crate::engine::change_ops::apply_claim::ApplyClaim;
use crate::engine::event_bus::{BusEvent, EmittedEvent, LagReplay, ReplayScope, SystemEvent};
use crate::engine::git_ops::{
    branch_head_sha, commits_in_range, files_have_client_update, find_worktree_for_branch,
    lane_branch, push_main_in_background, LandingOrder, PlanMarkerKind, WorktreeLookup, MAIN_LOCKS,
};
use crate::engine::merge_prompts::{build_lane_catch_up_prompt, build_lane_thread_prompt};
use crate::engine::session_claim::SpawnClaim;
use crate::engine::thread_events::landing_lane::LandingSetting;
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent, UnqueueReason};
use crate::engine::until_restored::UntilRestored;
use crate::engine::{ApplyResult, ApplyStatus, LucidosEngine};

pub(crate) struct Submission {
    pub(crate) change: Change,
    pub(crate) claim: ApplyClaim,
    pub(crate) actor: Option<MessageOrigin>,
    pub(crate) snapshot_sha: String,
}

pub(crate) type LaneAnswer = Result<ApplyResult, String>;

pub(crate) enum LaneMsg {
    SettingChanged,
    Submit(Box<Submission>, oneshot::Sender<LaneAnswer>),
    ThreadEnded {
        batch_id: Uuid,
        thread_id: Uuid,
        end: LaneThreadEnd,
    },
    /// INVARIANT: The lane thread is free after a clean finish. The hold keeps a new run
    /// off it while the batch lands.
    Land {
        batch_id: Uuid,
        hold: Box<SpawnClaim>,
    },
    Cancel {
        batch_id: Uuid,
        actor: Option<MessageOrigin>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ThreadClosed {
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    },
    Withdrawn(Uuid, UnqueueReason),
    ClaimReleased(Uuid),
    OriginAnswered,
    MainFollowed(String),
    Remove {
        change_id: Uuid,
        actor: Option<MessageOrigin>,
        reply: oneshot::Sender<Result<(), LaneRemoval>>,
    },
    /// INVARIANT: Apply All was canceled: its members leave the lane. The reply names
    /// the members the lane held, so Apply All leaves their threads alone.
    CancelMembers {
        change_ids: Vec<Uuid>,
        actor: Option<MessageOrigin>,
        reply: oneshot::Sender<Vec<Uuid>>,
    },
    PressOpened(Uuid),
    PressSubmitted(Uuid),
    /// INVARIANT: Recovery rebuilt the lane from events. The driver handles nothing
    /// before it (`UntilRestored`).
    Restored(Box<LaneRestore>),
    HoldCapReached(Uuid),
}

const ARRIVAL_HOLD_CAP: Duration = Duration::from_secs(15 * 60);

const QUEUED_BEHIND_BATCH: &str = "Queued in the landing lane. It would not merge cleanly onto \
     main while a batch resolves, so it resolves in the next batch.";
const GATHERING_FOR_PRESS: &str = "Queued in the landing lane. It would not merge cleanly onto \
     main, so it resolves in one batch with the rest of the Apply All.";
const RESOLVING_IN_BATCH: &str = "It would not merge cleanly onto main, so it resolves in a \
     lane batch. It lands when the batch does.";
const HELD_FOR_BATCH: &str = "Queued in the landing lane. A lane batch is hardening again \
     after catching up with main, so it lands after that batch, in arrival order.";
const WAITING_FOR_ORIGIN: &str = "Queued in the landing lane. origin did not take the push, so \
     it lands on the lane's next try.";
const LANDING_FAILED: &str = "The landing lane could not land this change";
const CANCELED: &str = "Canceled from the landing lane.";
const REMOVED: &str = "Removed from the landing lane.";
const BATCH_FAILED_BEFORE_RESTART: &str = "Its lane batch failed before the engine restarted.";
const LANE_PLAN_REASON: &str = "Lane batch: integrates changes whose own plans were \
     cleared at Apply.";

struct PassMember {
    member: LaneMember,
    local: Option<LocalPassMember>,
    actor: Option<MessageOrigin>,
}

struct LocalPassMember {
    change: Change,
    claim: ApplyClaim,
}

struct Driver {
    setting: LandingSetting,
    engine: Arc<LucidosEngine>,
    repo_root: PathBuf,
    workspace: PathBuf,
    lane: Lane,
    actors: HashMap<Uuid, Option<MessageOrigin>>,
    held: HashMap<Uuid, PassMember>,
    /// INVARIANT: The batch being landed: from its lane thread's clean finish until it
    /// lands, fails, or goes back to its lane thread for a catch-up turn.
    landing: Option<Uuid>,
    waiting_for_origin: Option<String>,
    multiple_landers: bool,
    batch_waiting_for_origin: Option<(Uuid, Uuid)>,
    remote_batch_failure: Option<BatchFailure>,
    /// INVARIANT: Waiting members a pass returned because `origin` refused it. They pass
    /// again when `origin` answers, even while a batch holds the queue.
    origin_waiters: HashSet<Uuid>,
    /// INVARIANT: Members the hold cap released while their apply claim was busy. They
    /// pass once the claim frees, even while a batch holds the queue.
    cap_owed: HashSet<Uuid>,
    until_restored: UntilRestored<LaneMsg>,
}

impl LucidosEngine {
    pub(crate) fn notify_landing_lane(&self, msg: LaneMsg) {
        if self.landing_lane_tx.send(msg).is_err() {
            log_legacy!(
                "[LandingLane] driver gone, dropping a message; recovery rebuilds the lane"
            );
        }
    }

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

    pub fn report_lane_thread_end(&self, batch_id: Uuid, thread_id: Uuid, end: LaneThreadEnd) {
        self.notify_landing_lane(LaneMsg::ThreadEnded {
            batch_id,
            thread_id,
            end,
        });
    }

    pub async fn cancel_lane_batch(
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

    pub async fn remove_from_landing_lane(
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

    pub(crate) fn open_lane_press(&self, press_id: Uuid) {
        self.notify_landing_lane(LaneMsg::PressOpened(press_id));
    }

    pub(crate) fn submit_lane_press(&self, press_id: Uuid) {
        self.notify_landing_lane(LaneMsg::PressSubmitted(press_id));
    }

    pub(crate) fn notify_origin_answered(&self) {
        self.notify_landing_lane(LaneMsg::OriginAnswered);
    }

    pub fn landing_lane_snapshot(&self) -> LaneSnapshot {
        match self.landing_lane_snapshot.read() {
            Ok(snapshot) => snapshot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn start_landing_lane_driver(self: &Arc<Self>) {
        let rx = crate::engine::LANDING_LANE_RX.with(|cell| cell.borrow_mut().take());
        let Some(mut rx) = rx else {
            log_legacy!("[LandingLane] driver receiver missing, lane not started");
            return;
        };
        let mut driver = Driver {
            setting: LandingSetting::default(),
            engine: self.clone(),
            repo_root: self.repo_root().to_path_buf(),
            workspace: self.workspace_path().to_path_buf(),
            lane: Lane::default(),
            actors: HashMap::new(),
            held: HashMap::new(),
            landing: None,
            waiting_for_origin: None,
            multiple_landers: false,
            batch_waiting_for_origin: None,
            remote_batch_failure: None,
            origin_waiters: HashSet::new(),
            cap_owed: HashSet::new(),
            until_restored: UntilRestored::default(),
        };
        // INVARIANT: Weak: the claims live on the engine, so a strong handle would cycle.
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
            log_legacy!("[LandingLane] driver exiting, channel closed");
        });
        self.start_lane_event_feed();
    }

    fn start_lane_event_feed(self: &Arc<Self>) {
        let (mut rx, mut replay) = LagReplay::subscribe(
            &self.event_bus,
            ReplayScope::Workspace,
            Some(lane_event_types()),
        );
        let engine = self.clone();
        tokio::spawn(async move {
            while forward_lane_events(rx.recv().await, &mut replay, engine.pool(), |msg| {
                engine.notify_landing_lane(msg)
            })
            .await
            {}
        });
    }

    /// INVARIANT: Rebuild the lane from events and git after a restart.
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
                log_legacy!(
                    "[LandingLane] recovery could not read the lane: {}; it starts empty",
                    e
                );
                LaneRestore::default()
            }
        };
        log_legacy!(
            "[LandingLane] recovered {} waiting change(s), batch open: {}",
            restore.lane.waiting().len(),
            restore.lane.resolving().is_some()
        );
        self.notify_landing_lane(LaneMsg::Restored(Box::new(restore)));
    }

    async fn read_lane_restore(
        &self,
    ) -> Result<LaneRestore, Box<dyn std::error::Error + Send + Sync>> {
        settle_restored_lane(self.pool(), &self.event_bus).await?;
        let records = load_lane_records(self.pool()).await?;
        if records.is_empty() {
            return Ok(LaneRestore::default());
        }
        let pending: HashSet<Uuid> =
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
                // INVARIANT: A caller that stopped waiting still gets its change landed.
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
                log_legacy!("[LandingLane] {} is free to claim again", change_id);
                if self.cap_owed.remove(&change_id) && self.lane.queue_held() {
                    let taken = self.lane.take_members(&HashSet::from([change_id]));
                    let members = self.claim_members(taken, None).await;
                    self.note_cap_owed(&members, [change_id]);
                    self.run_pass(members).await;
                    self.announce().await;
                }
                self.run_waiting().await;
            }
            LaneMsg::MainFollowed(repo) => {
                if self.setting == LandingSetting::Here
                    && self.repo_root == Path::new(&repo)
                    && self.lane.resolving().is_some()
                {
                    self.multiple_landers = true;
                    self.announce().await;
                }
            }
            LaneMsg::OriginAnswered => {
                if self.setting == LandingSetting::AnotherHost {
                    self.heal_landed_waiting().await;
                    self.run_waiting().await;
                    self.announce().await;
                } else {
                    self.retry_remote_failure().await;
                    self.intake_origin().await;
                    self.retry_origin().await;
                    self.heal_landed_waiting().await;
                }
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
                    log_legacy!("[LandingLane] press {} is in, the queue passes", press_id);
                    self.run_waiting().await;
                }
            }
            LaneMsg::Restored(restore) => {
                self.read_setting().await;
                self.restore(*restore).await;
            }
            LaneMsg::SettingChanged => {
                self.read_setting().await;
                if self.setting == LandingSetting::AnotherHost {
                    self.multiple_landers = false;
                    self.retry_remote_failure().await;
                }
                self.engine.nudge_follow_main();
                self.announce().await;
            }
            LaneMsg::HoldCapReached(batch_id) => self.hold_cap_reached(batch_id).await,
        }
        self.publish();
    }

    async fn intake_origin(&mut self) {
        if crate::runtime::is_packaged()
            || !matches!(
                crate::engine::git_ops::origin_remote(&self.repo_root).await,
                crate::engine::git_ops::GitAnswer::Yes
            )
        {
            return;
        }
        let entries = match remote::read_queue(&self.repo_root).await {
            Ok(entries) => entries,
            Err(error) => {
                self.wait_for_origin(error);
                return;
            }
        };
        let known = entries.iter().all(Result::is_ok);
        let advertised: HashMap<_, _> = entries
            .iter()
            .filter_map(|entry| {
                let member = entry.as_ref().ok()?;
                Some((member.change_id, member.remote.as_ref()?.oid.as_str()))
            })
            .collect();
        let advertisement_gone = |member: &LaneMember| {
            member.remote.as_ref().is_some_and(|source| {
                advertised.get(&member.change_id).copied() != Some(source.oid.as_str())
            })
        };
        if known {
            let consumed: Vec<_> = self
                .lane
                .waiting()
                .iter()
                .filter(|m| advertisement_gone(m))
                .cloned()
                .collect();
            if !consumed.is_empty() {
                self.multiple_landers = true;
            }
            for member in consumed {
                self.lane.unqueue(member.change_id);
                self.restore_promoted(member).await;
            }
            if let Some(batch) = self
                .lane
                .resolving()
                .cloned()
                .filter(|batch| batch.unlanded().any(advertisement_gone))
            {
                self.multiple_landers = true;
                let healed = self.heal_landed_members(&batch).await;
                if self
                    .lane
                    .resolving()
                    .is_some_and(ResolvingBatch::all_landed)
                {
                    let post = branch_head_sha(&self.repo_root, "main")
                        .await
                        .unwrap_or_default();
                    self.close_landed(&batch, batch.base_sha.clone(), post, None, healed)
                        .await;
                } else {
                    self.end_from_outside(batch.batch_id, "Another lander took a queue ref.", None)
                        .await;
                }
                // INVARIANT: Closing a batch may publish results for other entries from
                // this read. Intake again only after getting fresh refs.
                self.announce().await;
                if self.lane.resolving().is_none() {
                    self.engine.nudge_follow_main();
                }
                return;
            }
        }
        let mut arrivals = Vec::new();
        let mut promoted = HashSet::new();
        for entry in entries {
            let member = match entry {
                Ok(member) => member,
                Err(error) => {
                    self.wait_for_origin(error);
                    continue;
                }
            };
            if self.lane.attach_remote(&member) {
                if !self.lane.is_held(member.change_id) {
                    promoted.insert(member.change_id);
                }
                continue;
            }
            if self.lane.place(member.change_id) != Place::Absent {
                continue;
            }
            if self.lane.arrival_hold().is_some() {
                self.lane.hold(member);
            } else {
                arrivals.push(member);
            }
        }
        if (!arrivals.is_empty() || !promoted.is_empty()) && self.lane.arrival_hold().is_none() {
            let taken = self.lane.take_members(&promoted);
            let mut members = self.claim_members(taken, None).await;
            members.extend(self.claim_waiting(None).await);
            members.extend(self.claim_members(arrivals, None).await);
            self.run_pass(members).await;
        }
        self.announce().await;
    }

    async fn read_setting(&mut self) {
        if let Err(error) = self.read_setting_for_landing().await {
            log_legacy!("[LandingLane] {error}");
        }
    }

    async fn read_setting_for_landing(&mut self) -> Result<(), String> {
        let value = LANDING_SETTING
            .try_read(self.engine.pool())
            .await
            .map_err(|error| format!("Could not read the landing setting: {error}"))?;
        self.setting = LandingSetting::resolve(&value);
        Ok(())
    }

    fn publish(&self) {
        let mut snapshot = super::snapshot::snapshot_of(&self.lane, self.landing);
        snapshot.setting = self.setting;
        snapshot.multiple_landers = self.multiple_landers;
        snapshot
            .waiting_for_origin
            .clone_from(&self.waiting_for_origin);
        match self.engine.landing_lane_snapshot.write() {
            Ok(mut published) => *published = snapshot,
            Err(poisoned) => *poisoned.into_inner() = snapshot,
        }
    }

    async fn announce(&self) {
        self.publish();
        self.engine.broadcast_changes_updated().await;
    }

    async fn submit(&mut self, sub: Submission) -> LaneAnswer {
        let change_id = sub.change.id;
        match self.lane.place(change_id) {
            Place::Resolving => {
                // INVARIANT: The lane holds every member's claim, so this Apply got one
                // only because the lane lost it. The lane takes it back.
                let thread = sub.change_thread();
                let member = self
                    .lane
                    .resolving()
                    .and_then(|b| b.members().find(|m| m.change_id == change_id).cloned());
                if let Some(member) = member.filter(|member| member.remote.is_none()) {
                    self.held.entry(change_id).or_insert(PassMember {
                        member,
                        local: Some(LocalPassMember {
                            change: sub.change,
                            claim: sub.claim,
                        }),
                        actor: sub.actor,
                    });
                }
                Ok(queued(change_id, thread, RESOLVING_IN_BATCH))
            }
            Place::Waiting if self.lane.is_held(change_id) => {
                Ok(queued(change_id, sub.change_thread(), HELD_FOR_BATCH))
            }
            Place::Waiting if self.lane.queue_held() => {
                Ok(queued(change_id, sub.change_thread(), self.why_waiting()))
            }
            Place::Waiting => {
                if self
                    .lane
                    .waiting()
                    .iter()
                    .any(|m| m.change_id == change_id && m.remote.is_some())
                {
                    return Ok(queued(
                        change_id,
                        sub.change_thread(),
                        "Waiting for the lander",
                    ));
                }
                let snapshot_sha = self.snapshot_of(change_id, &sub.snapshot_sha);
                let mut members = self.claim_waiting(Some(change_id)).await;
                let thread = sub.change_thread();
                members.push(PassMember {
                    member: LaneMember {
                        change_id,
                        snapshot_sha,
                        remote: None,
                    },
                    local: Some(LocalPassMember {
                        change: sub.change,
                        claim: sub.claim,
                    }),
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
            remote: None,
        };
        if let Err(error) = self
            .engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id: event_thread,
                event: ThreadEvent::ChangeQueued {
                    change_id: change_id.to_string(),
                    snapshot_sha: member.snapshot_sha.clone(),
                },
                meta: EventMeta::with_actor(sub.actor.clone()),
            })
            .await
        {
            let reason = format!("Could not queue the change: {error}");
            self.engine
                .emit_apply_failed(event_thread, change_id, &reason, sub.actor)
                .await;
            return Err(reason);
        }
        if self.lane.arrival_hold().is_some() {
            self.lane.hold(member);
            self.actors.insert(change_id, sub.actor);
            self.announce().await;
            return Ok(queued(change_id, thread, HELD_FOR_BATCH));
        }
        let mut members = self.claim_waiting(None).await;
        members.push(PassMember {
            member,
            local: Some(LocalPassMember {
                change: sub.change,
                claim: sub.claim,
            }),
            actor: sub.actor,
        });
        self.pass_answer(members, change_id, thread).await
    }

    async fn pass_answer(
        &mut self,
        members: Vec<PassMember>,
        change_id: Uuid,
        thread: Option<Uuid>,
    ) -> LaneAnswer {
        let answer = self.run_pass(members).await.remove(&change_id);
        answer.unwrap_or_else(|| Ok(queued(change_id, thread, self.why_waiting())))
    }

    fn why_waiting(&self) -> &'static str {
        if self.lane.gathering() {
            GATHERING_FOR_PRESS
        } else {
            QUEUED_BEHIND_BATCH
        }
    }

    fn snapshot_of(&self, change_id: Uuid, fallback: &str) -> String {
        self.lane
            .waiting()
            .iter()
            .find(|m| m.change_id == change_id)
            .map_or_else(|| fallback.to_string(), |m| m.snapshot_sha.clone())
    }

    /// INVARIANT: Take the waiting members for a pass, claiming each. One whose claim
    /// another holds stays queued, and that claim's release runs the pass
    /// again ([`LaneMsg::ClaimReleased`]). One whose row could not be read
    /// stays queued for the next lane message. One no longer pending leaves with
    /// `ChangeUnqueued`, so no restart queues it again.
    /// `skip` is a member the caller already holds.
    async fn claim_waiting(&mut self, skip: Option<Uuid>) -> Vec<PassMember> {
        let taken = self.lane.take_pass();
        self.claim_members(taken, skip).await
    }

    async fn claim_members(
        &mut self,
        taken: Vec<LaneMember>,
        skip: Option<Uuid>,
    ) -> Vec<PassMember> {
        let mut members = Vec::new();
        for member in taken {
            if Some(member.change_id) == skip {
                continue;
            }
            if member.remote.is_some() {
                members.push(PassMember {
                    member,
                    local: None,
                    actor: None,
                });
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
                        local: Some(LocalPassMember { change, claim }),
                        actor,
                    });
                }
                Ok(change) => {
                    self.actors.remove(&member.change_id);
                    self.leave_resolved(member, change).await;
                }
                Err(e) => {
                    log_legacy!(
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

    async fn leave_resolved(&mut self, member: LaneMember, change: Option<Change>) {
        let Some(reason) = unqueue_reason_for(change.as_ref().map(Change::status)) else {
            return;
        };
        log_legacy!(
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

    fn push_after(&self, order: &LandingOrder) {
        if *order == LandingOrder::LocalFirst {
            push_main_in_background(&self.repo_root, &self.engine.event_bus);
        }
    }

    fn wait_for_origin(&mut self, why: String) {
        log_legacy!("[LandingLane] waiting for origin: {}", why);
        self.waiting_for_origin = Some(why);
    }

    async fn retry_origin(&mut self) {
        if self.waiting_for_origin.is_none() {
            return;
        }
        log_legacy!("[LandingLane] origin answered, trying the landing again");
        self.land_batch_waiting_for_origin();
        if self.lane.arrival_hold().is_some() {
            return;
        }
        let waiters = std::mem::take(&mut self.origin_waiters);
        let taken = if self.lane.queue_held() {
            self.lane.take_members(&waiters)
        } else {
            self.lane.take_pass()
        };
        let members = self.claim_members(taken, None).await;
        self.run_pass(members).await;
    }

    fn land_batch_waiting_for_origin(&mut self) {
        if let Some((batch_id, thread_id)) = self.batch_waiting_for_origin.take() {
            if self.lane.resolving_batch(batch_id).is_some() {
                self.landing = Some(batch_id);
                self.land_when_free(batch_id, thread_id);
            }
        }
    }

    async fn handoff(&mut self, member: PassMember) -> LaneAnswer {
        let id = member.member.change_id;
        let thread = member.change_thread();
        let outcome = self.publish_handoff(&member).await;
        let message = match outcome {
            Ok(Some(reason)) => {
                self.engine
                    .emit_apply_failed(member.thread(), id, &reason, member.actor)
                    .await;
                if let Some(local) = member.local {
                    self.spawn_settle_source(local.change, member.member.snapshot_sha, false);
                }
                return Err(reason);
            }
            Ok(None) => "Waiting for the lander".to_string(),
            Err(error) => {
                let reason = format!("Waiting for origin: {error}");
                self.wait_for_origin(reason.clone());
                reason
            }
        };
        self.actors.insert(id, member.actor);
        self.lane.wait(member.member);
        Ok(queued(id, thread, &message))
    }

    async fn publish_handoff(&self, member: &PassMember) -> Result<Option<String>, String> {
        let id = member.member.change_id;
        let snapshot = &member.member.snapshot_sha;
        let thread_id = member.thread();
        let started = handoff_started(self.engine.pool(), id)
            .await
            .map_err(|e| e.to_string())?;
        if let Some((oid, target, note)) = remote::read_result(&self.repo_root, id).await? {
            if started {
                if target != *snapshot {
                    return Err("The lander's result names a different snapshot.".into());
                }
                self.engine
                    .event_bus
                    .emit(BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::ChangeUnqueued {
                            change_id: id.to_string(),
                            reason: if note.removed {
                                UnqueueReason::Removed
                            } else {
                                UnqueueReason::LandingFailed
                            },
                        },
                        meta: EventMeta::default(),
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                if let Err(error) = remote::clear_result(&self.repo_root, id, &oid).await {
                    log_legacy!("[LandingLane] could not clear the consumed result: {error}");
                }
                return Ok(Some(note.reason));
            }
            remote::clear_result(&self.repo_root, id, &oid).await?;
        }
        if !started {
            self.engine
                .event_bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeHandoffStarted {
                        change_id: id.to_string(),
                    },
                    meta: EventMeta::with_actor(member.actor.clone()),
                })
                .await
                .map_err(|e| e.to_string())?;
        }
        let change = &member
            .local
            .as_ref()
            .ok_or("The source change is unavailable.")?
            .change;
        let note = QueueNote {
            title: member_title(change),
            summary: change
                .summary
                .clone()
                .unwrap_or_else(|| change.description.clone()),
            branch: change.branch_name.clone(),
            gateway: crate::landing_gateway::local_label(),
        };
        remote::push_queue(&self.repo_root, id, snapshot, note).await?;
        Ok(None)
    }

    async fn run_pass(&mut self, members: Vec<PassMember>) -> HashMap<Uuid, LaneAnswer> {
        let mut answers = HashMap::new();
        if members.is_empty() {
            return answers;
        }
        let engine = self.engine.clone();
        let gate = engine.landing_setting_gate.lock().await;
        if let Err(error) = self.read_setting_for_landing().await {
            self.wait_for_origin(error.clone());
            for member in members {
                let id = member.member.change_id;
                answers.insert(id, Ok(queued(id, member.change_thread(), &error)));
                self.actors.insert(id, member.actor);
                self.lane.wait(member.member);
            }
            self.announce().await;
            return answers;
        }
        if self.setting == LandingSetting::AnotherHost {
            drop(gate);
            self.waiting_for_origin = None;
            for member in members {
                let id = member.member.change_id;
                if member.member.remote.is_some() {
                    self.restore_promoted(member.member).await;
                } else {
                    answers.insert(id, self.handoff(member).await);
                }
            }
            self.announce().await;
            return answers;
        }
        let mut local = Vec::with_capacity(members.len());
        for member in members {
            let id = member.member.change_id;
            if member.member.remote.is_some() {
                local.push(member);
                continue;
            }
            match handoff_started(self.engine.pool(), id).await {
                Ok(false) => local.push(member),
                Ok(true) => {
                    answers.insert(id, self.handoff(member).await);
                }
                Err(error) => {
                    let why = format!("Could not read the change's landing hand-off: {error}");
                    answers.insert(id, Ok(queued(id, member.change_thread(), &why)));
                    self.lane.wait(member.member);
                }
            }
        }
        let members = local;
        if members.is_empty() {
            self.announce().await;
            return answers;
        }
        let inputs: Vec<LandingInput> = members.iter().map(landing_input).collect();
        let mut settle = Vec::new();
        let remote: Vec<_> = members
            .iter()
            .map(|m| &m.member)
            .filter(|m| m.remote.is_some())
            .collect();
        let order = if remote.is_empty() {
            crate::landing_gateway::landing_order().await
        } else {
            LandingOrder::OriginFirst
        };
        let published =
            publish_clean_pass(&remote, &MAIN_LOCKS, &self.repo_root, &order, &inputs).await;
        if published
            .as_ref()
            .is_ok_and(|pass| matches!(pass, CleanPass::Published(p) if p.moved_main()))
        {
            self.push_after(&order);
        }
        drop(gate);
        let publication = match published {
            Ok(CleanPass::Published(publication)) => publication,
            Ok(CleanPass::WaitingForOrigin(why)) => {
                self.wait_for_origin(why);
                for m in members {
                    let id = m.member.change_id;
                    answers.insert(id, Ok(queued(id, m.change_thread(), WAITING_FOR_ORIGIN)));
                    self.actors.insert(id, m.actor);
                    self.origin_waiters.insert(id);
                    self.lane.wait(m.member);
                }
                self.announce().await;
                return answers;
            }
            Err(e) => {
                for m in members {
                    let id = m.member.change_id;
                    settle.extend(m.settle(false));
                    answers.insert(id, Err(self.fail_member(m, &e).await));
                }
                self.settle_sources(settle);
                return answers;
            }
        };
        self.waiting_for_origin = None;
        if publication.moved_main()
            && publication.chain.steps.iter().any(|(id, step)| {
                matches!(step, LandingStep::Landed(_))
                    && members
                        .iter()
                        .any(|m| m.member.change_id == *id && m.member.remote.is_some())
            })
        {
            self.report_remote_landing(&publication.base, &publication.chain.tip)
                .await;
        }
        self.land_batch_waiting_for_origin();
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
            let source_settle = outcome.settles_source().and_then(|landed| m.settle(landed));
            let m_remote = m.member.remote.as_ref().map(|_| m.member.clone());
            let answer = match outcome {
                PassOutcome::Lands(merge) => self.announce_landing(m, Some(merge)).await,
                PassOutcome::AlreadyOnMain => self.announce_landing(m, None).await,
                PassOutcome::Fails(e) => Err(self.fail_member(m, e).await),
                PassOutcome::SetAside => {
                    set_aside.push(m);
                    continue;
                }
            };
            if let Some(member) = m_remote.filter(|_| answer.is_ok()) {
                self.restore_promoted(member).await;
            }
            if let Some(mut source) = source_settle {
                source.landed &= answer.is_ok();
                settle.push(source);
            }
            answers.insert(*change_id, answer);
        }
        self.take_in_set_aside(set_aside, publication.main_now(), &mut answers)
            .await;
        self.announce().await;
        self.settle_sources(settle);
        answers
    }

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
                answers.insert(id, Ok(queued(id, m.change_thread(), why)));
                self.actors.insert(id, m.actor);
            }
            return;
        };
        log_legacy!(
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
            log_legacy!("[LandingLane] could not create {} yet: {}", branch, e);
        }
        for m in set_aside {
            let id = m.member.change_id;
            answers.insert(id, Ok(queued(id, m.change_thread(), RESOLVING_IN_BATCH)));
            self.held.insert(id, m);
        }
        self.open_lane_thread(&batch);
    }

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
                log_legacy!("[LandingLane] plan marker for {}: {}", lane.branch, e);
            }
            let briefs: Vec<_> = briefs.iter().map(OwnedBrief::as_brief).collect();
            let prompt = build_lane_thread_prompt(&briefs);
            spawn_lane_turn(&engine, batch_id, thread_id, prompt, Some(title));
        });
    }

    async fn thread_ended(&mut self, batch_id: Uuid, thread_id: Uuid, end: LaneThreadEnd) {
        if self.lane.resolving_batch(batch_id).is_none() {
            self.cleanup_lane_worktree(batch_id, thread_id, None);
            return;
        }
        if self
            .remote_batch_failure
            .as_ref()
            .is_some_and(|failure| failure.batch_id == batch_id)
        {
            return;
        }
        match end {
            LaneThreadEnd::Completed => {
                self.landing = Some(batch_id);
                self.announce().await;
                self.land_when_free(batch_id, thread_id);
            }
            LaneThreadEnd::Stopped => {
                log_legacy!(
                    "[LandingLane] lane thread {} stopped; batch {} waits for its next turn",
                    thread_id,
                    batch_id
                );
                if self
                    .batch_waiting_for_origin
                    .is_some_and(|(id, _)| id == batch_id)
                {
                    self.batch_waiting_for_origin = None;
                }
                if self.landing == Some(batch_id) {
                    self.landing = None;
                }
                self.announce().await;
            }
            LaneThreadEnd::Failed(reason) => self.fail_batch(batch_id, reason, None, None).await,
        }
    }

    /// INVARIANT: Wait out the lane thread's teardown, then land under its hold. A run
    /// that started meanwhile is a new turn on the batch: its own end decides.
    fn land_when_free(&self, batch_id: Uuid, thread_id: Uuid) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            match engine.hold_thread_for_worktree_reset(thread_id).await {
                Ok(Some(hold)) => engine.notify_landing_lane(LaneMsg::Land {
                    batch_id,
                    hold: Box::new(hold),
                }),
                Ok(None) => log_legacy!(
                    "[LandingLane] a new run holds lane thread {}; its end lands batch {}",
                    thread_id,
                    batch_id
                ),
                Err(e) => log_legacy!(
                    "[LandingLane] could not hold lane thread {}: {}",
                    thread_id,
                    e
                ),
            }
        });
    }

    async fn land_batch(&mut self, batch_id: Uuid, hold: SpawnClaim) {
        if self.landing != Some(batch_id) {
            return;
        }
        let Some(batch) = self.lane.resolving_batch(batch_id).cloned() else {
            return;
        };
        let engine = self.engine.clone();
        let gate = engine.landing_setting_gate.lock().await;
        if let Err(error) = self.read_setting_for_landing().await {
            self.wait_for_origin(error);
            self.batch_waiting_for_origin = Some((batch_id, batch.lane_thread_id));
            self.landing = None;
            self.announce().await;
            return;
        }
        if self.setting == LandingSetting::AnotherHost {
            drop(gate);
            self.fail_batch(
                batch_id,
                "Landing moved to another host. Apply the changes again.".into(),
                None,
                Some(hold),
            )
            .await;
            return;
        }
        if let Err(reason) = self.hold_members(&batch).await {
            drop(gate);
            self.fail_batch(batch_id, reason, None, Some(hold)).await;
            return;
        }
        let lane = LaneWorktree::of(&self.workspace, batch_id);
        let unlanded: Vec<LaneMember> = batch.unlanded().cloned().collect();
        let merges = match verify_integration(&lane.path, &batch.base_sha, &unlanded).await {
            Ok(merges) => merges,
            Err(e) => {
                let reason = format!("The lane thread finished, but {e}.");
                drop(gate);
                self.fail_batch(batch_id, reason, None, Some(hold)).await;
                return;
            }
        };
        let order = crate::landing_gateway::landing_order().await;
        let certification =
            IntegrationCertification::load(self.engine.pool(), &self.repo_root, &lane.branch).await;
        let covered = self.engine.main_check_covers_landing().await;
        let remote: Vec<_> = unlanded.iter().filter(|m| m.remote.is_some()).collect();
        let order = if remote.is_empty() {
            order
        } else {
            LandingOrder::OriginFirst
        };
        let outcome = land_integration(
            &remote,
            &MAIN_LOCKS,
            &self.repo_root,
            &order,
            &lane.path,
            &certification,
            covered.then_some(CertificateCarry {
                bus: &self.engine.event_bus,
                batch_id,
                branch: &lane.branch,
                merges: &merges,
            }),
        )
        .await;
        if matches!(&outcome, Ok(IntegrationLanding::Landed { pre, post }) if pre != post) {
            self.push_after(&order);
        }
        drop(gate);
        match outcome {
            Ok(IntegrationLanding::NeedsHardening { caught_up }) => {
                drop(hold);
                self.landing = None;
                self.batch_waiting_for_origin = None;
                self.announce().await;
                if caught_up {
                    self.hold_arrivals(batch_id).await;
                    spawn_lane_turn(
                        &self.engine, batch_id, batch.lane_thread_id,
                        "Main advanced and merged cleanly into this integration branch. Run /harden on the updated head before finishing this landing.".into(),
                        None,
                    );
                } else if let Err(e) = self.engine.create_notification(
                    "Landing awaits hardening",
                    "The integration head has no current full hardening. Continue the landing thread and run /harden before finishing.",
                    None, Some(batch.lane_thread_id), None,
                    crate::scheduler::notifications::Tap::Modal, None,
                ).await {
                    log_legacy!("[LandingLane] could not announce missing hardening: {}", e);
                }
            }
            Ok(IntegrationLanding::Landed { pre, post }) => {
                if pre != post && !remote.is_empty() {
                    self.report_remote_landing(&pre, &post).await;
                }
                let mut landed = Vec::new();
                for (change_id, merge) in merges {
                    if let Some(m) = self.held.remove(&change_id) {
                        landed.extend(self.land_member(m, &merge).await);
                    }
                }
                self.waiting_for_origin = None;
                self.close_landed(&batch, pre, post, Some(hold), landed)
                    .await;
            }
            Ok(IntegrationLanding::WaitingForOrigin(why)) => {
                drop(hold);
                self.landing = None;
                self.batch_waiting_for_origin = Some((batch_id, batch.lane_thread_id));
                self.wait_for_origin(why);
                self.announce().await;
            }
            Ok(IntegrationLanding::CatchUpConflict { main_sha, commits }) => {
                log_legacy!(
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

    async fn hold_members(&mut self, batch: &ResolvingBatch) -> Result<(), String> {
        for member in batch.unlanded() {
            if self.held.contains_key(&member.change_id) {
                continue;
            }
            if member.remote.is_some() {
                self.held.insert(
                    member.change_id,
                    PassMember {
                        member: member.clone(),
                        local: None,
                        actor: None,
                    },
                );
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
                    local: Some(LocalPassMember { change, claim }),
                    actor: None,
                },
            );
        }
        Ok(())
    }

    async fn land_member(
        &mut self,
        m: PassMember,
        merge: &LandingMerge,
    ) -> Option<(Change, String)> {
        let settle = m
            .local
            .as_ref()
            .map(|local| (local.change.clone(), m.member.snapshot_sha.clone()));
        let remote = m.member.remote.as_ref().map(|_| m.member.clone());
        let change_id = m.member.change_id;
        if let Err(error) = self.announce_landing(m, Some(merge)).await {
            log_legacy!(
                "[LandingLane] Could not record merged change {}: {}",
                change_id,
                error
            );
            return None;
        }
        self.lane.landed(change_id);
        if let Some(member) = remote {
            self.restore_promoted(member).await;
        }
        settle
    }

    async fn close_landed(
        &mut self,
        batch: &ResolvingBatch,
        pre_sha: String,
        post_sha: String,
        hold: Option<SpawnClaim>,
        landed: Vec<(Change, String)>,
    ) {
        log_legacy!("[LandingLane] batch {} landed", batch.batch_id);
        self.landing = None;
        self.release_arrivals(batch.batch_id, LaneHoldRelease::BatchLanded)
            .await;
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
        self.cleanup_lane_worktree(batch.batch_id, batch.lane_thread_id, hold);
        self.announce().await;
        for (change, snapshot) in landed {
            self.spawn_settle_source(change, snapshot, true);
        }
        self.actors_retain();
        self.heal_landed_waiting().await;
        self.run_waiting().await;
    }

    async fn report_remote_landing(&self, from: &str, to: &str) {
        self.engine
            .event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::RemoteChangesLanded {
                    repo: self.repo_root.display().to_string(),
                    from_sha: from.into(),
                    to_sha: to.into(),
                }),
                "[LandingLane] RemoteChangesLanded",
            )
            .await;
        let engine = self.engine.clone();
        tokio::spawn(async move {
            if engine.automatic_source_builds_enabled().await {
                engine.automatic_background_rebuild().await;
            }
        });
    }

    async fn restore_promoted(&mut self, mut member: LaneMember) {
        if matches!(
            handoff_started(self.engine.pool(), member.change_id).await,
            Ok(true)
        ) {
            member.remote = None;
            self.lane.wait(member);
        }
    }

    async fn reject_remote(
        &self,
        members: &[&LaneMember],
        reason: &str,
        removed: bool,
    ) -> Result<(), String> {
        let _gate = self.engine.landing_setting_gate.lock().await;
        let setting = LANDING_SETTING
            .try_read(self.engine.pool())
            .await
            .map_err(|error| error.to_string())?;
        if LandingSetting::resolve(&setting) == LandingSetting::AnotherHost {
            if removed {
                return Err("Only the lander can remove this change.".into());
            }
            return Ok(());
        }
        remote::reject(&self.repo_root, members, reason, removed).await
    }

    async fn retry_remote_failure(&mut self) {
        if let Some(failure) = self.remote_batch_failure.clone() {
            self.fail_batch(failure.batch_id, failure.reason, failure.actor, None)
                .await;
        }
    }

    async fn fail_batch(
        &mut self,
        batch_id: Uuid,
        reason: String,
        actor: Option<MessageOrigin>,
        hold: Option<SpawnClaim>,
    ) {
        let Some(batch) = self.lane.resolving_batch(batch_id).cloned() else {
            return;
        };
        let remote: Vec<_> = batch.unlanded().filter(|m| m.remote.is_some()).collect();
        if !remote.is_empty() {
            if self.remote_batch_failure.is_none() {
                let event = BusEvent::System(SystemEvent::LaneBatchFailureRequested {
                    batch_id,
                    reason: reason.clone(),
                    actor: actor.clone(),
                });
                if let Err(error) = self.engine.event_bus.emit(event).await {
                    self.landing = None;
                    self.wait_for_origin(format!("Could not record the batch's failure: {error}"));
                    self.announce().await;
                    return;
                }
                self.remote_batch_failure = Some(BatchFailure {
                    batch_id,
                    reason: reason.clone(),
                    actor: actor.clone(),
                });
            }
            if let Err(error) = self.reject_remote(&remote, &reason, false).await {
                self.landing = None;
                self.batch_waiting_for_origin = None;
                self.wait_for_origin(error);
                self.announce().await;
                return;
            }
        }
        self.remote_batch_failure = None;
        self.release_arrivals(batch_id, LaneHoldRelease::BatchFailed)
            .await;
        let Some(batch) = self.lane.end_batch(batch_id) else {
            return;
        };
        self.landing = None;
        log_legacy!("[LandingLane] batch {} failed: {}", batch_id, reason);
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
            if member.remote.is_some() {
                self.held.remove(&member.change_id);
                self.restore_promoted(member.clone()).await;
            } else {
                self.unqueue_failed_member(member.change_id, &reason).await;
            }
        }
        self.cleanup_lane_worktree(batch_id, batch.lane_thread_id, hold);
        self.announce().await;
        self.actors_retain();
        self.run_waiting().await;
    }

    async fn unqueue_failed_member(&mut self, change_id: Uuid, reason: &str) {
        // INVARIANT: Kept until the member has left the lane: its claim goes with it.
        let held = self.held.remove(&change_id);
        let change = match &held {
            Some(m) => m.local.as_ref().map(|local| local.change.clone()),
            None => match self.engine.changes().get_by_id(change_id).await {
                Ok(change) => change,
                Err(e) => {
                    log_legacy!("[LandingLane] could not read {}: {}", change_id, e);
                    None
                }
            },
        };
        let thread = match &change {
            Some(change) => Some(change.thread_id.unwrap_or(change_id)),
            None => match super::queued_thread(self.engine.pool(), change_id).await {
                Ok(thread) => thread,
                Err(e) => {
                    log_legacy!(
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
            None => log_legacy!(
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

    /// INVARIANT: Remove a closed batch's worktree and branch once its lane thread holds
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
                        log_legacy!(
                            "[LandingLane] could not hold lane thread {}: {}",
                            thread_id,
                            e
                        );
                        None
                    }),
            };
            let Some(hold) = hold else {
                log_legacy!(
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
                .agent_host()
                .interrupt_agent(
                    &self.engine,
                    Some(batch.lane_thread_id),
                    actor,
                    crate::engine::agent_host::SettleTerminal::StuckProjection,
                )
                .await
            {
                log_legacy!(
                    "[LandingLane] stopping lane thread {}: {}",
                    batch.lane_thread_id,
                    e
                );
            }
        }
    }

    async fn announce_landing(
        &self,
        m: PassMember,
        merge: Option<&LandingMerge>,
    ) -> Result<ApplyResult, String> {
        let engine = &self.engine;
        let Some(local) = &m.local else {
            return Ok(ApplyResult::noop(
                m.member.change_id,
                None,
                0,
                "The change landed on origin.",
            ));
        };
        let change = &local.change;
        let thread = m.thread();
        let Some(merge) = merge else {
            engine
                .emit_change_applied(
                    &local.claim,
                    thread,
                    false,
                    false,
                    Vec::new(),
                    change.thread_title.clone(),
                    m.actor,
                    None,
                )
                .await?;
            return Ok(ApplyResult::noop(
                change.id,
                change.thread_id,
                change.files.len(),
                "Change already present on main, marked applied.",
            ));
        };
        let commits =
            commits_in_range(&self.repo_root, &merge.first_parent, &merge.merge_sha).await;
        let build_request = engine
            .emit_change_applied(
                &local.claim,
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
            .await?;
        engine
            .emit_entity_events_for_change_apply(
                &change.files,
                Some(&merge.first_parent),
                Some(&merge.merge_sha),
                m.actor.clone(),
                change.thread_id,
            )
            .await;
        Ok(ApplyResult::applied_with_merge(
            change.id,
            change.thread_id,
            change.requires_restart,
            merge.first_parent.clone(),
            merge.merge_sha.clone(),
            &commits,
            change.files.len(),
        )
        .with_version_build(build_request))
    }

    async fn fail_member(&mut self, m: PassMember, error: &str) -> String {
        if m.member.remote.is_some() {
            match self.reject_remote(&[&m.member], error, false).await {
                Ok(()) => self.restore_promoted(m.member).await,
                Err(error) => {
                    self.wait_for_origin(error);
                    self.origin_waiters.insert(m.member.change_id);
                    self.lane.wait(m.member);
                }
            }
            return error.into();
        }
        let message = format!("{LANDING_FAILED}: {error}. It is still pending; apply it again.");
        log_legacy!("[LandingLane] {}: {}", m.member.change_id, message);
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

    /// INVARIANT: Hold clean arrivals while batch `batch_id` hardens again after its
    /// catch-up, so no landing moves `main` under it before it lands. A hold
    /// already open keeps its arrivals and its cap.
    async fn hold_arrivals(&mut self, batch_id: Uuid) {
        if !self.lane.hold_arrivals(batch_id) {
            return;
        }
        log_legacy!(
            "[LandingLane] batch {} hardens again; clean arrivals wait for it",
            batch_id
        );
        self.engine
            .event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::LaneArrivalsHeld { batch_id }),
                "[LandingLane] LaneArrivalsHeld",
            )
            .await;
        self.arm_hold_cap(batch_id, ARRIVAL_HOLD_CAP);
        self.announce().await;
    }

    fn arm_hold_cap(&self, batch_id: Uuid, after: Duration) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            engine.notify_landing_lane(LaneMsg::HoldCapReached(batch_id));
        });
    }

    async fn release_arrivals(&mut self, batch_id: Uuid, cause: LaneHoldRelease) -> Vec<Uuid> {
        let Some(arrivals) = self.lane.release_arrivals(batch_id) else {
            return Vec::new();
        };
        log_legacy!(
            "[LandingLane] batch {} releases {} held arrival(s): {:?}",
            batch_id,
            arrivals.len(),
            cause
        );
        self.engine
            .event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::LaneArrivalsReleased {
                    batch_id,
                    cause,
                    arrivals: arrivals.clone(),
                }),
                "[LandingLane] LaneArrivalsReleased",
            )
            .await;
        arrivals
    }

    async fn hold_cap_reached(&mut self, batch_id: Uuid) {
        let mut passing: HashSet<Uuid> = self
            .release_arrivals(batch_id, LaneHoldRelease::CapReached)
            .await
            .into_iter()
            .collect();
        if passing.is_empty() {
            self.announce().await;
            return;
        }
        passing.extend(std::mem::take(&mut self.origin_waiters));
        let taken = self.lane.take_members(&passing);
        let members = self.claim_members(taken, None).await;
        self.note_cap_owed(&members, passing);
        self.run_pass(members).await;
        self.announce().await;
    }

    /// INVARIANT: Remember each id `claim_members` sent back to waiting because its
    /// claim was busy, so [`LaneMsg::ClaimReleased`] passes it.
    fn note_cap_owed(&mut self, members: &[PassMember], ids: impl IntoIterator<Item = Uuid>) {
        let waiting: HashSet<Uuid> = self.lane.waiting().iter().map(|m| m.change_id).collect();
        let claimed: HashSet<Uuid> = members.iter().map(|m| m.member.change_id).collect();
        self.cap_owed.extend(
            ids.into_iter()
                .filter(|id| !claimed.contains(id) && waiting.contains(id)),
        );
    }

    async fn run_waiting(&mut self) {
        if self.lane.queue_held() || self.lane.waiting().is_empty() {
            return;
        }
        self.cap_owed.clear();
        let members = self.claim_waiting(None).await;
        self.run_pass(members).await;
    }

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

    async fn remove(
        &mut self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), LaneRemoval> {
        self.lane.removal(change_id)?;
        if let Some(member) = self
            .lane
            .waiting()
            .iter()
            .find(|m| m.change_id == change_id && m.remote.is_some())
            .cloned()
        {
            self.reject_remote(&[&member], REMOVED, true)
                .await
                .map_err(|_| LaneRemoval::Unavailable)?;
            self.lane.unqueue(change_id);
            self.restore_promoted(member).await;
            self.announce().await;
            return Ok(());
        }
        if handoff_started(self.engine.pool(), change_id)
            .await
            .map_err(|_| LaneRemoval::Unavailable)?
        {
            return Err(LaneRemoval::HeldByLander);
        }
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

    /// INVARIANT: Take the restored lane. The driver handled nothing before this, so no
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
        self.remote_batch_failure = restore.remote_failure;
        if self.remote_batch_failure.is_some() {
            self.retry_remote_failure().await;
        } else if let Some(batch) = self.lane.resolving().cloned() {
            self.restore_batch(batch).await;
        }
        if let Some(hold) = self.lane.arrival_hold() {
            let held_for = Duration::from_secs(restore.hold_age_secs.unwrap_or(0));
            self.arm_hold_cap(hold.batch_id, ARRIVAL_HOLD_CAP.saturating_sub(held_for));
        }
        self.run_waiting().await;
    }

    async fn remove_closed_lane_worktrees(&self, opened_here: &HashSet<Uuid>) {
        let open = self.lane.resolving().map(|b| b.batch_id);
        match lane_worktrees(&self.repo_root, &self.workspace).await {
            Ok(lanes) => {
                for lane in closed_lanes_opened_here(&lanes, opened_here, open) {
                    retire_lane_worktree(self.engine.pool(), &self.repo_root, lane).await;
                }
            }
            Err(e) => log_legacy!("[LandingLane] could not list lane branches: {}", e),
        }
    }

    async fn cancel_members(
        &mut self,
        change_ids: &[Uuid],
        actor: Option<MessageOrigin>,
    ) -> Vec<Uuid> {
        let held = self.lane.held_of(change_ids);
        for &change_id in &held.waiting {
            if let Err(e) = self.remove(change_id, actor.clone()).await {
                log_legacy!(
                    "[LandingLane] {} did not leave the lane: {:?}",
                    change_id,
                    e
                );
            }
        }
        if let Some(batch_id) = held.batch {
            if let Err(e) = self.cancel(batch_id, actor).await {
                log_legacy!("[LandingLane] batch {} not canceled: {}", batch_id, e);
            }
        }
        held.members()
    }

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
                BatchRecovery::AwaitLaneThread => log_legacy!(
                    "[LandingLane] batch {} waits for lane thread {}",
                    batch_id,
                    batch.lane_thread_id
                ),
                BatchRecovery::Fail => {
                    self.fail_batch(batch_id, LANE_THREAD_CLOSED.into(), None, None)
                        .await;
                }
            },
            Err(e) => log_legacy!(
                "[LandingLane] could not read lane thread {}: {}; batch {} stays open",
                batch.lane_thread_id,
                e,
                batch_id
            ),
        }
    }

    async fn heal_landed_members(&mut self, batch: &ResolvingBatch) -> Vec<(Change, String)> {
        let mut healed = Vec::new();
        let unlanded: Vec<LaneMember> = batch.unlanded().cloned().collect();
        for member in unlanded {
            if let Some(source) = &member.remote {
                match remote::queue_oid(&self.repo_root, member.change_id).await {
                    Ok(oid) if oid.as_deref() != Some(source.oid.as_str()) => {}
                    _ => continue,
                }
            }
            let merge = match find_landing_merge(&self.repo_root, &member.snapshot_sha).await {
                Ok(Some(merge)) => merge,
                Ok(None) => continue,
                Err(e) => {
                    log_legacy!(
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
            log_legacy!(
                "[LandingLane] {} landed as {} before a restart, announcing it",
                member.change_id,
                merge.merge_sha
            );
            healed.extend(self.land_member(m, &merge).await);
        }
        healed
    }

    async fn heal_landed_waiting(&mut self) {
        let waiting = self.lane.waiting().to_vec();
        for member in waiting {
            if member.remote.is_some() {
                continue;
            }
            let merge = match find_landing_merge(&self.repo_root, &member.snapshot_sha).await {
                Ok(Some(merge)) => Some(merge),
                Ok(None) => {
                    if !matches!(
                        handoff_started(self.engine.pool(), member.change_id).await,
                        Ok(true)
                    ) || crate::engine::git_ops::git_answer(
                        &["merge-base", "--is-ancestor", &member.snapshot_sha, "main"],
                        &self.repo_root,
                    )
                    .await
                        != crate::engine::git_ops::GitAnswer::Yes
                    {
                        continue;
                    }
                    None
                }
                Err(e) => {
                    log_legacy!(
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
            if !change.is_pending() {
                self.lane.unqueue(member.change_id);
                continue;
            }
            log_legacy!(
                "[LandingLane] {} landed as {} before a restart, announcing it",
                member.change_id,
                merge
                    .as_ref()
                    .map_or("main", |merge| merge.merge_sha.as_str())
            );
            let snapshot = member.snapshot_sha.clone();
            let m = PassMember {
                member,
                local: Some(LocalPassMember {
                    change: change.clone(),
                    claim,
                }),
                actor: None,
            };
            match self.announce_landing(m, merge.as_ref()).await {
                Ok(_) => {
                    self.lane.unqueue(change.id);
                    self.actors.remove(&change.id);
                    self.spawn_settle_source(change, snapshot, true);
                }
                Err(error) => log_legacy!(
                    "[LandingLane] Could not record recovered landing: {}",
                    error
                ),
            }
        }
    }

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
    /// INVARIANT: What a source thread's worktree and work become once the lane is done
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
            return;
        }
        let finished =
            crate::engine::coding_turn_end::last_turn_ended_cleanly(self.pool(), thread_id).await;
        if let Err(e) = self
            .agent_host()
            .redecide_branch_work(self, thread_id, branch, &repo_root, None, finished)
            .await
        {
            log_legacy!(
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
                log_legacy!(
                    "[LandingLane] a run holds thread {}, leaving its worktree as it is",
                    thread_id
                );
                return;
            }
            Err(e) => {
                log_legacy!("[LandingLane] could not hold thread {}: {}", thread_id, e);
                return;
            }
        };
        // INVARIANT: Asked again under the hold: a commit may have landed meanwhile.
        if branch_head_sha(repo_root, branch).await.as_deref() != Some(snapshot) {
            return;
        }
        if let Err(e) =
            crate::engine::change_ops::reset_worktree_to_main_after_apply(&worktree).await
        {
            log_legacy!("[LandingLane] kept worktree {}: {}", worktree.display(), e);
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
    fn thread(&self) -> Uuid {
        self.change_thread().unwrap_or(self.member.change_id)
    }

    fn change_thread(&self) -> Option<Uuid> {
        self.local.as_ref().and_then(|local| local.change.thread_id)
    }

    fn settle(&self, landed: bool) -> Option<SourceSettle> {
        self.local.as_ref().map(|local| SourceSettle {
            change: local.change.clone(),
            snapshot: self.member.snapshot_sha.clone(),
            landed,
        })
    }
}

struct OwnedBrief {
    title: String,
    summary: String,
    branch: String,
    snapshot_sha: String,
}

impl OwnedBrief {
    fn of(m: &PassMember) -> Self {
        let (title, summary, branch) = if let Some(remote) = &m.member.remote {
            (
                remote.title.clone(),
                remote.summary.clone(),
                remote.branch.clone(),
            )
        } else {
            let change = &m
                .local
                .as_ref()
                .expect("local lane member has a change")
                .change;
            (
                member_title(change),
                change.description.clone(),
                change.branch_name.clone(),
            )
        };
        Self {
            title,
            summary,
            branch,
            snapshot_sha: m.member.snapshot_sha.clone(),
        }
    }

    fn as_brief(&self) -> crate::engine::merge_prompts::LaneMemberBrief<'_> {
        crate::engine::merge_prompts::LaneMemberBrief {
            title: &self.title,
            summary: &self.summary,
            branch: &self.branch,
            snapshot_sha: &self.snapshot_sha,
        }
    }
}

pub(super) fn lane_event_types() -> Vec<&'static str> {
    LANE_THREAD_CLOSING_EVENT_TYPES
        .iter()
        .copied()
        .chain(["PreferencesChanged", "MainFollowed"])
        .collect()
}

pub(super) async fn forward_lane_events(
    received: Result<EmittedEvent, RecvError>,
    replay: &mut LagReplay,
    pool: &sqlx::PgPool,
    notify: impl Fn(LaneMsg),
) -> bool {
    let Some(frames) = replay.read(received, pool).await else {
        return false;
    };
    for frame in frames {
        if matches!(&frame.typed, BusEvent::System(SystemEvent::PreferencesChanged { key, .. }) if key == LANDING_SETTING.key())
        {
            notify(LaneMsg::SettingChanged);
        }
        if let BusEvent::System(SystemEvent::MainFollowed { repo, .. }) = &frame.typed {
            notify(LaneMsg::MainFollowed(repo.clone()));
        }
        for (thread_id, actor) in lane_thread_closures(&frame.typed) {
            notify(LaneMsg::ThreadClosed { thread_id, actor });
        }
    }
    true
}

/// INVARIANT: Announce cancellation before admitting any recovered lane work. The floor
/// also excludes historical worktree ownership on every later boot. Emitting
/// through the bus (rather than deleting rows) makes a crash retry observable.
pub(crate) async fn settle_restored_lane(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(boundary) = crate::core::snapshot::boundary::read(pool).await? else {
        return Ok(());
    };
    let batches: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT (payload->'data'->>'batch_id')::uuid FROM events start \
         WHERE event_type = 'LaneBatchStarted' AND sequence <= $1 \
         AND NOT EXISTS (SELECT 1 FROM events ended WHERE ended.event_type IN ('LaneBatchFailed', 'LaneBatchLanded') \
           AND ended.payload->'data'->>'batch_id' = start.payload->'data'->>'batch_id')")
        .bind(boundary.event_sequence).fetch_all(pool).await?;
    for batch_id in batches {
        bus.emit(BusEvent::System(SystemEvent::LaneBatchFailed {
            batch_id,
            reason: "Workspace restored from snapshot; apply these changes again.".into(),
            actor: None,
        }))
        .await?;
    }
    // INVARIANT: Latest queue authority matters: a fresh Apply after restoration must
    // survive, including one for a change that appeared in the archive.
    let queued: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT thread_id, payload->>'change_id' FROM events queued \
         WHERE event_type = 'ChangeQueued' AND sequence <= $1 \
         AND NOT EXISTS (SELECT 1 FROM events later WHERE later.sequence > queued.sequence \
           AND later.event_type IN ('ChangeQueued', 'ChangeUnqueued', 'ChangeApplied') \
           AND later.payload->>'change_id' = queued.payload->>'change_id') \
         ORDER BY sequence",
    )
    .bind(boundary.event_sequence)
    .fetch_all(pool)
    .await?;
    for (thread_id, change_id) in queued {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ChangeUnqueued {
                change_id,
                reason: UnqueueReason::Removed,
            },
            meta: EventMeta::NONE,
        })
        .await?;
    }
    Ok(())
}

pub(super) async fn load_lane_records(pool: &sqlx::PgPool) -> Result<Vec<LaneRecord>, sqlx::Error> {
    let floor = crate::core::snapshot::boundary::read(pool)
        .await?
        .map_or(0, |b| b.event_sequence);
    let rows: Vec<(String, serde_json::Value, i64)> = sqlx::query_as(
        "SELECT event_type, payload, EXTRACT(EPOCH FROM now() - created)::bigint FROM events \
         WHERE sequence > $1 AND (event_type IN ('ChangeQueued', 'ChangeUnqueued', 'LaneBatchStarted', \
                              'LaneBatchLanded', 'LaneBatchFailed', 'LaneBatchFailureRequested', 'LaneArrivalsHeld', \
                              'LaneArrivalsReleased') \
            OR (event_type = 'ChangeApplied' \
                AND payload->>'change_id' IN \
                    (SELECT payload->>'change_id' FROM events WHERE event_type = 'ChangeQueued'))) \
         ORDER BY sequence",
    )
    .bind(floor)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .filter_map(|(event_type, payload, age_secs)| lane_record(event_type, payload, *age_secs))
        .collect())
}

fn lane_record(event_type: &str, payload: &serde_json::Value, age_secs: i64) -> Option<LaneRecord> {
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
            remote: None,
        }),
        "ChangeUnqueued" => LaneRecord::Unqueued(change_id()?),
        "ChangeApplied" => LaneRecord::Applied(change_id()?),
        "LaneBatchStarted" => LaneRecord::BatchStarted {
            batch_id: data_uuid("batch_id")?,
            base_sha: data?.get("base_sha")?.as_str()?.to_string(),
            lane_thread_id: data_uuid("lane_thread_id")?,
            members: serde_json::from_value(data?.get("members")?.clone()).ok()?,
        },
        "LaneBatchFailureRequested" => LaneRecord::BatchFailureRequested(BatchFailure {
            batch_id: data_uuid("batch_id")?,
            reason: data?.get("reason")?.as_str()?.to_string(),
            actor: data?
                .get("actor")
                .and_then(|actor| serde_json::from_value(actor.clone()).ok()),
        }),
        "LaneBatchLanded" | "LaneBatchFailed" => LaneRecord::BatchEnded(data_uuid("batch_id")?),
        "LaneArrivalsHeld" => LaneRecord::ArrivalsHeld {
            batch_id: data_uuid("batch_id")?,
            age_secs: u64::try_from(age_secs).unwrap_or(0),
        },
        "LaneArrivalsReleased" => LaneRecord::ArrivalsReleased(data_uuid("batch_id")?),
        _ => return None,
    })
}

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

/// INVARIANT: Send the lane thread a turn. A turn that fails before its run answers
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
    engine.agent_host().spawn_engine_coding_turn_or_else(
        engine,
        thread_id,
        prompt,
        title,
        lane_turn_reason(),
        Box::new(move |e| {
            reporter.report_lane_thread_end(
                batch_id,
                thread_id,
                LaneThreadEnd::Failed(format!("The lane thread's turn could not start: {e}")),
            );
        }),
    );
}

pub(super) fn unqueue_reason_for(status: Option<ChangeStatus>) -> Option<UnqueueReason> {
    match status {
        Some(ChangeStatus::Pending | ChangeStatus::Applied | ChangeStatus::Reverted) => None,
        Some(ChangeStatus::SetAside) => Some(UnqueueReason::SetAside),
        Some(ChangeStatus::Discarded | ChangeStatus::Withdrawn) | None => {
            Some(UnqueueReason::Discarded)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PassOutcome<'a> {
    Lands(&'a LandingMerge),
    AlreadyOnMain,
    Fails(&'a str),
    SetAside,
}

impl<'a> PassOutcome<'a> {
    pub(super) fn of(step: &'a LandingStep, failed: Option<&'a str>) -> Self {
        match (step, failed) {
            (LandingStep::Landed(merge), None) => Self::Lands(merge),
            (LandingStep::Landed(_), Some(e)) => Self::Fails(e),
            (LandingStep::AlreadyOnMain(Some(merge)), _) => Self::Lands(merge),
            (LandingStep::AlreadyOnMain(None), _) => Self::AlreadyOnMain,
            (LandingStep::SetAside, _) => Self::SetAside,
        }
    }

    pub(super) fn settles_source(&self) -> Option<bool> {
        match self {
            Self::Lands(_) | Self::AlreadyOnMain => Some(true),
            Self::Fails(_) => Some(false),
            Self::SetAside => None,
        }
    }
}

struct SourceSettle {
    change: Change,
    snapshot: String,
    landed: bool,
}

fn landing_input(m: &PassMember) -> LandingInput {
    let brief = OwnedBrief::of(m);
    LandingInput {
        change_id: m.member.change_id,
        snapshot_sha: m.member.snapshot_sha.clone(),
        subject: format!("Land: {}", brief.title),
        body: format!("Change: {}\nBranch: {}", m.member.change_id, brief.branch),
    }
}

fn queued(change_id: Uuid, thread_id: Option<Uuid>, message: &str) -> ApplyResult {
    ApplyResult::queued(change_id, thread_id, message)
}
