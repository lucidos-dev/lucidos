use super::*;

impl LucidosEngine {
    /// If a pending change already exists for this branch, returns its ID instead of creating a duplicate.
    pub(crate) async fn propose_change(
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
            incomplete,
        } = input;

        // Reconcile the "a coding-agent thread has at most one pending change"
        // invariant BEFORE proposing on this branch: discard any pending change
        // the thread still holds on a DIFFERENT branch (e.g. a merge-conflict
        // recovery re-ran on a fresh branch). Doing this *before* the
        // `ChangeProposed` emit below is load-bearing — `ChangeDiscarded` runs
        // `CcFlagRule::ClearAll`, so discarding after the propose would wipe the
        // `coding_agent_proposed` flag this proposal is about to set. Same-branch
        // multi-change is preserved (keep = same branch). See
        // docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md.
        self.discard_pending_for_thread_except(thread_id, origin.clone(), |c| {
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
            incomplete,
        })
        .await
    }

    /// The emit half of [`propose_change`], WITHOUT its sibling-discard
    /// reconcile. If a pending change already exists for this branch, reuse its
    /// `change_id` and re-emit `ChangeProposed`; otherwise mint a new one.
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
    /// history. `incomplete` participates in the dedup so a follow-up
    /// successful turn against the same branch clears a prior failure tag (the
    /// re-emit propagates `incomplete: false` to the projection row).
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
            incomplete,
        } = input;

        let existing = self
            .changes()
            .get_pending_by_branch(branch_name)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("get_pending_by_branch({}): {}", branch_name, e).into()
            })?;
        let change_id = existing.as_ref().map(|c| c.id).unwrap_or_else(Uuid::new_v4);
        let needs_emit = existing.as_ref().is_none_or(|e| {
            e.description != description
                || e.files != files
                || e.requires_restart != requires_restart
                || e.hardened != hardened
                || e.incomplete != incomplete
        });

        if needs_emit {
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
                            incomplete,
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
        // still hold real work.
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
                incomplete: false,
            })
            .await
        {
            log!(
                "[Changes] Failed to reconcile emptied change {}: {}",
                existing.id,
                e
            );
        }
        self.broadcast_changes_updated().await;
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
