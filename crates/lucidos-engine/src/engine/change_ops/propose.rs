use super::*;
use crate::engine::event_bus::{Asked, EventBus, HookRefusal};
use crate::engine::thread_events::{EventMeta, ThreadEvent};

#[derive(Debug, Default, Clone, Copy)]
pub struct HoldReentry {
    pub(crate) plan: bool,
    pub(crate) hardening: bool,
}

impl HoldReentry {
    pub fn contains(self, hold: ProposalHold) -> bool {
        match hold {
            ProposalHold::Plan(PlanHold::PlanOnlyAwaitingApproval)
            | ProposalHold::IntegratedIntoParent
            | ProposalHold::CarriedBySuccessor => false,
            ProposalHold::Plan(_) => self.plan,
            ProposalHold::HardeningMissing => self.hardening,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProposeOutcome {
    Proposed(Uuid),
    Held(ProposalHold),
    Unfinished,
    AwaitingLanding(Uuid),
    Refused(HookRefusal),
}

enum Proposal {
    Unchanged(Uuid),
    Record(Box<ProposalRecord>),
}

struct ProposalRecord {
    change_id: Uuid,
    brought_back: Option<(ThreadEvent, EventMeta)>,
    reproposed: bool,
    event: ThreadEvent,
    meta: EventMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenChangeAfterHold {
    Withdrawn,
    StillCarries(Uuid),
    /// INVARIANT: The branch had none, or it could not be read.
    None,
}

async fn withhold_work(
    bus: &EventBus,
    input: &ProposeChangeInput<'_>,
    branch_moved: bool,
    reason: Option<UnproposedReason>,
    agent_returns: bool,
) -> Result<OpenChangeAfterHold, Box<dyn std::error::Error + Send + Sync>> {
    let open = match bus
        .changes_projection()
        .get_open_by_branch(input.branch_name)
        .await
    {
        Ok(Some(open))
            if branch_moved
                || open.description != input.description
                || open.files != input.files =>
        {
            emit_change_withdrawn(bus, input.thread_id, open.id, input.origin.clone()).await?;
            OpenChangeAfterHold::Withdrawn
        }
        Ok(Some(open)) => OpenChangeAfterHold::StillCarries(open.id),
        Ok(None) => OpenChangeAfterHold::None,
        Err(e) => {
            log_legacy!(
                "[Changes] get_open_by_branch({}): {}; leaving any open change in place",
                input.branch_name,
                e
            );
            OpenChangeAfterHold::None
        }
    };
    if let Some(reason) = reason.filter(|_| !matches!(open, OpenChangeAfterHold::StillCarries(_))) {
        emit_proposal_withheld(
            bus,
            input.thread_id,
            input.branch_name,
            input.files,
            reason,
            agent_returns,
        )
        .await;
    }
    Ok(open)
}

pub(crate) async fn withhold_unfinished_work(
    bus: &EventBus,
    input: &ProposeChangeInput<'_>,
    branch_moved: bool,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let reason = Some(UnproposedReason::TurnIncomplete);
    let open = withhold_work(bus, input, branch_moved, reason, false).await?;
    Ok(open == OpenChangeAfterHold::Withdrawn)
}

/// INVARIANT: Withdraw the open change of a turn that handed off, unannounced, and say
/// whether it withdrew one. The successor session carries the work, and its own
/// turn end decides it. So a pending change goes back to unproposed work even
/// when it still carries everything: nothing may offer Apply before that
/// session. A set-aside change stays set aside unless the branch moved past it,
/// and a queued one stays queued: the landing lane lands the snapshot it froze.
pub async fn withhold_handed_off_work(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    repo_root: &Path,
    branch_name: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let open = bus
        .changes_projection()
        .get_open_by_branch(branch_name)
        .await
        .map_err(|e| format!("get_open_by_branch({branch_name}): {e}"))?;
    let Some(open) = open else {
        return Ok(false);
    };
    let queued = crate::engine::landing_lane::queued_snapshot(pool, thread_id, open.id)
        .await
        .map_err(|e| format!("could not read the landing lane for this thread: {e}"))?;
    if queued.is_some() {
        return Ok(false);
    }
    if open.status() == ChangeStatus::SetAside
        && !branch_moved_since_last_idle(pool, thread_id, repo_root, branch_name).await
    {
        return Ok(false);
    }
    emit_change_withdrawn(bus, thread_id, open.id, None).await?;
    Ok(true)
}

/// INVARIANT: Whether the branch head moved since the thread's last idle recorded it. An
/// open change covers the head of the idle that last proposed it. Engine idles
/// that record no head are skipped. An unreadable head on either side counts
/// as moved: that direction never lets Apply merge a stopped turn's commits.
pub(crate) async fn branch_moved_since_last_idle(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    let recorded: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'worktree_head_sha' FROM events \
         WHERE thread_id = $1 AND event_type IN ('CodingAgentIdled', 'ClaudeCodeIdled') \
           AND COALESCE(payload->>'worktree_head_sha', '') <> '' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        log_legacy!("[Changes] last idle head for {}: {}", thread_id, e);
        None
    })
    .flatten();
    let current = crate::engine::git_ops::branch_head_sha(repo_root, branch_name).await;
    match (recorded, current) {
        (Some(recorded), Some(current)) => recorded != current,
        _ => true,
    }
}

pub(crate) async fn withhold_for_landing(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    input: &ProposeChangeInput<'_>,
) -> Result<Option<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    let queued = crate::engine::landing_lane::queued_changes(pool, input.thread_id)
        .await
        .map_err(|e| format!("could not read the landing lane for this thread: {e}"))?;
    let Some(member) = queued.into_iter().next() else {
        return Ok(None);
    };
    let head =
        crate::engine::git_ops::branch_head_sha(Path::new(input.repo_root), input.branch_name)
            .await;
    if head.as_deref() != Some(member.snapshot_sha.as_str()) {
        emit_proposal_withheld(
            bus,
            input.thread_id,
            input.branch_name,
            input.files,
            UnproposedReason::AwaitingLanding,
            false,
        )
        .await;
    }
    Ok(Some(member.change_id))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoldOutcome {
    Clear,
    Held(ProposalHold, OpenChangeAfterHold),
}

pub(crate) async fn withhold_unready(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    input: &ProposeChangeInput<'_>,
    reentry: HoldReentry,
) -> Result<HoldOutcome, Box<dyn std::error::Error + Send + Sync>> {
    let repo_root = Path::new(input.repo_root);
    let Some(hold) = read_proposal_hold(
        pool,
        bus,
        input.thread_id,
        repo_root,
        input.branch_name,
        input.files,
    )
    .await
    else {
        return Ok(HoldOutcome::Clear);
    };
    log_legacy!(
        "[Changes] Not proposing branch {} for thread {}: the proposal hold keeps it ({:?})",
        input.branch_name,
        input.thread_id,
        hold
    );
    let moved =
        branch_moved_since_last_idle(pool, input.thread_id, repo_root, input.branch_name).await;
    let open = withhold_work(bus, input, moved, Some(hold.into()), reentry.contains(hold)).await?;
    Ok(HoldOutcome::Held(hold, open))
}

/// INVARIANT: Send a pending or set-aside change back to unproposed work. Its branch
/// keeps every commit.
///
/// A failed emit is an error: the change would stay applicable over work it
/// no longer covers, so no caller may carry on as if it was withdrawn.
pub async fn emit_change_withdrawn(
    bus: &EventBus,
    thread_id: Uuid,
    change_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::engine::thread_events::{EventMeta, ThreadEvent};
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeWithdrawn {
            change_id: change_id.to_string(),
        },
        meta: EventMeta::with_actor(actor),
    })
    .await
    .map_err(|e| format!("could not withdraw change {change_id}: {e}"))?;
    Ok(())
}

pub async fn emit_proposal_withheld(
    bus: &EventBus,
    thread_id: Uuid,
    branch_name: &str,
    files: &[String],
    reason: UnproposedReason,
    agent_returns: bool,
) {
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ProposalWithheld {
                agent_returns,
                branch_name: branch_name.to_string(),
                files: files.to_vec(),
                reason,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        },
        "[Changes] ProposalWithheld",
    )
    .await;
}

pub(crate) use crate::core::git::LANE_BRANCH_IS_NOT_A_CHANGE;

fn refuse_lane_branch(
    input: &ProposeChangeInput<'_>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if crate::engine::git_ops::is_lane_branch(input.branch_name) {
        return Err(LANE_BRANCH_IS_NOT_A_CHANGE.into());
    }
    Ok(())
}

impl LucidosEngine {
    pub async fn propose_turn_work(
        &self,
        input: ProposeChangeInput<'_>,
        finished: bool,
        reentry: HoldReentry,
    ) -> Result<ProposeOutcome, Box<dyn std::error::Error + Send + Sync>> {
        refuse_lane_branch(&input)?;
        if finished {
            return self.propose_change(input, reentry).await;
        }
        if let Some(queued) = withhold_for_landing(&self.pool, &self.event_bus, &input).await? {
            return Ok(ProposeOutcome::AwaitingLanding(queued));
        }
        let moved = branch_moved_since_last_idle(
            &self.pool,
            input.thread_id,
            Path::new(input.repo_root),
            input.branch_name,
        )
        .await;
        if withhold_unfinished_work(&self.event_bus, &input, moved).await? {
            self.broadcast_changes_updated().await;
        }
        Ok(ProposeOutcome::Unfinished)
    }

    pub async fn propose_change(
        &self,
        input: ProposeChangeInput<'_>,
        reentry: HoldReentry,
    ) -> Result<ProposeOutcome, Box<dyn std::error::Error + Send + Sync>> {
        refuse_lane_branch(&input)?;
        if let Some(queued) = withhold_for_landing(&self.pool, &self.event_bus, &input).await? {
            return Ok(ProposeOutcome::AwaitingLanding(queued));
        }
        match withhold_unready(&self.pool, &self.event_bus, &input, reentry).await? {
            HoldOutcome::Clear => {}
            HoldOutcome::Held(_, OpenChangeAfterHold::StillCarries(id)) => {
                return Ok(ProposeOutcome::Proposed(id));
            }
            HoldOutcome::Held(hold, open) => {
                if open == OpenChangeAfterHold::Withdrawn {
                    self.broadcast_changes_updated().await;
                }
                return Ok(ProposeOutcome::Held(hold));
            }
        }

        let proposal = self.prepare_proposal(&input).await?;
        let asked = match &proposal {
            Proposal::Unchanged(_) => None,
            Proposal::Record(record) => match self
                .event_bus
                .ask_hooks(input.thread_id, &record.event, &record.meta)
                .await
            {
                Ok(asked) => Some(asked),
                Err(refusal) => return self.withhold_refused_proposal(&input, refusal).await,
            },
        };

        // INVARIANT: Reconcile the "a coding-agent thread has at most one open change"
        // invariant BEFORE proposing on this branch: discard any open change
        // the thread still holds on a DIFFERENT branch (e.g. a merge-conflict
        // recovery re-ran on a fresh branch). Doing this *before* the
        // `ChangeProposed` emit below is load-bearing: `ChangeDiscarded` clears
        // the branch work, so discarding after the propose would wipe the
        // state this proposal is about to set. Same-branch
        // multi-change is preserved (keep = same branch). See
        // docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md.
        self.discard_open_changes_for_thread_except(input.thread_id, input.origin.clone(), |c| {
            c.branch_name == input.branch_name
        })
        .await;

        let change_id = match proposal {
            Proposal::Unchanged(change_id) => change_id,
            Proposal::Record(record) => self.record_proposal(input.thread_id, *record, asked).await,
        };
        Ok(ProposeOutcome::Proposed(change_id))
    }

    async fn withhold_refused_proposal(
        &self,
        input: &ProposeChangeInput<'_>,
        refusal: HookRefusal,
    ) -> Result<ProposeOutcome, Box<dyn std::error::Error + Send + Sync>> {
        log_legacy!(
            "[Changes] Not proposing branch {} for thread {}: {}",
            input.branch_name,
            input.thread_id,
            refusal
        );
        let moved = branch_moved_since_last_idle(
            &self.pool,
            input.thread_id,
            Path::new(input.repo_root),
            input.branch_name,
        )
        .await;
        let reason = Some(UnproposedReason::HookRefused);
        if withhold_work(&self.event_bus, input, moved, reason, false).await?
            == OpenChangeAfterHold::Withdrawn
        {
            self.broadcast_changes_updated().await;
        }
        Ok(ProposeOutcome::Refused(refusal))
    }

    async fn emit_change_correction(
        &self,
        input: ProposeChangeInput<'_>,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        Ok(match self.prepare_proposal(&input).await? {
            Proposal::Unchanged(change_id) => change_id,
            Proposal::Record(record) => self.record_proposal(input.thread_id, *record, None).await,
        })
    }

    async fn prepare_proposal(
        &self,
        input: &ProposeChangeInput<'_>,
    ) -> Result<Proposal, Box<dyn std::error::Error + Send + Sync>> {
        let ProposeChangeInput {
            thread_id: _,
            branch_name,
            repo_root,
            description,
            files,
            requires_restart,
            channel,
            hardened,
            ref origin,
        } = *input;

        let existing = self
            .changes()
            .get_open_by_branch(branch_name)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("get_open_by_branch({}): {}", branch_name, e).into()
            })?;
        let change_id = existing.as_ref().map(|c| c.id).unwrap_or_else(Uuid::new_v4);
        let new_work = existing
            .as_ref()
            .is_none_or(|e| e.description != description || e.files != files);
        let needs_emit = existing.as_ref().is_none_or(|e| {
            e.description != description
                || e.files != files
                || e.requires_restart != requires_restart
                || e.hardened != hardened
                || e.incomplete
        });

        let set_aside = existing
            .as_ref()
            .is_some_and(|e| e.status() == ChangeStatus::SetAside);
        if !needs_emit || (set_aside && !new_work) {
            return Ok(Proposal::Unchanged(change_id));
        }
        Ok(Proposal::Record(Box::new(ProposalRecord {
            change_id,
            brought_back: set_aside.then(|| {
                (
                    ThreadEvent::ChangeBroughtBack {
                        change_id: change_id.to_string(),
                    },
                    EventMeta::with_actor(origin.clone()),
                )
            }),
            reproposed: existing.is_some(),
            event: ThreadEvent::ChangeProposed {
                content: crate::engine::thread_events::ChangeContent::from_files(files),
                change_id: change_id.to_string(),
                description: Some(description.to_string()),
                files: files.to_vec(),
                requires_restart,
                origin: origin.clone(),
                commit_sha: None,
                branch_name: branch_name.to_string(),
                repo_root: repo_root.to_string(),
                hardened,
                incomplete: false,
                set_aside: false,
                path: String::new(),
                diff: String::new(),
            },
            meta: EventMeta {
                channel: Some(channel),
                ..EventMeta::NONE
            },
        })))
    }

    async fn record_proposal(
        &self,
        thread_id: Uuid,
        record: ProposalRecord,
        asked: Option<Asked>,
    ) -> Uuid {
        use crate::engine::event_bus::BusEvent;
        if let Some((event, meta)) = record.brought_back {
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id,
                        event,
                        meta,
                    },
                    "[Changes] ChangeBroughtBack (new work)",
                )
                .await;
        }
        let (event, meta) = (record.event, record.meta);
        let recorded = match asked {
            Some(asked) => {
                self.event_bus
                    .emit_asked(asked, thread_id, event, meta)
                    .await
            }
            None => {
                self.event_bus
                    .emit(BusEvent::Thread {
                        thread_id,
                        event,
                        meta,
                    })
                    .await
            }
        };
        if let Err(e) = recorded {
            log_legacy!(
                "[Changes] ChangeProposed emit failed for {}: {}",
                thread_id,
                e
            );
        }
        if record.reproposed {
            self.broadcast_changes_updated().await;
        }
        record.change_id
    }

    pub async fn branch_has_open_change(&self, branch_name: &str) -> bool {
        match self.changes().get_open_by_branch(branch_name).await {
            Ok(row) => row.is_some(),
            Err(e) => {
                log_legacy!(
                    "[Changes] get_open_by_branch({}): {}; treating it as having no open change",
                    branch_name,
                    e
                );
                false
            }
        }
    }

    pub async fn reconcile_emptied_pending_change(
        &self,
        thread_id: Uuid,
        repo_root: &Path,
        branch_name: &str,
    ) {
        let existing = match self.changes().get_pending_by_branch(branch_name).await {
            Ok(row) => row,
            Err(e) => {
                log_legacy!(
                    "[Changes] reconcile: get_pending_by_branch({}): {} — leaving the row alone",
                    branch_name,
                    e
                );
                return;
            }
        };
        let diff =
            crate::engine::git_ops::branch_changed_files_checked(repo_root, branch_name).await;
        if let (Some(row), Err(e)) = (existing.as_ref(), diff.as_ref()) {
            log_legacy!(
                "[Changes] reconcile: {} — leaving change {} at {} file(s)",
                e,
                row.id,
                row.file_count
            );
        }
        let Some(existing) = existing else { return };
        if !should_reconcile_emptied_change(
            (existing.file_count, existing.requires_restart),
            diff.as_deref().map_err(String::as_str),
        ) {
            return;
        }

        let hardened =
            crate::engine::git_ops::has_full_harden_marker(&self.pool, repo_root, branch_name)
                .await;
        let fallback = crate::engine::session_queries::change_description_fallback(
            &self.pool,
            thread_id,
            branch_name,
        )
        .await;
        let base = crate::engine::git_ops::default_local_branch(repo_root).await;
        let log_range = format!("{}..{}", base, branch_name);
        let description =
            crate::engine::git_ops::describe_branch_changes(repo_root, &log_range, &fallback, None)
                .await;

        log_legacy!(
            "[Changes] Branch {} has no diff left — change {} reconciled to 0 files (was {})",
            branch_name,
            existing.id,
            existing.file_count
        );
        if let Err(e) = self
            .emit_change_correction(ProposeChangeInput {
                thread_id,
                branch_name,
                repo_root: &repo_root.to_string_lossy(),
                description: &description,
                files: &[],
                requires_restart: false,
                channel: crate::engine::thread_events::EventChannel::ClaudeCode,
                hardened,
                origin: None,
            })
            .await
        {
            log_legacy!(
                "[Changes] Failed to reconcile emptied change {}: {}",
                existing.id,
                e
            );
        }
    }

    pub async fn request_hardening_in_session(
        &self,
        thread_id: Uuid,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    ) -> Result<
        tokio::sync::oneshot::Receiver<crate::engine::InputAnswered>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        self.emit_missing_hardening_detected(thread_id).await;
        crate::engine::coding_turn_end::send_awaited_prompt(
            msg_tx,
            crate::engine::continuation_reason::AUTO_HARDEN_MESSAGE.to_string(),
        )
    }

    pub async fn harden_held_work(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !crate::engine::thread_stop::thread_is_stopped(self.pool(), thread_id).await {
            if let Some(session) = self.live_session_info(thread_id).await {
                return self
                    .nudge_held_proposal_in_session(
                        thread_id,
                        ProposalHold::HardeningMissing,
                        &session.msg_tx,
                    )
                    .await;
            }
        }
        let requested = crate::engine::thread_events::emit_continuation_requested_or_log(
            &self.event_bus,
            thread_id,
            crate::engine::continuation_reason::HARDEN_REQUESTED_REASON,
            actor,
            "[Changes] ContinuationRequested (harden)",
        )
        .await;
        if !requested {
            return Err("the hardening run could not be requested".into());
        }
        Ok(())
    }

    pub async fn nudge_held_proposal_in_session(
        &self,
        thread_id: Uuid,
        hold: ProposalHold,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{
            ActorMode, EngineReason, EventChannel, EventMeta, ThreadEvent,
        };

        let origin = MessageOrigin::engine(match hold {
            ProposalHold::Plan(PlanHold::PlanOnlyAwaitingApproval)
            | ProposalHold::IntegratedIntoParent
            | ProposalHold::CarriedBySuccessor => return Ok(()),
            ProposalHold::Plan(_) => EngineReason::MissingPlan,
            ProposalHold::HardeningMissing => EngineReason::MissingHardening,
        });
        let text = hold.agent_nudge();
        let anchor = self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::PromptInjected {
                    text: text.to_string(),
                    mode: ActorMode::Engine,
                    origin: Some(origin.clone()),
                    injected_message_id: None,
                    delivered_event_id: None,
                },
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    actor: Some(origin),
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap_or_else(|e| {
                log_legacy!(
                    "[Changes] PromptInjected (held proposal) emit failed: {}",
                    e
                );
                None
            });
        msg_tx
            .send(crate::engine::AgentUserInput {
                text: text.to_string(),
                images: None,
                origin_event_id: anchor.map(|a| a.event_id),
                kind: crate::engine::AgentInputKind::ReentryFromEngine,
                completion: None,
            })
            .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> {
                "Session channel closed".into()
            })?;
        Ok(())
    }
}

/// INVARIANT: Should an existing pending change be re-synced to zero files?
///
/// `existing_row` is the pending row's `(file_count, requires_restart)` — the
/// two fields the reconcile rewrites. `branch_diff` is
/// `branch_changed_files_checked`'s answer: `Err` means *git could not answer*,
/// which is deliberately NOT the same as an empty diff.
///
/// Each `false` arm is load-bearing:
/// - **git errored** → never zero on a failure. `branch_changed_files` folds a
///   spawn failure, a timeout, and a missing branch ref into an empty `Vec`;
///   treating that as "no changes" would wipe the recorded file list of work
///   still sitting on the branch, which is why the caller uses the checked
///   variant.
/// - **the branch still has files** → the ordinary propose path owns that case
///   and rewrites the row with the real list.
/// - **the row already reads empty** → nothing to correct; without this the
///   reconcile would re-emit `ChangeProposed` on every single idle of a
///   diffless branch.
///
/// The caller adds the fourth refusal it can't express here: no pending row at
/// all → return, never CREATE one (otherwise every diffless session would
/// invent an empty change).
pub(crate) fn should_reconcile_emptied_change(
    existing_row: (i32, bool),
    branch_diff: Result<&[String], &str>,
) -> bool {
    let (file_count, requires_restart) = existing_row;
    match branch_diff {
        Err(_) => false,
        Ok(files) if !files.is_empty() => false,
        Ok(_) => file_count != 0 || requires_restart,
    }
}

#[cfg(test)]
mod tests {
    use super::should_reconcile_emptied_change;

    #[test]
    fn a_lane_branch_is_never_proposed() {
        let input = |branch| super::ProposeChangeInput {
            thread_id: uuid::Uuid::new_v4(),
            branch_name: branch,
            repo_root: "/repo",
            description: "",
            files: &[],
            requires_restart: false,
            channel: crate::engine::thread_events::EventChannel::ClaudeCode,
            hardened: true,
            origin: None,
        };
        let lane = crate::engine::git_ops::lane_branch(uuid::Uuid::new_v4());
        let refused = super::refuse_lane_branch(&input(&lane));
        assert!(refused.is_err_and(|e| e.to_string() == super::LANE_BRANCH_IS_NOT_A_CHANGE));
        assert!(super::refuse_lane_branch(&input("lucidos-claude-code-repo-lucidos-x")).is_ok());
    }

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn reconciles_a_row_that_still_claims_files() {
        assert!(should_reconcile_emptied_change((1, true), Ok(&[])));
        assert!(should_reconcile_emptied_change((3, false), Ok(&[])));
    }

    #[test]
    fn reconciles_a_zero_file_row_that_still_demands_a_restart() {
        assert!(should_reconcile_emptied_change((0, true), Ok(&[])));
    }

    #[test]
    fn already_reconciled_row_is_left_alone() {
        assert!(!should_reconcile_emptied_change((0, false), Ok(&[])));
    }

    #[test]
    fn git_failure_never_zeroes_the_row() {
        assert!(!should_reconcile_emptied_change(
            (7, true),
            Err("git diff --name-only main...gone failed: unknown revision"),
        ));
    }

    #[test]
    fn branch_with_files_is_left_to_the_propose_path() {
        assert!(!should_reconcile_emptied_change(
            (1, true),
            Ok(&files(&["src/main.rs"])),
        ));
    }
}
