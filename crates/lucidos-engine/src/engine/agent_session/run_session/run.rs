use super::idle_snapshot::CodingAgentIdleSnapshot;
use super::spawn_context::SpawnWorktreeContext;
use crate::engine::agentic_loop::should_flush;
use crate::engine::change_ops::now_epoch_millis;
use crate::engine::claude_code::STALE_RESUME_ERROR;
use crate::engine::git_ops::{
    auto_commit_preserving_marker, branch_changed_files, default_local_branch,
    describe_branch_changes, files_require_restart, git_cmd, is_external_repo_path,
    is_harden_marker_present, main_worktree,
};
use crate::engine::thread_events::{EventChannel, SessionEndReason};
use crate::engine::{AgentSession, AgentUserInput, LucidosEngine, ProcessResult, StopReason};
use crate::runtime::{AgentEvent, AgentInput, CodingAgent};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use crate::engine::agent_session::io_helpers::{drain_lost_followups, lost_followups_to_orphans};
use crate::engine::agent_session::lifecycle::{
    classify_result, idle_action,
    is_silent_resume, is_stale_resume_signal, reset_per_turn_flags,
    should_auto_commit_on_cleanup, should_propose_change_at_idle,
    terminate_decision, watchdog_gate, IdleAction, TerminalKind,
    TerminateDecision, WatchdogGate, WATCHDOG_DIAG_LOG_THRESHOLD_MS,
    WATCHDOG_INACTIVITY_LIMIT_MS, WATCHDOG_TICK_INTERVAL_SECS,
};
use crate::engine::agent_session::resume::{change_description_fallback, resolve_resume_context, CC_TURN_CLOSER_EVENTS};
use crate::engine::agent_session::spawn::spawn_or_resume;

impl LucidosEngine {
    // Bridges every per-session input (thread / user msg / images / origin /
    // cancel / recovery / repo / prompt / resume / CC model+effort + worktree)
    // to the agent runtime; a builder would just shuffle the same fields.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_direct_agent(
        &self,
        request_id: Uuid,
        thread_id: Uuid,
        user_message: &str,
        user_images: Option<&[crate::api::ChatImage]>,
        origin_id: Uuid,
        spawning_event_id: Option<Uuid>,
        cancel_token: &tokio_util::sync::CancellationToken,
        conflict_change_id: Option<Uuid>,
        recovery_worktree: Option<(PathBuf, String)>,
        repo_id: Option<String>,
        system_prompt_override: Option<String>,
        resume_session_id: Option<String>,
        cc_model: Option<String>,
        cc_reasoning_effort: Option<String>,
        // CWD for `--resume`. See the `UserQuestionAsked.worktree_path` doc.
        resume_worktree_path: Option<PathBuf>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let cc_start = std::time::Instant::now();
        let thread_id_str = thread_id.to_string();

        // Pre-spawn app-coding-agent-thread detection: `spawn_agent_thread`
        // stashes the app id here when the LLM picks `folder=data/apps/<id>`;
        // pop it once so the worktree dispatcher routes to sparse-checkout
        // and the system-prompt selector picks the app variant. Falls back
        // to thread_summaries on resume, so a follow-up message on an
        // existing app thread still routes correctly after the pending
        // stash was cleared by the initial spawn.
        let app_spawn_id: Option<String> = {
            let mut guard = self.pending_app_spawn.lock().expect("pending_app_spawn poisoned");
            guard.remove(&thread_id)
        };
        let app_spawn_id = if app_spawn_id.is_some() {
            app_spawn_id
        } else {
            // Resume path — read from thread_summaries. Require kind == 'app';
            // when folder is missing (early row, projection lag, partial
            // replay) fall back to the worktree path on disk so a follow-up
            // turn still routes through the sparse-checkout dispatch instead
            // of silently re-spawning as a Lucidos-source thread.
            match sqlx::query_as::<_, (Option<String>, Option<String>)>(
                "SELECT coding_agent_kind, coding_agent_folder FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_optional(&self.pool)
            .await
            {
                Ok(Some((Some(k), folder))) if k == "app" => {
                    if let Some(f) = folder {
                        std::path::Path::new(&f)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .map(str::to_string)
                    } else {
                        // Folder is NULL but kind is 'app'. Probe disk for
                        // the worktree's `data/apps/<id>/` so we can recover
                        // the app id. Worst case (no worktree on disk yet),
                        // return a placeholder so the dispatcher errors
                        // cleanly instead of silently using the wrong path.
                        log!(
                            "[ClaudeCode] thread {} has coding_agent_kind='app' but NULL folder — \
                             folder reconstruction would race the projection; refusing to spawn",
                            thread_id
                        );
                        return Err(
                            "Thread is marked as an app coding-agent thread but its \
                             coding_agent_folder is missing — wait for the projection \
                             to settle or re-archive the thread."
                                .into(),
                        );
                    }
                }
                _ => None,
            }
        };
        let is_app_spawn = app_spawn_id.is_some();

        let meta = crate::engine::thread_events::EventMeta {
            request_event_id: Some(origin_id),
            channel: Some(EventChannel::ClaudeCode),
            ..crate::engine::thread_events::EventMeta::NONE
        };

        // Check if already running for this thread (single lock to avoid TOCTOU).
        // Skip for recovery sessions — the old session stays in agent_sessions
        // during the handoff so the thread remains in the "active" set. This session
        // will replace it via insert().
        let mut had_dead_session = false;
        if recovery_worktree.is_none() {
            let guard = self.agent_sessions.lock().await;
            if let Some(session) = guard.get(&thread_id) {
                if !session.process_exited {
                    if session.is_waiting {
                        // Session is idle — route follow-up via msg_tx. The caller
                        // already emitted MessageReceived with the frontend UUID.
                        log!("[ClaudeCode] Session already running and idle — routing follow-up via msg_tx");
                        let images = user_images.map(|imgs| imgs.to_vec());
                        session
                            .pending_followups
                            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        if session
                            .msg_tx
                            .send(AgentUserInput {
                                text: user_message.to_string(),
                                images,
                                origin_event_id: Some(origin_id),
                                kind: crate::engine::AgentInputKind::User,
                            })
                            .is_err()
                        {
                            session
                                .pending_followups
                                .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                            drop(guard);
                            return Err("Claude Code session ended while routing message. Please try again.".into());
                        }
                        drop(guard);
                        return Ok(ProcessResult {
                            response: String::new(),
                            steps: vec![],
                            images: vec![],
                            request_id,
                            thread_id,
                            proposed_change: false,
                            auto_apply: false,
                            orphaned_injections: vec![],
                        });
                    }
                    drop(guard);
                    return Err("Claude Code is already running for this thread. Cancel it first or wait for it to finish.".into());
                }
                had_dead_session = true;
            }
            drop(guard);
        }

        // Debounce: reject if a Claude Code session was spawned very recently for THIS thread
        // (prevents double-submit). Per-thread so concurrent starts on different threads
        // are not blocked. Skip for recovery sessions and for follow-ups after a dead
        // session (process_exited=true) — those are legitimate new requests, not
        // double-submits.
        if recovery_worktree.is_none() {
            let mut spawns = self.last_cc_spawn.lock().unwrap();
            if !had_dead_session {
                if let Some(t) = spawns.get(&thread_id) {
                    if t.elapsed() < std::time::Duration::from_secs(3) {
                        return Err(
                            "A Claude Code session was just started — ignoring duplicate request."
                                .into(),
                        );
                    }
                }
            }
            // Prune expired entries to prevent unbounded growth
            spawns.retain(|_, t| t.elapsed() < std::time::Duration::from_secs(10));
            spawns.insert(thread_id, std::time::Instant::now());
        }

        let (resume_session_id, resume_branch) =
            if recovery_worktree.is_none() && conflict_change_id.is_none() {
                resolve_resume_context(self.pool(), self.changes(), thread_id, resume_session_id).await
            } else {
                (resume_session_id, None)
            };

        // Conflict resolution mode: run in the merge worktree (not repo root)
        let conflict_change = if let Some(cid) = conflict_change_id {
            Some(
                self.changes()
                    .get_by_id(cid)
                    .await?
                    .ok_or("Conflict change not found")?,
            )
        } else {
            None
        };

        log!(
            "[ClaudeCode] [TIMING] resume lookup: {:?}",
            cc_start.elapsed()
        );

        // If no repo_id was provided (e.g. follow-up message), look up the thread's
        // stored repo from thread_summaries so we stay bound to the original repo.
        let repo_id = if repo_id.is_none() {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT cc_repo_id FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_optional(&self.pool)
            .await?
            .flatten()
        } else {
            repo_id
        };

        // Claude Code sessions must branch from main, not from a stale worktree branch.
        let dev_root = main_worktree().await;

        // Explicit repo_id wins; otherwise fall back to the workspace's registered
        // Lucidos repo so SessionStarted always names a real repository. Without
        // this, the route panel rendered the workspace name as if it were the repo.
        let repo = if let Some(ref rid) = repo_id {
            let repo_uuid = Uuid::parse_str(rid)?;
            Some(
                crate::core::repositories::RepositoryStore::get(&self.pool, repo_uuid)
                    .await?
                    .ok_or_else(|| format!("Repository {} not found", rid))?,
            )
        } else {
            crate::core::repositories::RepositoryStore::get_by_name(
                &self.pool,
                Self::DEFAULT_REPO_NAME,
            )
            .await?
        };

        let (repo_id, repo_root, is_external_repo, external_repo_name, repo_name) = if is_app_spawn {
            // App coding-agent thread: the worktree's git root is the
            // workspace itself, not any registered repo. Skip the repo
            // lookup entirely — apps aren't in the repo registry.
            (None, self.workspace_path.clone(), false, None, None)
        } else if let Some(repo) = repo {
            let path = PathBuf::from(&repo.path);
            if !path.exists() {
                return Err(format!("Repository path does not exist: {}", repo.path).into());
            }
            let is_external = is_external_repo_path(&path, &dev_root);
            let repo_name = repo.name.clone();
            let external_repo_name = if is_external { Some(repo.name) } else { None };
            (
                Some(repo.id.to_string()),
                path,
                is_external,
                external_repo_name,
                Some(repo_name),
            )
        } else {
            // No default registered (very early startup) and no explicit id.
            (None, dev_root, false, None, None)
        };

        let workspace_name = self.workspace_name();
        let last_idle_sha =
            crate::engine::agent_session::resume::lookup_latest_worktree_head_sha(self.pool(), thread_id).await;
        let SpawnWorktreeContext {
            cwd,
            system_prompt,
            branch_name,
            worktree_path,
            interactive_session,
            adoption_note,
            resume_session_id,
        } = self
            .resolve_run_worktree_context(
                recovery_worktree,
                &conflict_change,
                system_prompt_override,
                &app_spawn_id,
                is_app_spawn,
                is_external_repo,
                external_repo_name,
                &workspace_name,
                &repo_root,
                &repo_id,
                &last_idle_sha,
                resume_worktree_path,
                resume_branch,
                resume_session_id,
                thread_id,
                cc_start,
            )
            .await?;

        // Append thread history as context so new Claude Code sessions in an existing thread
        // can see what was discussed/done previously.
        let system_prompt = {
            let thread_messages = self.event_store.get_thread_messages(&thread_id_str).await?;
            if thread_messages.is_empty() {
                system_prompt
            } else {
                let mut history = String::from("\n\nTHREAD HISTORY: This session continues an existing thread. Here is the conversation so far:\n\n");
                for msg in &thread_messages {
                    let content = msg.content.trim();
                    if content.is_empty() {
                        continue;
                    }
                    let label = match msg.role.as_str() {
                        "user" => "User",
                        "assistant" if msg.channel.as_deref() == Some("claude_code") => {
                            "Claude Code"
                        }
                        "assistant" => "Assistant",
                        other => other,
                    };
                    // Truncate very long messages to keep the prompt reasonable
                    let truncated = if content.len() > 2000 {
                        let end = content.floor_char_boundary(2000);
                        format!(
                            "{}…\n[truncated, {} chars total]",
                            &content[..end],
                            content.len()
                        )
                    } else {
                        content.to_string()
                    };
                    history.push_str(&format!("**{}:** {}\n\n", label, truncated));
                }
                history.push_str("---\nEnd of thread history. The user's new message follows.\n");
                format!("{}{}", system_prompt, history)
            }
        };

        // Resolve model/effort BEFORE spawning: explicit param > active session > thread events.
        // Must happen before spawn_or_resume so the CC process starts with the correct model.
        // Shadow cc_model so Init handler knows a model was pre-selected and won't overwrite.
        let (prev_model, prev_effort, prev_builtin, prev_skill) = {
            let sessions = self.agent_sessions.lock().await;
            sessions
                .get(&thread_id)
                .map(|s| {
                    (
                        s.current_model.clone(),
                        s.current_reasoning_effort.clone(),
                        s.builtin_commands.clone(),
                        s.skill_commands.clone(),
                    )
                })
                .unwrap_or_default()
        };
        let (event_model, event_effort) = if cc_model.is_none() || cc_reasoning_effort.is_none() {
            self.cc_thread_settings(thread_id).await
        } else {
            (None, None)
        };
        let cc_model = cc_model.or(prev_model).or(event_model);
        let cc_reasoning_effort = cc_reasoning_effort
            .or(prev_effort)
            .or(event_effort)
            .or_else(crate::runtime::claude_code::read_cc_default_effort);

        // Acquire startup semaphore — limits concurrent CC process initializations.
        // Hold the permit until Init event is received (process is initialized and mostly idle).
        let startup_permit = self
            .cc_startup_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("Startup semaphore closed: {}", e))?;
        log!(
            "[ClaudeCode] [TIMING] Startup semaphore acquired: {:?}",
            cc_start.elapsed()
        );

        if resume_session_id.is_some() {
            log!("[ClaudeCode] Resuming session for thread {}", thread_id);
        }
        let agent_cancel = tokio_util::sync::CancellationToken::new();
        let allowed_tools = crate::engine::claude_code::cc_allowed_tools(self.user_dir());
        let runtime = match spawn_or_resume(
            self,
            CodingAgent::ClaudeCode,
            crate::runtime::SpawnArgs {
                worktree_path: &cwd,
                workspace_path: self.workspace_path(),
                allowed_tools: Some(&allowed_tools),
                system_prompt: Some(&system_prompt),
                resume_session_id: resume_session_id.as_deref(),
                model: cc_model.as_deref(),
                reasoning_effort: cc_reasoning_effort.as_deref(),
                thread_id,
                spawning_event_id,
                repo_name: repo_name.as_deref(),
                interactive: interactive_session,
            },
            agent_cancel.clone(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                if let Some(ref wt) = worktree_path {
                    let wt_str = wt.to_string_lossy();
                    let _ = git_cmd(
                        &["worktree", "remove", "--force", &wt_str],
                        &repo_root,
                    )
                    .await;
                }
                let _ = git_cmd(&["branch", "-D", &branch_name], &repo_root).await;
                return Err(format!("Failed to start Claude Code: {}", e).into());
            }
        };

        log!(
            "[ClaudeCode] [TIMING] CC process spawned: {:?}",
            cc_start.elapsed()
        );

        let crate::runtime::RunningAgent {
            mut events_rx,
            input_tx: agent_input_tx,
            control_tx: agent_control_tx,
            kind: _,
        } = runtime;

        // Skip empty messages (warm-up resumes) to avoid triggering unwanted LLM output.
        // AskUserQuestion answers are sent as plain user messages, not `tool_result`
        // blocks: `claude --print --resume` of an unfinished tool_use auto-injects
        // synthetic `Continue from where you left off.` / `No response requested.`
        // BEFORE processing stdin, orphaning any `tool_result` we'd send for the
        // original tool_use_id and making the LLM re-ask the same question.
        let has_user_images = user_images.is_some_and(|imgs| !imgs.is_empty());
        let has_content = !user_message.is_empty() || has_user_images;
        if has_content {
            let images = user_images.map(|imgs| imgs.to_vec()).unwrap_or_default();

            // Phase 8.2: detect external user edits made between turns and
            // prepend a short note so CC reacts instead of being surprised.
            // Only fires when:
            //   - this thread has at least one prior `CodingAgentIdled` event
            //     with a recorded `worktree_head_sha` (skips truly-first
            //     spawns, where there's no SHA to compare against)
            //   - the user message itself is non-empty (continue-signal
            //     style empty inputs already produce an empty `has_content`
            //     branch above and never reach this code)
            //   - the worktree has actually changed since the recorded SHA
            //     (no diff → no note, see helper)
            //
            // The note is prepended to the text only — images are forwarded
            // as-is. Failures inside the helper degrade silently to "no
            // note", matching the rest of the resume code's tolerance for
            // best-effort git introspection.
            let final_text = if !user_message.is_empty() {
                let edit_note = match (worktree_path.as_deref(), last_idle_sha.as_deref()) {
                    (Some(wt), Some(sha)) => {
                        crate::engine::agent_session::external_edits::compute_external_edit_note(wt, Some(sha)).await
                    }
                    _ => None,
                };
                let combined = match (adoption_note.as_deref(), edit_note.as_deref()) {
                    (Some(a), Some(e)) => Some(format!("{}\n{}", a, e)),
                    (Some(a), None) => Some(a.to_string()),
                    (None, Some(e)) => Some(e.to_string()),
                    (None, None) => None,
                };
                match combined {
                    Some(n) => {
                        log!(
                            "[ClaudeCode] Injecting external-edit note for thread {} ({} chars)",
                            thread_id,
                            n.len()
                        );
                        format!("{}\n\n{}", n, user_message)
                    }
                    None => user_message.to_string(),
                }
            } else {
                user_message.to_string()
            };

            if agent_input_tx
                .send(AgentInput {
                    text: final_text,
                    images,
                })
                .is_err()
            {
                if let Some(ref wt) = worktree_path {
                    let wt_str = wt.to_string_lossy();
                    let _ = git_cmd(
                        &["worktree", "remove", "--force", &wt_str],
                        &repo_root,
                    )
                    .await;
                }
                let _ = git_cmd(&["branch", "-D", &branch_name], &repo_root).await;
                return Err(
                    "Agent input channel closed before initial prompt could be sent".into(),
                );
            }
        }

        let mut startup_permit = Some(startup_permit);

        // Create channel for user follow-up messages and register the session
        let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let stop = Arc::new(tokio::sync::Notify::new());
        let interrupt = Arc::new(tokio::sync::Notify::new());
        let idle_notify = Arc::new(tokio::sync::Notify::new());
        let shutting_down = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let external_terminal_emitted =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut normalized_model = cc_model.clone();
        // Initial input (when has_content) produces one expected `Result` event;
        // see AgentSession.pending_followups for the full rationale.
        let pending_followups = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(
            if has_content { 1 } else { 0 },
        ));
        // Cloned into the session struct so the external watchdog reads
        // the same atomic the loop below mutates.
        let tools_in_flight_shared =
            std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
        {
            let mut sessions = self.agent_sessions.lock().await;
            let session = AgentSession {
                msg_tx: msg_tx.clone(),
                is_waiting: false,
                has_changes: false,
                requires_restart: false,
                pending_stop: None,
                stop: stop.clone(),
                interrupt: interrupt.clone(),
                idle_notify: idle_notify.clone(),
                apply_now_in_progress: false,
                process_exited: false,
                worktree_path: worktree_path.clone(),
                branch_name: Some(branch_name.clone()),
                repo_root: Some(repo_root.clone()),
                cc_session_id: None,
                last_event_at: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(
                    now_epoch_millis(),
                )),
                shutting_down: shutting_down.clone(),
                external_terminal_emitted: external_terminal_emitted.clone(),
                control_tx: agent_control_tx.clone(),
                builtin_commands: prev_builtin,
                skill_commands: prev_skill,
                current_model: normalized_model.clone(),
                current_reasoning_effort: cc_reasoning_effort.clone(),
                pending_followups: pending_followups.clone(),
                tools_in_flight: tools_in_flight_shared.clone(),
            };
            sessions.insert(thread_id, session);
        }

        let chat_cancel = cancel_token.clone();
        let images: Vec<String> = Vec::new();

        // Emit SessionStarted immediately so the branch→thread mapping exists
        // before CC produces any output. Without this, an engine crash during CC
        // initialization leaves no mapping and recovery creates orphan threads.
        // The cc_session_id is not yet known (comes from CC's Init event), but
        // recovery uses CodingAgentIdled for --resume, not SessionStarted.
        let session_kind = if app_spawn_id.is_some() {
            crate::engine::agent_session::CodingAgentKind::App
        } else if is_external_repo {
            crate::engine::agent_session::CodingAgentKind::External
        } else {
            crate::engine::agent_session::CodingAgentKind::Lucidos
        };
        let session_folder = if let Some(ref app_id) = app_spawn_id {
            self.workspace_path
                .join("data")
                .join("apps")
                .join(app_id)
                .to_string_lossy()
                .to_string()
        } else {
            // Lucidos / External: folder = repo_root. Workspace-local repos
            // (lucidos source under a non-default install root) reuse the
            // same path their SessionStarted event has always carried.
            repo_root.to_string_lossy().to_string()
        };
        if let Err(e) = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::SessionStarted {
                    session_id: String::new(),
                    branch: branch_name.clone(),
                    repo_id: repo_id.clone(),
                    coding_agent_kind: session_kind,
                    coding_agent_folder: session_folder,
                    app_id: app_spawn_id.clone(),
                },
                meta: meta.clone(),
            })
            .await
        {
            log!(
                "[ClaudeCode] Failed to emit initial SessionStarted for {}: {}",
                thread_id,
                e
            );
        }

        // Seed `coding_agent_has_diff` from the actual worktree state. The
        // projection's per-event handlers maintain this column for live
        // updates (ChangeProposed/Applied/Discarded/Archived) but cannot fill
        // in the gap between session start and the first new commit — a CC
        // thread resumed after engine restart would show no Diff button until
        // its next commit, even when commits already exist on the branch.
        // Outside the projection tx by design: `git rev-list` inside a
        // Postgres tx is the wrong shape. The helper logs and continues on
        // failure — bootstrap must not block on git or the projection write.
        // On git error this writes TRUE optimistically; the recovery sweep /
        // next commit hook will correct it.
        //
        // Seeds for both initial-start (SessionStarted, via the chat HTTP
        // handler → `run_direct_agent`) and resume (ContinuationStarted, via
        // `SpawnConsumer::Continue` → `run_direct_agent`) — both paths land
        // here.
        crate::engine::session_seed::seed_coding_agent_has_diff(
            self.pool(),
            thread_id,
            &repo_root,
            &branch_name,
        )
        .await;

        // Persist initial model/effort so cc_thread_settings() can restore them
        // after the session exits. Without this, the frontend loses the model
        // selection when viewing idle threads (no live session to query).
        if normalized_model.is_some() || cc_reasoning_effort.is_some() {
            if let Err(e) = self
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentSettingsChanged {
                        model: normalized_model.clone(),
                        reasoning_effort: cc_reasoning_effort.clone(),
                        permission_mode: None,
                        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                        // Fires before CC's Init — the session id isn't known
                        // yet. The Init handler emits a second SettingsChanged
                        // carrying it (see below).
                        cc_session_id: None,
                    },
                    meta: meta.clone(),
                })
                .await
            {
                log!(
                    "[ClaudeCode] Failed to persist initial CodingAgentSettingsChanged for {}: {}",
                    thread_id,
                    e
                );
            }
        }

        let mut result_texts: Vec<String> = Vec::new();
        let mut claude_text_buf = String::new();
        let mut last_text_persisted_len: usize = 0;
        let mut is_waiting = false;
        let mut proposed_change = false;
        let mut emitted_terminal_event = false; // Track whether ResponseGenerated/ResponseCanceled was emitted
        // user_hit_stop: when true, the next Result emits ResponseCanceled (exchange:
        // "Canceled") instead of ResponseGenerated. Reset on next user follow-up.
        let mut user_hit_stop = false;
        // last_emitted_idle: true iff the most recent in-loop event was
        // CodingAgentIdled. The post-loop relies on this flag to decide whether to
        // synthesize an idle event before SessionEnded.
        let mut last_emitted_idle = false;
        // Paired ToolCalled / ToolResult counter. Watchdog disarms while
        // > 0 — tool execution (Bash, Read, AskUserQuestion, TaskOutput,
        // agent sub-tasks) is legitimate silence, not a hang. CC may batch
        // multiple calls per turn, so a counter (not a bool) is the right
        // shape. Mirrored on `AgentSession` so the external watchdog sees
        // the same atomic from outside this `select!`.
        let tools_in_flight = tools_in_flight_shared;
        // last_terminal_kind: terminal emitted by the most recently completed
        // turn. Drives `should_auto_commit_on_cleanup` so we only commit (and
        // therefore fire the per-commit hook → ChangeProposed) when the last
        // turn ended Generated. Safety-net abort, Failed, Canceled, Aborted
        // all leave this None or non-Generated → no auto-commit, no spurious
        // Apply card. Reset on each new turn alongside `emitted_terminal_event`.
        let mut last_terminal_kind: Option<TerminalKind> = None;
        // Set by the watchdog tick below; consumed by the safety net to
        // pick ContinuationRequested auto-resume vs ResponseAborted. Not derived
        // from `agent_cancel.is_cancelled()` because the stale-resume and
        // question-cancel paths also cancel the token for non-hang reasons.
        let mut watchdog_fired = false;

        'event_loop: loop {
            tokio::select! {
                event_opt = events_rx.recv() => {
                    let Some(ev) = event_opt else {
                        // Driver task exited without sending Exited (defensive — should not happen).
                        log!(
                            "[ClaudeCode] events_rx closed without AgentEvent::Exited for thread {}",
                            thread_id
                        );
                        break;
                    };
                    if let AgentEvent::Exited = ev {
                        // Final flush of any pending text
                        if !claude_text_buf.is_empty() {
                            let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                            if !delta.is_empty() {
                                self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), coding_agent: crate::runtime::CodingAgent::ClaudeCode },
                                    meta: meta.clone(),
                                }, "[ClaudeCode] CodingAgentTextStreamed (final flush on exit)").await;
                            }
                        }
                        if is_waiting {
                            // CC process exited after producing a Result — session is idle.
                            // Don't hold the ThreadGuard waiting for follow-ups. Instead,
                            // auto-commit (only when the last turn ended Generated, per
                            // `should_auto_commit_on_cleanup`), remove from sessions map,
                            // and return. The worktree and branch persist on disk so
                            // follow-ups can reuse them via a new run_direct_agent call.
                            // This makes engine shutdown instant (no idle loop to cancel).
                            log!("[ClaudeCode] CC process exited while idle — releasing thread {}", thread_id);

                            // Half-assed work (Failed / Canceled / Aborted / safety-net)
                            // skips the auto-commit so the post-commit hook doesn't fire
                            // a spurious ChangeProposed for partial work. should_discard
                            // is always false here (no user Discard click without breaking
                            // out via the stop arm); pass false to keep the gate purely
                            // about terminal kind.
                            if let Some(ref wt) = worktree_path {
                                if should_auto_commit_on_cleanup(false, &last_terminal_kind) {
                                    auto_commit_preserving_marker(&self.pool, wt, &repo_root, &branch_name, "Claude Code changes (auto-committed on idle exit)").await;
                                }
                            }

                            // Save slash commands to cache before removing session.
                            // Model/effort are persisted via CodingAgentSettingsChanged events.
                            let cache_snapshot = {
                                let mut guard = self.agent_sessions.lock().await;
                                let snapshot = if let Some(s) = guard.get_mut(&thread_id) {
                                    s.process_exited = true;
                                    s.idle_notify.notify_waiters();
                                    s.repo_root.as_ref().map(|r| {
                                        (r.to_string_lossy().to_string(), s.to_commands_info())
                                    })
                                } else {
                                    None
                                };
                                guard.remove(&thread_id);
                                snapshot
                            };
                            self.clear_cc_debounce(thread_id);
                            if let Some((repo_key, info)) = cache_snapshot {
                                self.upsert_cc_commands_cache(repo_key, info).await;
                            }

                            // Drain follow-ups that arrived between CodingAgentIdled
                            // and process exit. Convert to orphaned injections so the
                            // caller re-processes them instead of showing "interrupted".
                            let orphans = lost_followups_to_orphans(drain_lost_followups(&mut msg_rx));

                            return Ok(ProcessResult {
                                response: String::new(),
                                steps: vec![],
                                images,
                                request_id,
                                thread_id,
                                proposed_change,
                                auto_apply: false,
                                orphaned_injections: orphans,
                            });
                        }
                        log!(
                            "[ClaudeCode] CC exited without Result event for thread {} (buffered_text_len={})",
                            thread_id,
                            claude_text_buf.len()
                        );
                        break;
                    }
                    // Stamp liveness — used by apply_now's timeout
                    {
                        let guard = self.agent_sessions.lock().await;
                        if let Some(s) = guard.get(&thread_id) {
                            s.last_event_at.store(now_epoch_millis(), std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    match ev {
                        AgentEvent::Init { session_id: cc_sid, model: init_model, slash_commands: cmds, skills } => {
                            log!("[ClaudeCode] [TIMING] Init event received: {:?}", cc_start.elapsed());
                            // Enable --resume for follow-ups and engine restart
                            let cache_update = {
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) {
                                    s.cc_session_id = Some(cc_sid.clone());
                                    // Always update from Init — CC reports the actual
                                    // full model ID (e.g. "claude-opus-4-6"), which is
                                    // authoritative over any alias the user selected.
                                    if let Some(ref m) = init_model {
                                        // Reconcile against the originally-supplied alias so the
                                        // [1m] suffix survives — CC strips it when echoing the
                                        // model id, and context_window_for keys on it for 1M.
                                        let norm = crate::runtime::claude_code::reconcile_cc_model(
                                            cc_model.as_deref(),
                                            m,
                                        );
                                        s.current_model = Some(norm.clone());
                                        normalized_model = Some(norm);
                                    }
                                    let skill_set: std::collections::HashSet<&str> = skills.iter().map(String::as_str).collect();
                                    s.builtin_commands = cmds.into_iter()
                                        .filter(|c: &String| !skill_set.contains(c.as_str()))
                                        .collect();
                                    s.skill_commands = skills;
                                    Some(s.to_commands_info())
                                } else {
                                    None
                                }
                            };
                            // Update per-repo cache outside sessions lock to avoid nested locks
                            if let Some(info) = cache_update {
                                let repo_key = repo_root.to_string_lossy().to_string();
                                self.upsert_cc_commands_cache(repo_key, info).await;
                            }
                            // Persist the CC session id (and authoritative model) the
                            // instant CC reports them at Init. Emitted unconditionally
                            // — even when the model is unchanged — so the session id is
                            // durable in the event store *before* the first
                            // CodingAgentIdled. A mid-turn engine restart can then still
                            // `--resume` the conversation: the resume/recovery lookups
                            // read `cc_session_id` from this event as well as from
                            // CodingAgentIdled. Without it, a long turn interrupted before
                            // its first idle loses the id entirely and falls back to a
                            // fresh session with only a reconstructed summary.
                            if let Err(e) = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentSettingsChanged {
                                    model: normalized_model.clone(),
                                    reasoning_effort: cc_reasoning_effort.clone(),
                                    permission_mode: None,
                                    coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                                    cc_session_id: Some(cc_sid.clone()),
                                },
                                meta: meta.clone(),
                            }).await {
                                log!("[ClaudeCode] Failed to persist Init CodingAgentSettingsChanged for {}: {}", thread_id, e);
                            }
                            // Release startup semaphore — CC process is initialized and mostly idle now.
                            if let Some(permit) = startup_permit.take() {
                                drop(permit);
                                log!("[ClaudeCode] [TIMING] Startup semaphore released: {:?}", cc_start.elapsed());
                            }
                            if let Some(ref effort) = cc_reasoning_effort {
                                log!("[ClaudeCode] Setting initial reasoning effort: {}", effort);
                                if agent_control_tx
                                    .send(crate::runtime::ControlRequest::SetReasoningEffort {
                                        effort: effort.clone(),
                                    })
                                    .is_err()
                                {
                                    log!("[ClaudeCode] Failed to forward reasoning effort: agent control channel closed");
                                }
                            }
                        }
                        AgentEvent::Message { text, .. } => {
                            // CC resumed after waiting — clear waiting state
                            if is_waiting {
                                is_waiting = false;
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                            }
                            claude_text_buf.push_str(&text);
                            // Persist + broadcast at natural boundaries
                            if should_flush(&claude_text_buf) {
                                let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                                if !delta.is_empty() {
                                    self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                        thread_id,
                                        event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), coding_agent: crate::runtime::CodingAgent::ClaudeCode },
                                        meta: meta.clone(),
                                    }, "[ClaudeCode] CodingAgentTextStreamed (Message flush)").await;
                                    last_text_persisted_len = claude_text_buf.len();
                                }
                            }
                        }
                        AgentEvent::ToolUse { name, input, id } => {
                            // CC resumed after waiting — clear waiting state
                            if is_waiting {
                                is_waiting = false;
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                            }
                            {
                                let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                                if !delta.is_empty() {
                                    self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                        thread_id,
                                        event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), coding_agent: crate::runtime::CodingAgent::ClaudeCode },
                                        meta: meta.clone(),
                                    }, "[ClaudeCode] CodingAgentTextStreamed (pre-ToolUse flush)").await;
                                    last_text_persisted_len = claude_text_buf.len();
                                }
                            }
                            if !claude_text_buf.is_empty() {
                                claude_text_buf.push_str("\n\n");
                            }
                            // Disarm the watchdog while ANY tool runs — including
                            // AskUserQuestion. The user might take ten minutes to
                            // pick an answer, and we must not euthanize the session
                            // for that. The matching ToolResult arm decrements.
                            // `Relaxed` is fine — the only reader (the watchdog
                            // tick) tolerates a one-tick staleness, and the in-
                            // loop watchdog inside this same `select!` observes
                            // the value monotonically anyway.
                            tools_in_flight.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if name == "AskUserQuestion" {
                                // Handled by the PreToolUse hook in the Claude Code subprocess (see
                                // `crate::engine::cc_settings` and `api/internal.rs::ask_user_question`).
                                // CC stays alive; the hook blocks until the user answers, then injects
                                // a synthetic `tool_result` and CC continues. run_session has nothing to
                                // do with this `tool_use` event — no emit (the endpoint emits
                                // `UserQuestionAsked`), no kill (CC keeps running), no session removal.
                            } else {
                                let description = crate::core::describe_cc_tool(&name, &input);
                                // Safety net: env-side fix (`pg_env_vars` injected
                                // into the Claude Code subprocess env) keeps the password
                                // out of `psql` argv in the common case, but a
                                // hardcoded URI in a Bash command or Python script
                                // can still slip through. Walk every string in
                                // `args` and mask `postgres(ql)://user:pass@…`
                                // before the event reaches the store / SSE stream.
                                let mut input = input;
                                crate::core::redact_postgres_secrets_in_json(&mut input);
                                self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentToolCalled {
                                        name,
                                        args: input,
                                        description,
                                        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                                        tool_use_id: id,
                                    },
                                    meta: meta.clone(),
                                }, "[ClaudeCode] CodingAgentToolCalled").await;
                            }
                        }
                        AgentEvent::ToolResult { output, status: _, id } => {
                            let summary: String = output.chars().take(200).collect();
                            // Re-arm the watchdog if this was the last in-flight
                            // tool. Floor at 0 so an unpaired ToolResult (CC
                            // oddity / replay) can't underflow into negative,
                            // which would falsely re-arm the watchdog forever:
                            // `watchdog_gate` reads `tools_in_flight > 0` and
                            // would never gate-skip again. `Relaxed` matches the
                            // increment.
                            let prev = tools_in_flight
                                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                            if prev <= 0 {
                                tools_in_flight.store(0, std::sync::atomic::Ordering::Relaxed);
                            }
                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentToolResult {
                                    name: String::new(),
                                    result: summary,
                                    coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                                    tool_use_id: id,
                                },
                                meta: meta.clone(),
                            }, "[ClaudeCode] CodingAgentToolResult").await;
                        }
                        AgentEvent::Usage {
                            model: cc_msg_model,
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_creation_tokens,
                        } => {
                            // Sections stay empty — CC doesn't expose its
                            // system prompt or tool schemas via stream-json.
                            // CC strips the [1m] suffix on the per-message
                            // model echo too, so reconcile against
                            // normalized_model (which the Init handler keeps
                            // suffix-correct) before measuring the window.
                            let snapshot_model = cc_msg_model
                                .as_deref()
                                .map(|m| crate::runtime::claude_code::reconcile_cc_model(
                                    normalized_model.as_deref(),
                                    m,
                                ))
                                .or_else(|| normalized_model.clone())
                                .unwrap_or_default();
                            let context_window =
                                crate::engine::context::context_window_for(&snapshot_model);
                            // Anthropic reports `input_tokens` as the
                            // uncached portion only. `ApiUsage.input_tokens`
                            // stores the TOTAL prompt size — same convention
                            // as `vertex.rs:678` — so the budget bar shows
                            // real context use and the modal's cache-miss
                            // formula (`input - read - write`) recovers
                            // the uncached count. `saturating_add` defends
                            // against a pathologically large stream.
                            let total_input = input_tokens
                                .saturating_add(cache_read_tokens)
                                .saturating_add(cache_creation_tokens);
                            let estimated_total_tokens =
                                (total_input as usize) + (output_tokens as usize);
                            let usage = crate::engine::ApiUsage {
                                input_tokens: total_input,
                                output_tokens,
                                cache_read_tokens,
                                cache_creation_tokens,
                            };
                            self.event_bus
                                .emit_or_log(
                                    crate::engine::event_bus::BusEvent::Thread {
                                        thread_id,
                                        event: crate::engine::thread_events::ThreadEvent::ContextCaptured {
                                            producer: crate::engine::ContextProducer::ClaudeCode,
                                            model: snapshot_model,
                                            context_window,
                                            sections: Vec::new(),
                                            tools: Vec::new(),
                                            estimated_total_tokens,
                                            usage: Some(usage),
                                            trimmed: false,
                                        },
                                        meta: meta.clone(),
                                    },
                                    "[ClaudeCode] ContextCaptured",
                                )
                                .await;
                        }
                        AgentEvent::Exited => unreachable!("Exited handled above"),
                        AgentEvent::Result { text, error: cc_error, .. } => {
                                        let err_suffix = cc_error.as_deref().map(|e| format!(" (error: {})", e)).unwrap_or_default();
                                        log!("[ClaudeCode] Result event received — entering waiting state{}", err_suffix);
                                        // Final flush of any pending text
                                        if !claude_text_buf.is_empty() {
                                            // The Result.text may contain text beyond what was
                                            // streamed via Message events (CC sometimes bundles
                                            // trailing text into the Result without a preceding
                                            // Message). Append the extra to the buffer so the
                                            // frontend sees the complete text before entering waiting.
                                            // Mirrors the same logic in build_session_messages.
                                            let buf_trimmed = claude_text_buf.trim();
                                            let result_trimmed = text.trim();
                                            if !result_trimmed.is_empty()
                                                && result_trimmed.len() > buf_trimmed.len()
                                                && result_trimmed.starts_with(buf_trimmed)
                                            {
                                                let extra = result_trimmed[buf_trimmed.len()..].trim();
                                                if !extra.is_empty() {
                                                    claude_text_buf.push_str("\n\n");
                                                    claude_text_buf.push_str(extra);
                                                }
                                            }
                                            let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                                            if !delta.is_empty() {
                                                self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                    thread_id,
                                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), coding_agent: crate::runtime::CodingAgent::ClaudeCode },
                                                    meta: meta.clone(),
                                                }, "[ClaudeCode] CodingAgentTextStreamed (Result flush)").await;
                                            }
                                        } else if !text.trim().is_empty() {
                                            // Slash commands (e.g. /model) produce a Result
                                            // without any preceding Message events. Emit the
                                            // result text as CodingAgentTextStreamed so the
                                            // frontend displays it.
                                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: text.trim().to_string(), coding_agent: crate::runtime::CodingAgent::ClaudeCode },
                                                meta: meta.clone(),
                                            }, "[ClaudeCode] CodingAgentTextStreamed (slash command result)").await;
                                        }
                                        // Both buffers must be checked: the slash-command path emits
                                        // Result.text without buffering it, so `claude_text_buf` alone
                                        // would mis-flag /model output as empty.
                                        let result_text_empty = text.trim().is_empty();
                                        let buffered_text_empty = claude_text_buf.trim().is_empty();
                                        // Detect stale resume: CC returned empty Result immediately
                                        // after resume. The session was expired and produced no output.
                                        // Abort without emitting ResponseGenerated/CodingAgentIdled so
                                        // the caller can retry with a fresh session.
                                        if is_stale_resume_signal(
                                            resume_session_id.is_some(),
                                            result_text_empty,
                                            buffered_text_empty,
                                            result_texts.is_empty(),
                                            !user_message.is_empty(),
                                            cc_error.is_some(),
                                        ) {
                                            log!("[ClaudeCode] Stale resume detected — CC returned empty Result for non-empty user message. Aborting session for retry.");
                                            agent_cancel.cancel();
                                            // Remove from sessions map so retry can start fresh
                                            {
                                                let mut guard = self.agent_sessions.lock().await;
                                                if let Some(s) = guard.get_mut(&thread_id) {
                                                    s.process_exited = true;
                                                    s.idle_notify.notify_waiters();
                                                }
                                                guard.remove(&thread_id);
                                            }
                                            // Shadow the stale CodingAgentIdled so
                                            // `resolve_resume_context` won't reuse the dead
                                            // sid on the retry (or after a restart). See
                                            // SessionEndReason::StaleResume.
                                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: crate::engine::thread_events::ThreadEvent::SessionEnded {
                                                    reason: SessionEndReason::StaleResume,
                                                },
                                                meta: meta.clone(),
                                            }, "[ClaudeCode] SessionEnded (stale resume)").await;
                                            // Clean up the worktree and branch so the retry
                                            // starts fresh (otherwise orphaned on disk until
                                            // engine restart).
                                            if let Some(ref wt) = worktree_path {
                                                let wt_str = wt.to_string_lossy();
                                                let _ = git_cmd(&["worktree", "remove", "--force", &wt_str], &repo_root).await;
                                            }
                                            let _ = git_cmd(&["branch", "-D", &branch_name], &repo_root).await;
                                            return Err(STALE_RESUME_ERROR.into());
                                        }

                                        result_texts.push(text.clone());
                                        // Single read of shutting_down — both the terminal-event
                                        // and the skip-idle decisions must agree on its value.
                                        let is_shutdown = shutting_down
                                            .load(std::sync::atomic::Ordering::Relaxed);
                                        let (terminal_kind, emit_idle) = classify_result(
                                            is_silent_resume(user_message.is_empty(), has_user_images),
                                            user_hit_stop,
                                            is_shutdown,
                                            cc_error,
                                            buffered_text_empty && result_text_empty,
                                        );
                                        // Capture before the `if let Some(kind)` below moves out —
                                        // `should_propose_change_at_idle` and the post-loop cleanup
                                        // both read this to refuse half-assed work.
                                        last_terminal_kind = terminal_kind.clone();
                                        if let Some(kind) = terminal_kind {
                                            if matches!(kind, TerminalKind::Aborted(_)) {
                                                // Reset on next user follow-up.
                                                user_hit_stop = false;
                                            }
                                            if !Self::external_terminal_already_emitted(&external_terminal_emitted, thread_id, "Result classify") {
                                                let terminal_event = Self::make_terminal_event(
                                                    kind,
                                                    text.clone(),
                                                    normalized_model.clone(),
                                                    cc_reasoning_effort.clone(),
                                                );
                                                self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                    thread_id,
                                                    event: terminal_event,
                                                    meta: meta.clone(),
                                                }, "[ClaudeCode] terminal event (Result classify)").await;
                                            }
                                        }
                                        emitted_terminal_event = true;
                                        claude_text_buf.clear();
                                        last_text_persisted_len = 0;
                                        // Auto-commit any dirty files before checking for changes.
                                        // CC may create/edit files via Bash without committing. Without
                                        // this, the three-dot diff below sees no committed changes and
                                        // wt_has_changes is false, preventing ChangeProposed from firing.
                                        if let Some(ref wt) = worktree_path {
                                            auto_commit_preserving_marker(&self.pool, wt, &repo_root, &branch_name, "Claude Code changes (auto-committed)").await;
                                        }
                                        // Check for worktree changes before entering waiting.
                                        // Use three-dot merge-base diff (main...HEAD) so we only see
                                        // changes introduced ON this branch, not changes main received
                                        // after the branch was created. Without this, a branch whose
                                        // changes were already merged appears to have changes because
                                        // main moved ahead.
                                        let (wt_has_changes, wt_requires_restart) = if conflict_change.is_some() {
                                            (true, false) // Conflict resolution always has work
                                        } else {
                                            // Reuse branch_changed_files so the runtime-path filter
                                            // applies here too — without it `coding_agent_proposed` would
                                            // flip to true whenever `.lucidos/` files were committed.
                                            let changed_files = branch_changed_files(&repo_root, &branch_name).await;
                                            (!changed_files.is_empty(), files_require_restart(&changed_files))
                                        };

                                        // Defensive: if this worktree has no changes but a previous
                                        // CodingAgentIdled on the same thread had has_changes:true
                                        // (without an intervening apply/discard/end), carry forward.
                                        // This prevents a text-only follow-up from erasing the change
                                        // state when the changes still exist on the original branch.
                                        let (wt_has_changes, wt_requires_restart) = if !wt_has_changes {
                                            let q = format!(
                                                "SELECT payload FROM events \
                                                 WHERE aggregate_id = $1 AND event_type IN ({}) \
                                                 ORDER BY created DESC LIMIT 1",
                                                CC_TURN_CLOSER_EVENTS,
                                            );
                                            match sqlx::query_scalar::<_, serde_json::Value>(&q)
                                            .bind(thread_id.to_string())
                                            .fetch_optional(self.pool())
                                            .await {
                                                Ok(Some(payload)) => {
                                                    let prev_has = payload.get("has_changes").and_then(|v| v.as_bool()).unwrap_or(false);
                                                    let prev_restart = payload.get("requires_restart").and_then(|v| v.as_bool()).unwrap_or(false);
                                                    if prev_has {
                                                        // Verify the branch still has actual changes.
                                                        // A commit+revert leaves the previous idle's
                                                        // has_changes=true stale, causing a phantom
                                                        // "Apply" button with zero changed files.
                                                        let files = branch_changed_files(&repo_root, &branch_name).await;
                                                        if files.is_empty() {
                                                            log!("[ClaudeCode] Carry-forward skipped — branch has no actual diff (likely commit+revert)");
                                                            (false, false)
                                                        } else {
                                                            log!("[ClaudeCode] Carrying forward has_changes=true from previous idle (worktree diff was empty)");
                                                            (prev_has, prev_restart)
                                                        }
                                                    } else {
                                                        (false, false)
                                                    }
                                                }
                                                _ => (false, false),
                                            }
                                        } else {
                                            (wt_has_changes, wt_requires_restart)
                                        };

                                        is_waiting = true;
                                        {
                                            let mut sessions = self.agent_sessions.lock().await;
                                            if let Some(s) = sessions.get_mut(&thread_id) {
                                                s.is_waiting = true;
                                                s.has_changes = wt_has_changes;
                                                s.requires_restart = wt_requires_restart;
                                                // Notify anyone waiting for idle (e.g. send_and_wait,
                                                // apply_now conflict resolution). Without this,
                                                // idle_notify only fires on EOF/process exit,
                                                // causing send_and_wait to hang indefinitely.
                                                s.idle_notify.notify_waiters();
                                            }
                                        }
                                        // `bg_bash_running` reflects the chat-agent's
                                        // `run_bash_background` tool (`BackgroundBashRegistry`).
                                        // It no longer gates the propose decision — it only keeps
                                        // CC alive at idle (via `terminate_decision` below) so
                                        // `spawn_bash_completion_watcher` can push a resume prompt
                                        // when the bash finishes. It's also recorded on the
                                        // `CodingAgentIdled` payload as `bg_bash_pending` for the
                                        // event history (no longer projected or gated — see the
                                        // field doc on `ThreadEvent::CodingAgentIdled`).
                                        let bg_bash_running = self
                                            .bash_background
                                            .has_running_for_thread(thread_id)
                                            .await;

                                        // Empty message (silent resume / warm-up): the previous
                                        // CodingAgentIdled already has the correct cc_session_id.
                                        // Shutdown: emitting idle would make recover_orphaned_worktrees
                                        // skip this session as "truly idle" and break recovery.
                                        if emit_idle {
                                            self.emit_coding_agent_idled(
                                                thread_id,
                                                CodingAgentIdleSnapshot {
                                                    has_changes: wt_has_changes,
                                                    is_external_repo,
                                                    requires_restart: wt_requires_restart,
                                                    bg_bash_pending: bg_bash_running,
                                                    worktree_path: worktree_path.as_deref(),
                                                },
                                                &meta,
                                            ).await;
                                            last_emitted_idle = true;
                                        }

                                        // Propose the change at idle time so the Apply button
                                        // shows immediately (propose_change deduplicates). When
                                        // CC skipped /harden, hardened=false propagates to the
                                        // change record and Apply runs hardening at click time.
                                        // Background bash deliberately does NOT gate this — see
                                        // `should_propose_change_at_idle` for the rationale and
                                        // the shutdown / external-repo / conflict-session guards.
                                        if should_propose_change_at_idle(
                                            wt_has_changes,
                                            is_external_repo,
                                            is_shutdown,
                                            conflict_change.is_some(),
                                            &last_terminal_kind,
                                        ) {
                                            let hardened = is_harden_marker_present(&self.pool, &repo_root, &branch_name).await;
                                            let changed_files = branch_changed_files(&repo_root, &branch_name).await;
                                            if changed_files.is_empty() {
                                                // Branch had worktree-level dirt at idle but the committed diff
                                                // against main is empty (commit + revert, or noise that auto-commit
                                                // captured then a subsequent edit reverted). Leave any pending row
                                                // for the user to resolve from Review — never auto-discard.
                                                match self.changes().get_pending_by_branch(&branch_name).await {
                                                    Ok(Some(stale)) => {
                                                        log!("[ClaudeCode] Branch {} has no actual diff but pending change {} exists — left in Review for user to resolve", branch_name, stale.id);
                                                    }
                                                    Ok(None) => {
                                                        log!("[ClaudeCode] Skipping proposal — branch has no changed files");
                                                    }
                                                    Err(e) => {
                                                        log!("[ClaudeCode] get_pending_by_branch({}): {} — skipping proposal", branch_name, e);
                                                    }
                                                }
                                                self.broadcast_changes_updated().await;
                                            } else {
                                                let requires_restart = files_require_restart(&changed_files);
                                                let fallback = change_description_fallback(self.pool(), thread_id, &branch_name).await;
                                                let base = default_local_branch(&repo_root).await;
                                                let log_range = format!("{}..{}", base, branch_name);
                                                let description = describe_branch_changes(&repo_root, &log_range, &fallback, None).await;
                                                let repo_root_str = repo_root.to_string_lossy().to_string();
                                                match self.propose_change(crate::engine::change_ops::ProposeChangeInput {
                                                    thread_id,
                                                    branch_name: &branch_name,
                                                    repo_root: &repo_root_str,
                                                    description: &description,
                                                    files: &changed_files,
                                                    requires_restart,
                                                    channel: EventChannel::ClaudeCode,
                                                    hardened,
                                                    // Live agent proposal — origin is
                                                    // carried by the surrounding
                                                    // MessageReceived. Engine-internal
                                                    // recovery paths stamp Engine origin
                                                    // via propose_branch_changes.
                                                    origin: None,
                                                    // Always false now: `should_propose_change_at_idle`
                                                    // refuses every non-Generated terminal, so partial
                                                    // work never reaches this point. The field stays in
                                                    // the event for backward compat with persisted rows.
                                                    incomplete: false,
                                                }).await {
                                                    Ok(_) => {
                                                        // Track for the ProcessResult returned via the
                                                        // Exited arm. Every idle exits the subprocess,
                                                        // so the post-loop cleanup that used to set
                                                        // this is skipped now.
                                                        proposed_change = true;
                                                    }
                                                    Err(e) => {
                                                        log!("[ClaudeCode] Failed to propose change at idle: {}", e);
                                                    }
                                                }
                                                self.broadcast_changes_updated().await;
                                            }
                                        }

                                        match idle_action(conflict_change.is_some(), is_shutdown) {
                                            IdleAction::EndSession => {
                                                log!("[ClaudeCode] Conflict-resolution session idle for thread {} — ending loop", thread_id);
                                                break 'event_loop;
                                            }
                                            IdleAction::ExitSubprocess => {
                                                // `swap(0)` resets per-Result (turn boundary).
                                                // AcqRel pairs with the fast-path `fetch_add` in
                                                // `chat::process` so a racing increment is
                                                // observed. The pure decision lives in
                                                // `terminate_decision` so the precedence rules
                                                // and the per-reason log line both read from one
                                                // place (see `TerminateDecision` variants).
                                                let prev = pending_followups
                                                    .swap(0, std::sync::atomic::Ordering::AcqRel);
                                                match terminate_decision(
                                                    prev,
                                                    bg_bash_running,
                                                ) {
                                                    TerminateDecision::KeepAliveForFollowup { inflight } => {
                                                        log!("[ClaudeCode] Skipping subprocess termination for thread {} — {} follow-up(s) inflight (queued or merged)", thread_id, inflight);
                                                    }
                                                    TerminateDecision::KeepAliveForBgBash => {
                                                        log!("[ClaudeCode] Skipping subprocess termination for thread {} — background bash still running (auto-wake will resume CC on completion)", thread_id);
                                                    }
                                                    TerminateDecision::Terminate => {
                                                        log!("[ClaudeCode] Idle reached — terminating Claude Code subprocess for thread {} so next turn resumes via --resume", thread_id);
                                                        agent_cancel.cancel();
                                                    }
                                                }
                                            }
                                            IdleAction::Nothing => {}
                                        }
                                    }
                                }
                }

                Some(user_input) = msg_rx.recv() => {
                    reset_per_turn_flags(
                        &mut is_waiting,
                        &mut last_emitted_idle,
                        &mut emitted_terminal_event,
                        &mut user_hit_stop,
                        &mut last_terminal_kind,
                    );
                    {
                        let mut sessions = self.agent_sessions.lock().await;
                        if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                    }
                    if !claude_text_buf.is_empty() {
                        let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                        if !delta.is_empty() {
                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), coding_agent: crate::runtime::CodingAgent::ClaudeCode },
                                meta: meta.clone(),
                            }, "[ClaudeCode] CodingAgentTextStreamed (flush before user_input)").await;
                        }
                        claude_text_buf.clear();
                        last_text_persisted_len = 0;
                    }

                    let images = user_input.images.clone().unwrap_or_default();
                    let input_kind = user_input.kind;
                    if agent_input_tx.send(AgentInput {
                        text: user_input.text.clone(),
                        images,
                    }).is_err() {
                        log!("[AgentSession] Failed to forward user input to agent runtime — channel closed");
                        break;
                    }
                    // `WakeFromChild` suppresses our emit — see `AgentInputKind`
                    // docs; the parent's `ChildThreadCompleted` is the start.
                    if matches!(input_kind, crate::engine::AgentInputKind::User) {
                        self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::CodingAgentPromptSent {
                                text: user_input.text,
                                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                                // Audit trail for a user-driven prompt — origin is
                                // carried by the MessageReceived emitted at the API
                                // boundary.
                                origin: None,
                            },
                            meta: meta.clone(),
                        }, "[ClaudeCode] CodingAgentPromptSent").await;
                    }
                }

                _ = interrupt.notified() => {
                    // Stop button = Esc in Claude Code CLI.
                    // Sends control_request:interrupt → CC stops current work, emits
                    // a Result, and goes idle. We set user_hit_stop so the Result
                    // handler emits ResponseCanceled (→ exchange "Canceled") instead
                    // of ResponseGenerated. CodingAgentIdled still follows, keeping
                    // the thread in "Waiting" state (CC is alive).
                    // During shutdown, the post-loop cleanup bails out early
                    // (no worktree removal, no SessionEnded). The session resumes
                    // after restart via recover_orphaned_worktrees.
                    if !is_waiting {
                        user_hit_stop = true;
                        log!("[ClaudeCode] Sending control_request interrupt to CC process");
                        if agent_control_tx
                            .send(crate::runtime::ControlRequest::Interrupt)
                            .is_err()
                        {
                            log!("[ClaudeCode] Failed to forward interrupt: agent control channel closed");
                        }
                    }
                    // Don't break — let the loop continue to read the Result event
                }

                _ = stop.notified() => {
                    // Apply / Discard / Archive emit their own lifecycle terminator
                    // (`ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`); only
                    // a real `UserStop` (or no reason set — engine shutdown direct
                    // notify) lets `ResponseCanceled` through.
                    let is_shutdown = shutting_down.load(std::sync::atomic::Ordering::Relaxed);
                    let suppress_user_terminal = matches!(
                        self.pending_stop_reason(thread_id).await,
                        Some(StopReason::Apply | StopReason::Discard | StopReason::Archive),
                    );
                    self.emit_stop_terminal(
                        "stop",
                        thread_id,
                        is_waiting,
                        is_shutdown,
                        suppress_user_terminal,
                        &agent_cancel,
                        &claude_text_buf,
                        last_text_persisted_len,
                        &meta,
                        &external_terminal_emitted,
                        &normalized_model,
                        &cc_reasoning_effort,
                    ).await;
                    emitted_terminal_event = true;
                    break;
                }

                _ = chat_cancel.cancelled() => {
                    // Upstream chat handler cancelled (engine shutdown / request abort).
                    // No user-action context here — suppress flag is always false.
                    let is_shutdown = shutting_down.load(std::sync::atomic::Ordering::Relaxed);
                    self.emit_stop_terminal(
                        "chat_cancel",
                        thread_id,
                        is_waiting,
                        is_shutdown,
                        false,
                        &agent_cancel,
                        &claude_text_buf,
                        last_text_persisted_len,
                        &meta,
                        &external_terminal_emitted,
                        &normalized_model,
                        &cc_reasoning_effort,
                    ).await;
                    emitted_terminal_event = true;
                    break;
                }

                _ = tokio::time::sleep(std::time::Duration::from_secs(WATCHDOG_TICK_INTERVAL_SECS)) => {
                    // Hung-subprocess watchdog. Any incoming event re-arms the
                    // sleep via the surrounding select!, so only a fully silent
                    // loop reaches here. On fire, the safety net at the bottom
                    // of run_session reads `watchdog_fired` and emits
                    // `ContinuationRequested{auto_recovery_after_hang}` instead of an
                    // abort, so the spawn dispatcher boots a fresh `--resume`
                    // without user intervention. Diagnostic line fires on any
                    // non-NotStale gate past the threshold so post-mortems can
                    // pin which gate held — the May-2026 incident lacked this.
                    let (last_ms, session_present) = {
                        let guard = self.agent_sessions.lock().await;
                        match guard.get(&thread_id) {
                            Some(s) => (
                                s.last_event_at.load(std::sync::atomic::Ordering::Relaxed),
                                true,
                            ),
                            None => (0, false),
                        }
                    };
                    let now_ms = now_epoch_millis();
                    let tools_in_flight_snapshot =
                        tools_in_flight.load(std::sync::atomic::Ordering::Relaxed);
                    let gate = watchdog_gate(
                        is_waiting,
                        last_ms,
                        now_ms,
                        WATCHDOG_INACTIVITY_LIMIT_MS,
                        tools_in_flight_snapshot,
                    );
                    // last_ms <= 0 routes to SkipBadHeartbeat (no fire) and
                    // would print epoch millis as elapsed — skip the diag.
                    // The Fire branch below is reachable only when last_ms > 0
                    // (the gate's heartbeat check enforces this), so logging
                    // the elapsed there is always meaningful.
                    if last_ms > 0 {
                        let elapsed_ms = now_ms.saturating_sub(last_ms);
                        if elapsed_ms >= WATCHDOG_DIAG_LOG_THRESHOLD_MS
                            && gate != WatchdogGate::NotStale
                        {
                            log!(
                                "[ClaudeCode] [Watchdog DIAG] thread={} elapsed={}s is_waiting={} tools_in_flight={} session_present={} gate={}",
                                thread_id,
                                elapsed_ms / 1000,
                                is_waiting,
                                tools_in_flight_snapshot,
                                session_present,
                                gate.diag_tag(),
                            );
                        }
                        if gate == WatchdogGate::Fire {
                            log!(
                                "[ClaudeCode] Watchdog: no events for {}s while mid-turn AND no tool in flight — killing Claude Code subprocess for thread {} (suspected network outage / hung API call); will auto-resume via ContinuationRequested once events_rx EOFs",
                                elapsed_ms / 1000,
                                thread_id
                            );
                            agent_cancel.cancel();
                            watchdog_fired = true;
                        }
                    }
                }
            }
        }

        self.finalize_direct_agent(
            thread_id,
            request_id,
            &meta,
            conflict_change,
            cwd,
            repo_root,
            branch_name,
            worktree_path,
            images,
            msg_rx,
            claude_text_buf,
            normalized_model,
            cc_reasoning_effort,
            last_terminal_kind,
            external_terminal_emitted,
            &agent_cancel,
            emitted_terminal_event,
            watchdog_fired,
            last_emitted_idle,
            is_external_repo,
            proposed_change,
        )
        .await
    }
}
