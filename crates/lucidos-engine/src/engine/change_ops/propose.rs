use super::*;
use crate::engine::event_bus::EventBus;

/// What a turn end did with the branch's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProposeOutcome {
    /// The change is pending under this id.
    Proposed(Uuid),
    /// The proposal hold keeps the branch, so nothing was proposed. The
    /// commits stay on the branch for the turn end that clears the hold.
    Held(ProposalHold),
    /// The turn did not finish, so its work stays unproposed (ADR 0400).
    Unfinished,
}

/// What withholding a turn end's work did with the branch's open change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenChangeAfterHold {
    /// The turn committed past it, so it went back to unproposed work.
    Withdrawn,
    /// It still carries all the work, so it stands as the branch's proposal.
    StillCarries(Uuid),
    /// The branch had none, or it could not be read.
    None,
}

/// Withhold a turn end's work for `reason` (ADR 0400). An open change the
/// turn committed past goes back to unproposed work first, set aside or not,
/// or Apply would merge commits it never covered. One that still carries all
/// the work stays, and nothing is announced. A failed withdrawal is an error,
/// announced as nothing.
///
/// `branch_moved` is [`branch_moved_since_last_idle`]: an amend or an
/// auto-commit can leave the files and the description unchanged.
async fn withhold_work(
    bus: &EventBus,
    input: &ProposeChangeInput<'_>,
    branch_moved: bool,
    reason: UnproposedReason,
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
            log!(
                "[Changes] get_open_by_branch({}): {}; leaving any open change in place",
                input.branch_name,
                e
            );
            OpenChangeAfterHold::None
        }
    };
    if !matches!(open, OpenChangeAfterHold::StillCarries(_)) {
        emit_proposal_withheld(bus, input.thread_id, input.branch_name, input.files, reason).await;
    }
    Ok(open)
}

/// Announce an unfinished turn's work as withheld. Returns whether it
/// withdrew an open change (see [`withhold_work`]).
pub(crate) async fn withhold_unfinished_work(
    bus: &EventBus,
    input: &ProposeChangeInput<'_>,
    branch_moved: bool,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let open = withhold_work(bus, input, branch_moved, UnproposedReason::TurnIncomplete).await?;
    Ok(open == OpenChangeAfterHold::Withdrawn)
}

/// Whether the branch head moved since the thread's last idle recorded it. An
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
        log!("[Changes] last idle head for {}: {}", thread_id, e);
        None
    })
    .flatten();
    let current = crate::engine::git_ops::branch_head_sha(repo_root, branch_name).await;
    match (recorded, current) {
        (Some(recorded), Some(current)) => recorded != current,
        _ => true,
    }
}

/// What the proposal hold did with a turn end's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoldOutcome {
    /// Nothing holds the work, so it may be proposed.
    Clear,
    /// The hold keeps the work, as [`withhold_work`] did it.
    Held(ProposalHold, OpenChangeAfterHold),
}

/// The proposal hold's verdict, announced when it keeps the branch: Apply
/// would refuse such a change or harden it first, so it is never offered.
pub(crate) async fn withhold_unready(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    input: &ProposeChangeInput<'_>,
) -> Result<HoldOutcome, Box<dyn std::error::Error + Send + Sync>> {
    let repo_root = Path::new(input.repo_root);
    let Some(hold) = read_proposal_hold(
        pool,
        input.thread_id,
        repo_root,
        input.branch_name,
        input.files,
    )
    .await
    else {
        return Ok(HoldOutcome::Clear);
    };
    log!(
        "[Changes] Not proposing branch {} for thread {}: the proposal hold keeps it ({:?})",
        input.branch_name,
        input.thread_id,
        hold
    );
    let moved =
        branch_moved_since_last_idle(pool, input.thread_id, repo_root, input.branch_name).await;
    let open = withhold_work(bus, input, moved, hold.into()).await?;
    Ok(HoldOutcome::Held(hold, open))
}

/// Send a pending or set-aside change back to unproposed work. Its branch
/// keeps every commit.
///
/// A failed emit is an error: the change would stay applicable over work it
/// no longer covers, so no caller may carry on as if it was withdrawn.
pub(crate) async fn emit_change_withdrawn(
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

/// Announce work a turn end left on the branch and did not propose.
pub(crate) async fn emit_proposal_withheld(
    bus: &EventBus,
    thread_id: Uuid,
    branch_name: &str,
    files: &[String],
    reason: UnproposedReason,
) {
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ProposalWithheld {
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

impl LucidosEngine {
    /// What a turn end does with its branch work (ADR 0400). A finished turn
    /// proposes it; an unfinished one never does, and says so.
    pub(crate) async fn propose_turn_work(
        &self,
        input: ProposeChangeInput<'_>,
        finished: bool,
    ) -> Result<ProposeOutcome, Box<dyn std::error::Error + Send + Sync>> {
        if finished {
            return self.propose_change(input).await;
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

    /// Propose the branch's work, unless the proposal hold keeps it
    /// ([`withhold_unready`]). If an open change already exists for this
    /// branch, reuses its id instead of creating a duplicate.
    pub(crate) async fn propose_change(
        &self,
        input: ProposeChangeInput<'_>,
    ) -> Result<ProposeOutcome, Box<dyn std::error::Error + Send + Sync>> {
        match withhold_unready(&self.pool, &self.event_bus, &input).await? {
            HoldOutcome::Clear => {}
            // Nothing new since it was proposed, so the open change stands.
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

        let ProposeChangeInput {
            thread_id,
            branch_name,
            repo_root,
            description,
            files,
            requires_restart,
            channel,
            hardened,
            origin,
        } = input;

        // Reconcile the "a coding-agent thread has at most one open change"
        // invariant BEFORE proposing on this branch: discard any open change
        // the thread still holds on a DIFFERENT branch (e.g. a merge-conflict
        // recovery re-ran on a fresh branch). Doing this *before* the
        // `ChangeProposed` emit below is load-bearing: `ChangeDiscarded` clears
        // the branch work, so discarding after the propose would wipe the
        // state this proposal is about to set. Same-branch
        // multi-change is preserved (keep = same branch). See
        // docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md.
        self.discard_open_changes_for_thread_except(thread_id, origin.clone(), |c| {
            c.branch_name == branch_name
        })
        .await;

        self.emit_change_proposed(ProposeChangeInput {
            thread_id,
            branch_name,
            repo_root,
            description,
            files,
            requires_restart,
            channel,
            hardened,
            origin,
        })
        .await
        .map(ProposeOutcome::Proposed)
    }

    /// The emit half of [`propose_change`], WITHOUT its sibling-discard
    /// reconcile. If an open change already exists for this branch, reuse its
    /// `change_id` and re-emit `ChangeProposed`; otherwise mint a new one. New
    /// work on a set-aside change brings it back first (ADR 0328).
    ///
    /// Split out because `reconcile_emptied_pending_change` must correct a row
    /// without resolving anything: routing it through `propose_change` would
    /// first discard every OTHER pending change the thread holds — so
    /// re-syncing an emptied change on branch A could silently discard a
    /// sibling pending change on branch B that still holds real work, which is
    /// exactly the "engine never resolves a change on the user's behalf" rule
    /// (`cca058432`) this feature is built around. The sibling discard belongs
    /// to a genuinely NEW proposal, not to a correction.
    ///
    /// The `needs_emit` guard short-circuits when no field changed — without
    /// it, every CC end-of-turn would re-emit identical events and inflate
    /// history. A proposal is always finished work, so it clears an
    /// `incomplete` mark a row from before ADR 0400 still carries.
    async fn emit_change_proposed(
        &self,
        input: ProposeChangeInput<'_>,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let ProposeChangeInput {
            thread_id,
            branch_name,
            repo_root,
            description,
            files,
            requires_restart,
            channel,
            hardened,
            origin,
        } = input;

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

        // Only new work brings a set-aside change back. A proposal that
        // differs in its flags alone leaves the change where the user put it.
        let set_aside = existing
            .as_ref()
            .is_some_and(|e| e.status() == ChangeStatus::SetAside);
        if set_aside && !new_work {
            return Ok(change_id);
        }

        if needs_emit {
            if set_aside {
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ChangeBroughtBack {
                                change_id: change_id.to_string(),
                            },
                            meta: crate::engine::thread_events::EventMeta::with_actor(
                                origin.clone(),
                            ),
                        },
                        "[Changes] ChangeBroughtBack (new work)",
                    )
                    .await;
            }
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ChangeProposed {
                            change_id: change_id.to_string(),
                            description: Some(description.to_string()),
                            files: files.to_vec(),
                            requires_restart,
                            origin,
                            commit_sha: None,
                            branch_name: branch_name.to_string(),
                            repo_root: repo_root.to_string(),
                            hardened,
                            incomplete: false,
                            set_aside: false,
                            path: String::new(),
                            diff: String::new(),
                        },
                        meta: crate::engine::thread_events::EventMeta {
                            channel: Some(channel),
                            ..crate::engine::thread_events::EventMeta::NONE
                        },
                    },
                    "[Changes] ChangeProposed",
                )
                .await;
            if existing.is_some() {
                self.broadcast_changes_updated().await;
            }
        }
        Ok(change_id)
    }

    /// Whether an open change already names `branch_name`. An unanswered read
    /// counts as no: a held idle then creates nothing, and the next idle asks
    /// again.
    pub(crate) async fn branch_has_open_change(&self, branch_name: &str) -> bool {
        match self.changes().get_open_by_branch(branch_name).await {
            Ok(row) => row.is_some(),
            Err(e) => {
                log!(
                    "[Changes] get_open_by_branch({}): {}; treating it as having no open change",
                    branch_name,
                    e
                );
                false
            }
        }
    }

    /// Re-sync a pending change whose branch diff has since gone empty.
    ///
    /// The propose path keeps a pending change's `files` in step with git on
    /// every clean idle — except when the list becomes EMPTY, because the
    /// propose gate gives up before it gets there. The row then keeps claiming
    /// files (and a restart) that the branch no longer has, so the Changes card
    /// says "1 file · Requires engine restart" while the Diff button, which
    /// runs a live `git diff`, renders "No changes" (real change `2cc8391f`: a
    /// stray file was auto-committed on shutdown and a later commit on the same
    /// branch deleted it). This closes that one-directional sync.
    ///
    /// Deliberately NOT a discard: the engine never resolves a change on the
    /// user's behalf (commit `cca058432`). The change stays `pending` under its
    /// original id, keeps its Diff/Discard buttons, and is simply honest about
    /// containing nothing.
    ///
    /// The decision itself is [`should_reconcile_emptied_change`] — see its
    /// truth table for the refusals and why each one is load-bearing.
    pub(crate) async fn reconcile_emptied_pending_change(
        &self,
        thread_id: Uuid,
        repo_root: &Path,
        branch_name: &str,
    ) {
        let existing = match self.changes().get_pending_by_branch(branch_name).await {
            Ok(row) => row,
            Err(e) => {
                log!(
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
            log!(
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
            crate::engine::git_ops::is_harden_marker_present(&self.pool, repo_root, branch_name)
                .await;
        let fallback = crate::engine::agent_session::change_description_fallback(
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

        log!(
            "[Changes] Branch {} has no diff left — change {} reconciled to 0 files (was {})",
            branch_name,
            existing.id,
            existing.file_count
        );
        // `emit_change_proposed`, NOT `propose_change`: the latter first
        // discards every other pending change the thread holds, which would
        // turn this correction into a resolution of a sibling change that may
        // still hold real work. It broadcasts the corrected row itself.
        if let Err(e) = self
            .emit_change_proposed(ProposeChangeInput {
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
            log!(
                "[Changes] Failed to reconcile emptied change {}: {}",
                existing.id,
                e
            );
        }
    }

    /// Emit the harden boundary event then queue AUTO_HARDEN_MESSAGE on a live
    /// agent session. Emit-before-send guarantees the panel sits above any
    /// CodingAgentTextStreamed events CC produces in response.
    ///
    /// The boundary event is stamped engine-origin internally (see
    /// `emit_missing_hardening_detected`), so callers don't pass an actor.
    pub(crate) async fn request_hardening_in_session(
        &self,
        thread_id: Uuid,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.emit_missing_hardening_detected(thread_id).await;
        msg_tx
            .send(crate::engine::AgentUserInput {
                text: crate::engine::claude_code::AUTO_HARDEN_MESSAGE.to_string(),
                images: None,
                origin_event_id: None,
                kind: crate::engine::AgentInputKind::User,
            })
            .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> {
                "Session channel closed".into()
            })?;
        Ok(())
    }

    /// Start the hardening run for work held for `hardening_missing`, which
    /// the Not ready strip's **Harden** asks for. A live session gets the
    /// hold's nudge. Otherwise the thread resumes with the harden message, as
    /// Continue resumes one. Either way its turn end proposes through the
    /// normal floor. No apply runs, so it emits no `MissingHardeningDetected`,
    /// which marks an apply's hardening phase.
    pub(crate) async fn harden_held_work(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(session) = CodingAgentChangeOps::live_session_info(self, thread_id).await {
            return self
                .nudge_held_proposal_in_session(
                    thread_id,
                    ProposalHold::HardeningMissing,
                    &session.msg_tx,
                )
                .await;
        }
        let requested = crate::engine::thread_events::emit_continuation_requested_or_log(
            &self.event_bus,
            thread_id,
            crate::engine::agent_recovery::HARDEN_REQUESTED_REASON,
            actor,
            "[Changes] ContinuationRequested (harden)",
        )
        .await;
        if !requested {
            return Err("the hardening run could not be requested".into());
        }
        Ok(())
    }

    /// Tell a live agent why its turn-end proposal was held, and how to clear
    /// the hold. The `PromptInjected` anchor starts the exchange its reply
    /// groups under, as an event-wait re-entry does.
    pub(crate) async fn nudge_held_proposal_in_session(
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
                log!(
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
            })
            .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> {
                "Session channel closed".into()
            })?;
        Ok(())
    }
}

/// Should an existing pending change be re-synced to zero files?
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

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    /// The incident shape: the row claims a file (and a restart), git says the
    /// branch's commits cancel out.
    #[test]
    fn reconciles_a_row_that_still_claims_files() {
        assert!(should_reconcile_emptied_change((1, true), Ok(&[])));
        assert!(should_reconcile_emptied_change((3, false), Ok(&[])));
    }

    /// A restart flag with no files is just as much of a lie as a file count —
    /// the card renders "Requires engine restart" and Apply reads "Apply*".
    #[test]
    fn reconciles_a_zero_file_row_that_still_demands_a_restart() {
        assert!(should_reconcile_emptied_change((0, true), Ok(&[])));
    }

    /// Idempotence: an already-reconciled row must not re-emit
    /// `ChangeProposed` on every subsequent idle of the same diffless branch.
    #[test]
    fn already_reconciled_row_is_left_alone() {
        assert!(!should_reconcile_emptied_change((0, false), Ok(&[])));
    }

    /// Zeroing on a git failure would destroy the file list of work that is
    /// still on the branch — a missing ref, a timeout, and a spawn failure all
    /// arrive here as `Err`.
    #[test]
    fn git_failure_never_zeroes_the_row() {
        assert!(!should_reconcile_emptied_change(
            (7, true),
            Err("git diff --name-only main...gone failed: unknown revision"),
        ));
    }

    /// A branch that still has a diff belongs to the ordinary propose path,
    /// which rewrites the row with the real list.
    #[test]
    fn branch_with_files_is_left_to_the_propose_path() {
        assert!(!should_reconcile_emptied_change(
            (1, true),
            Ok(&files(&["src/main.rs"])),
        ));
    }
}
