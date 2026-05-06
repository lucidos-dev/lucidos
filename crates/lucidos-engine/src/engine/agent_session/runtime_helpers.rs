use super::lifecycle::{cancel_terminal_kind, TerminalKind};
use crate::engine::git_ops::has_branch_commits;
use crate::engine::thread_events::{EventChannel, MessageOrigin, SessionEndReason};
use crate::engine::worktree_cleanup::is_worktree_dirty;
use crate::engine::LucidosEngine;
use std::path::Path;
use uuid::Uuid;

/// Decision for the run_session safety net when CC's event loop ends without
/// emitting a `Result` event (process exit, stream EOF, parser glitch).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SafetyNetOutcome {
    /// Treat as a successful turn: commits exist on the branch and the cleanup
    /// path will propose them as a reviewable change.
    Completed,
    /// Treat as aborted: no worktree, or no commits to surface.
    Aborted,
}

/// Decide which terminal event the safety net should emit when CC's event
/// loop ended without a `Result`. The cleanup path (run_session.rs) will
/// auto-commit any dirty files in the worktree *after* this runs, so the
/// signal must agree with the eventual outcome — anything that will become
/// a `ChangeProposed` must be Completed, or the user sees a misleading
/// "Response interrupted" toast for work that was actually preserved.
///
/// Completed when the worktree exists AND either has commits ahead of main
/// OR has dirty/untracked files the cleanup path is about to commit.
pub(crate) async fn safety_net_outcome(
    worktree_path: Option<&Path>,
    repo_root: &Path,
    branch_name: &str,
) -> SafetyNetOutcome {
    let Some(wt) = worktree_path else {
        return SafetyNetOutcome::Aborted;
    };
    if has_branch_commits(repo_root, branch_name).await || is_worktree_dirty(wt).await {
        SafetyNetOutcome::Completed
    } else {
        SafetyNetOutcome::Aborted
    }
}

impl LucidosEngine {
    pub(crate) fn clear_cc_debounce(&self, thread_id: Uuid) {
        if let Ok(mut spawns) = self.last_cc_spawn.lock() {
            spawns.remove(&thread_id);
        }
    }

    /// Signal the agent runtime to terminate, then flush any un-persisted text
    /// as a final stream event. The runtime task watches `cancel` and kills its
    /// child; we don't own the process here.
    pub(crate) async fn kill_cc_and_flush(
        cancel: &tokio_util::sync::CancellationToken,
        claude_text_buf: &str,
        last_text_persisted_len: usize,
        event_bus: &crate::engine::event_bus::EventBus,
        thread_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
    ) {
        cancel.cancel();
        if !claude_text_buf.is_empty() {
            let delta =
                &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
            if !delta.is_empty() {
                event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed {
                                text: delta.to_string(),
                                agent: crate::runtime::AgentKind::ClaudeCode,
                            },
                            meta: meta.clone(),
                        },
                        "[ClaudeCode] CodingAgentTextStreamed flush on cancel",
                    )
                    .await;
            }
        }
    }

    /// Stamp `actor: System` on `meta` when an aborted-by-host-system terminal
    /// fires (safety-net, shutdown cancel) and no actor has been set already.
    /// Lets the AbortPanel render '⚙ System' for process-killed aborts —
    /// distinct from engine-deliberate work like hardening retrigger.
    pub(super) fn stamp_system_actor_if_aborted(
        meta: &mut crate::engine::thread_events::EventMeta,
        is_aborted: bool,
    ) {
        if is_aborted && meta.actor.is_none() {
            meta.actor = Some(crate::engine::thread_events::MessageOrigin::system());
        }
    }

    /// True iff an engine-internal path already pre-emitted the boundary
    /// `ResponseAborted` for this session — `run_session`'s terminal arms
    /// (Result classify, cancel, chat_cancel, safety net) skip their own emit
    /// when set, so the user sees one panel instead of two. Set by
    /// `abort_in_flight_for_restart` (`/api/restart`) and
    /// `emit_stuck_thread_eviction_abort` (`register_thread_queued` 60s
    /// timeout).
    pub(super) fn external_terminal_already_emitted(
        flag: &std::sync::atomic::AtomicBool,
        thread_id: Uuid,
        site: &'static str,
    ) -> bool {
        let skip = flag.load(std::sync::atomic::Ordering::Acquire);
        if skip {
            crate::log!(
                "[ClaudeCode] Skipping terminal emit ({}) for thread {} — external pre-emit already landed",
                site,
                thread_id
            );
        }
        skip
    }

    /// Build the terminal event for a CC turn from its classified kind.
    pub(super) fn make_terminal_event(
        kind: TerminalKind,
        text: String,
        model: Option<String>,
        reasoning_effort: Option<String>,
    ) -> crate::engine::thread_events::ThreadEvent {
        match kind {
            TerminalKind::Generated => {
                crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                    text,
                    images: vec![],
                    model,
                    reasoning_effort,
                }
            }
            TerminalKind::Canceled => {
                crate::engine::thread_events::ThreadEvent::ResponseCanceled {
                    text,
                    images: vec![],
                    model,
                    reasoning_effort,
                }
            }
            TerminalKind::Aborted => crate::engine::thread_events::ThreadEvent::ResponseAborted {
                text,
                images: vec![],
                model,
                reasoning_effort,
            },
            // ResponseFailed has no text/model fields — the partial response is
            // already in the timeline as CodingAgentTextStreamed events. The
            // error string is what the frontend renders next to the red dot.
            TerminalKind::Failed { error } => {
                crate::engine::thread_events::ThreadEvent::ResponseFailed { error }
            }
        }
    }

    /// Run the cancel-arm body shared by both `cancel.notified()` and
    /// `chat_cancel.cancelled()` in `run_session`. Kills the CC subprocess,
    /// flushes any unpersisted text, then emits the terminal event chosen by
    /// `cancel_terminal_kind` (deduping against the restart pre-emit when
    /// emitting Aborted, stamping `actor: System` when appropriate). `arm`
    /// is a stable label ("cancel" / "chat_cancel") used in log lines and the
    /// dedup site so traces stay distinguishable across the two select arms.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn emit_cancel_terminal(
        &self,
        arm: &'static str,
        thread_id: Uuid,
        is_waiting: bool,
        is_shutdown: bool,
        agent_cancel: &tokio_util::sync::CancellationToken,
        claude_text_buf: &str,
        last_text_persisted_len: usize,
        meta: &crate::engine::thread_events::EventMeta,
        external_terminal_emitted: &std::sync::atomic::AtomicBool,
        normalized_model: &Option<String>,
        cc_reasoning_effort: &Option<String>,
    ) {
        Self::kill_cc_and_flush(
            agent_cancel,
            claude_text_buf,
            last_text_persisted_len,
            &self.event_bus,
            thread_id,
            meta,
        )
        .await;
        let Some(kind) = cancel_terminal_kind(is_shutdown, is_waiting) else {
            crate::log!(
                "[ClaudeCode] {} arm: session {} was idle, skipping terminal event",
                arm,
                thread_id
            );
            return;
        };
        let is_aborted = kind == TerminalKind::Aborted;
        // Dedup BOTH Aborted and Canceled — eviction-path pre-emits set the
        // flag with actor=System, so a follow-up Canceled here would mask the
        // engine-initiated abort as a user cancel.
        if Self::external_terminal_already_emitted(external_terminal_emitted, thread_id, arm) {
            return;
        }
        let terminal_event = Self::make_terminal_event(
            kind,
            claude_text_buf.to_string(),
            normalized_model.clone(),
            cc_reasoning_effort.clone(),
        );
        // Aborted-during-shutdown means the host system killed the process;
        // stamp `actor: System` so the AbortPanel reads ⚙ System. User-driven
        // Canceled inherits the existing meta.
        let mut emit_meta = meta.clone();
        Self::stamp_system_actor_if_aborted(&mut emit_meta, is_aborted);
        let log_label = format!("[ClaudeCode] terminal event ({})", arm);
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: terminal_event,
                    meta: emit_meta,
                },
                &log_label,
            )
            .await;
    }

    /// Emit a `CodingAgentPromptSent` event for automated CC sessions (hardening,
    /// merge conflict, recovery) and return the event ID for use as `origin_id`.
    /// This ensures all events emitted by the CC session have a valid
    /// `request_event_id` pointing to a real persisted event.
    ///
    /// `origin` flows onto the event so engine-internal callers (orphan recovery,
    /// hardening retrigger) can stamp themselves with `MessageOrigin::Engine`.
    /// Pass `None` when the surrounding flow already carries the origin (e.g.
    /// merge-conflict prompts emitted as part of an apply chain that has its
    /// own actor stamping).
    pub(crate) async fn emit_automated_prompt(
        &self,
        thread_id: Uuid,
        prompt: &str,
        origin: Option<MessageOrigin>,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let result = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::CodingAgentPromptSent {
                    text: prompt.to_string(),
                    agent: crate::runtime::AgentKind::ClaudeCode,
                    origin,
                },
                meta: crate::engine::thread_events::EventMeta {
                    channel: Some(EventChannel::CodingAgent),
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await?;
        Ok(result.expect("CodingAgentPromptSent is persisted").event_id)
    }

    /// Spawn a CC task with a panic guard that emits `ResponseFailed` + `SessionEnded`
    /// if the task panics. Without this, a panic in a spawned CC task would leave
    /// the thread stuck in "running" state forever with no frontend notification.
    pub(crate) fn spawn_cc_task_guarded(
        engine: std::sync::Arc<LucidosEngine>,
        thread_id: Uuid,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let handle = tokio::spawn(future);
        Self::monitor_cc_task(engine, thread_id, handle);
    }

    /// Monitor a spawned CC task's JoinHandle. If it panics, emit `ResponseFailed`
    /// and `SessionEnded` and clean up the session entry. Shared by both
    /// `spawn_cc_task_guarded` and `spawn_agent_thread`.
    pub(crate) fn monitor_cc_task(
        engine: std::sync::Arc<LucidosEngine>,
        thread_id: Uuid,
        handle: tokio::task::JoinHandle<()>,
    ) {
        tokio::spawn(async move {
            if let Err(join_err) = handle.await {
                if join_err.is_panic() {
                    let panic_msg = match join_err.into_panic().downcast::<String>() {
                        Ok(s) => *s,
                        Err(e) => match e.downcast::<&str>() {
                            Ok(s) => s.to_string(),
                            Err(_) => "unknown panic".to_string(),
                        },
                    };
                    crate::log!(
                        "[ClaudeCode] CC task panicked for thread {}: {}",
                        thread_id,
                        panic_msg
                    );
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                    error: format!("Internal error: {}", panic_msg),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[ClaudeCode] ResponseFailed after panic",
                        )
                        .await;
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::SessionEnded {
                                    reason: SessionEndReason::Panic,
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[ClaudeCode] SessionEnded after panic",
                        )
                        .await;
                    engine.agent_sessions.lock().await.remove(&thread_id);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `stamp_system_actor_if_aborted` stamps `MessageOrigin::System` (NOT
    /// `Engine{OrphanRecovery}`) on aborted terminals — the host system killed
    /// the process; the engine just emits the marker. Engine actor stays for
    /// engine-deliberate work like hardening retrigger or scheduler.
    #[test]
    fn stamp_system_actor_stamps_system_when_aborted_and_no_actor() {
        use crate::engine::thread_events::{EventMeta, MessageOrigin};
        let mut meta = EventMeta::NONE;
        LucidosEngine::stamp_system_actor_if_aborted(&mut meta, true);
        assert!(matches!(meta.actor, Some(MessageOrigin::System)));
    }

    /// Non-aborted terminals (Generated, Canceled) carry the inbound meta
    /// untouched — Generated is a normal turn end, Canceled is user-driven.
    /// Stamping system on those would mis-attribute the AbortPanel.
    #[test]
    fn stamp_system_actor_no_op_when_not_aborted() {
        use crate::engine::thread_events::EventMeta;
        let mut meta = EventMeta::NONE;
        LucidosEngine::stamp_system_actor_if_aborted(&mut meta, false);
        assert!(meta.actor.is_none());
    }

    /// If a more specific actor is already set (e.g. device for /api/restart
    /// pre-emit), don't overwrite it. The pre-emit's device attribution must
    /// survive so the AbortPanel reads "You — Restarted" not "System".
    #[test]
    fn stamp_system_actor_does_not_overwrite_existing() {
        use crate::engine::thread_events::{EventMeta, MessageOrigin};
        let device = MessageOrigin::Device {
            device_id: "d-1".into(),
            label: "iOS Safari PWA".into(),
        };
        let mut meta = EventMeta {
            actor: Some(device.clone()),
            ..EventMeta::NONE
        };
        LucidosEngine::stamp_system_actor_if_aborted(&mut meta, true);
        assert_eq!(meta.actor, Some(device));
    }

    #[test]
    fn terminal_event_matches_kind() {
        let aborted = LucidosEngine::make_terminal_event(
            TerminalKind::Aborted,
            "partial output".into(),
            Some("claude-opus-4-6".into()),
            Some("high".into()),
        );
        match &aborted {
            crate::engine::thread_events::ThreadEvent::ResponseAborted {
                model,
                reasoning_effort,
                ..
            } => {
                assert_eq!(model.as_deref(), Some("claude-opus-4-6"));
                assert_eq!(reasoning_effort.as_deref(), Some("high"));
            }
            _ => panic!("Expected ResponseAborted"),
        }
        assert!(matches!(
            LucidosEngine::make_terminal_event(TerminalKind::Canceled, "x".into(), None, None),
            crate::engine::thread_events::ThreadEvent::ResponseCanceled { .. }
        ));
        assert!(matches!(
            LucidosEngine::make_terminal_event(TerminalKind::Generated, "x".into(), None, None),
            crate::engine::thread_events::ThreadEvent::ResponseGenerated { .. }
        ));
    }

    /// Helper: temp git repo with an initial commit on `main`.
    async fn make_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        use crate::engine::git_ops::git_cmd;
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let _ = git_cmd(&["init"], &repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
        tokio::fs::write(repo.join("init.txt"), "initial")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "initial"], &repo).await;
        (tmp, repo)
    }

    /// Regression: CC committed work but the engine never saw a `Result`
    /// event AND `/harden` had not run yet. Old heuristic required the
    /// harden marker too, so it returned Aborted and the UI showed
    /// "Response interrupted" even though the change was about to be
    /// proposed by the cleanup path.
    #[tokio::test]
    async fn safety_net_completed_when_commits_exist_without_harden_marker() {
        use crate::engine::git_ops::git_cmd;
        let (_tmp, repo) = make_test_repo().await;
        let branch = "claude-code/test-completed";
        let _ = git_cmd(&["checkout", "-b", branch], &repo).await;
        tokio::fs::write(repo.join("work.txt"), "some work")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "CC work"], &repo).await;

        // No record_hardened call — marker is absent.
        let outcome = safety_net_outcome(Some(&repo), &repo, branch).await;
        assert_eq!(
            outcome,
            SafetyNetOutcome::Completed,
            "commits without harden marker should still be treated as completed"
        );
    }

    #[tokio::test]
    async fn safety_net_aborted_when_no_worktree() {
        let (_tmp, repo) = make_test_repo().await;
        let outcome = safety_net_outcome(None, &repo, "claude-code/anything").await;
        assert_eq!(outcome, SafetyNetOutcome::Aborted);
    }

    #[tokio::test]
    async fn safety_net_aborted_when_branch_has_no_commits_and_worktree_clean() {
        use crate::engine::git_ops::git_cmd;
        let (_tmp, repo) = make_test_repo().await;
        let branch = "claude-code/test-empty";
        let _ = git_cmd(&["checkout", "-b", branch], &repo).await;
        // Branch exists but has the same HEAD as main AND no dirty files —
        // genuine no-op session.
        let outcome = safety_net_outcome(Some(&repo), &repo, branch).await;
        assert_eq!(outcome, SafetyNetOutcome::Aborted);
    }

    /// Regression for the "Adding GPT-5.5 Support to Lucidos" incident
    /// (2026-04-25): CC's stream ended without a `Result` event but it had
    /// modified files in the worktree without committing. The cleanup path
    /// auto-commits dirty work *after* the safety net runs, so the old
    /// "commits exist?" check fired before any commit existed and emitted
    /// `ResponseAborted` ("Response interrupted") even though a Change was
    /// about to be proposed. Treat dirty worktree as Completed so the
    /// terminal event matches the cleanup outcome.
    #[tokio::test]
    async fn safety_net_completed_when_worktree_dirty_without_commits() {
        use crate::engine::git_ops::git_cmd;
        let (_tmp, repo) = make_test_repo().await;
        let branch = "claude-code/test-dirty";
        let _ = git_cmd(&["checkout", "-b", branch], &repo).await;
        // Modify a tracked file but do NOT commit. CC left work uncommitted;
        // the cleanup path is about to auto-commit it.
        tokio::fs::write(repo.join("init.txt"), "modified by cc")
            .await
            .unwrap();

        let outcome = safety_net_outcome(Some(&repo), &repo, branch).await;
        assert_eq!(
            outcome,
            SafetyNetOutcome::Completed,
            "dirty worktree without commits should be Completed — cleanup will rescue the work"
        );
    }

    /// Untracked-only counterpart: CC created a brand-new file (e.g. wrote a
    /// fresh module via Bash heredoc) but didn't add or commit it. Cleanup
    /// path's `git add -A` + `git commit` rescues it, so the safety net
    /// should agree the turn was Completed.
    #[tokio::test]
    async fn safety_net_completed_when_worktree_has_untracked_files() {
        use crate::engine::git_ops::git_cmd;
        let (_tmp, repo) = make_test_repo().await;
        let branch = "claude-code/test-untracked";
        let _ = git_cmd(&["checkout", "-b", branch], &repo).await;
        tokio::fs::write(repo.join("new_file.txt"), "fresh from cc")
            .await
            .unwrap();

        let outcome = safety_net_outcome(Some(&repo), &repo, branch).await;
        assert_eq!(
            outcome,
            SafetyNetOutcome::Completed,
            "untracked files in worktree should be Completed — cleanup will commit them"
        );
    }

    #[tokio::test]
    async fn test_startup_semaphore_limits_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let sem = Arc::new(tokio::sync::Semaphore::new(2));
        let active = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..6 {
            let sem = sem.clone();
            let active = active.clone();
            let max_seen = max_seen.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert!(
            max_seen.load(Ordering::SeqCst) <= 2,
            "Semaphore should limit to 2 concurrent startups"
        );
    }
}
