use super::idle_snapshot::CodingAgentIdleSnapshot;
use super::spawn_context::SpawnWorktreeContext;
use crate::engine::agentic_loop::should_flush;
use crate::engine::change_ops::now_epoch_millis;
use crate::engine::claude_code::STALE_RESUME_ERROR;
use crate::engine::git_ops::{
    auto_commit_preserving_marker, branch_changed_files, default_local_branch,
    describe_branch_changes, files_require_restart, is_external_repo_path,
    is_harden_marker_present, main_worktree,
};
use crate::engine::thread_events::{EventChannel, SessionEndReason};
use crate::engine::{AgentSession, AgentUserInput, LucidosEngine, ProcessResult, StopReason};
use crate::runtime::{AgentEvent, AgentInput, CodingAgent};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

use crate::engine::agent_session::io_helpers::{drain_lost_followups, lost_followups_to_orphans};
use crate::engine::agent_session::lifecycle::{
    classify_result, idle_action, is_definitive_session_not_found, is_resume_settle_result,
    is_silent_resume, is_stale_resume_signal, may_touch_change_state_at_idle, reset_per_turn_flags,
    should_auto_commit_on_cleanup, terminal_clears_user_hit_stop, terminate_decision,
    watchdog_gate, IdleAction, StaleResumeInputs, TerminalKind, TerminateDecision, WatchdogGate,
    WATCHDOG_DIAG_LOG_THRESHOLD_MS, WATCHDOG_HUNG_TOOL_CEILING_MS, WATCHDOG_INACTIVITY_LIMIT_MS,
    WATCHDOG_TICK_INTERVAL_SECS,
};
use crate::engine::agent_session::resume::{
    change_description_fallback, default_claude_config_dir, resolve_resume_context,
    CC_TURN_CLOSER_EVENTS,
};
use crate::engine::agent_session::spawn::spawn_or_resume;

/// Decrement the paired-tool counter, flooring at 0. An unpaired decrement
/// going negative would permanently disarm the hang watchdog —
/// `watchdog_gate` reads `tools_in_flight > 0` and would never gate-skip
/// again — so every decrement site (the ToolResult arm and the
/// permission-bridge waiter task) shares this one contract. `Relaxed`
/// matches the increments: the only reader tolerates one-tick staleness.
fn release_tool_slot(tools_in_flight: &std::sync::atomic::AtomicI32) {
    let prev = tools_in_flight.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    if prev <= 0 {
        tools_in_flight.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The coding-agent driver dropped its input receiver before the engine could
/// send the first prompt — which only happens when the driver task already
/// wound down (the agent process failed to start, exited immediately, or its
/// handshake failed). In every such path the driver flushes its REAL cause onto
/// `events_rx` first — a `Result { error }` (Codex spawn / handshake failure, a
/// CC `error_during_execution` line) or, when the process simply died, an
/// `Exited { killed_by_signal }`. Recover it so the thread surfaces an
/// actionable reason instead of the old bare "input channel closed", which
/// named neither what failed nor why.
///
/// Non-blocking on purpose: by the time the receiver-drop is observed the driver
/// has returned, so every event it will ever emit is already buffered in the
/// unbounded channel — `try_recv` drains the full tail without awaiting.
fn drain_startup_failure_reason(
    events_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
) -> Option<String> {
    let mut reason: Option<String> = None;
    let mut killed_by_signal = false;
    while let Ok(ev) = events_rx.try_recv() {
        match ev {
            // Last non-empty error wins — the driver emits the specific cause
            // before the terminal Exited.
            AgentEvent::Result { error: Some(e), .. } if !e.trim().is_empty() => {
                reason = Some(e);
            }
            AgentEvent::Exited {
                killed_by_signal: k,
            } => killed_by_signal = k,
            _ => {}
        }
    }
    reason.or_else(|| {
        killed_by_signal
            .then(|| "coding agent process was killed by a signal during startup".to_string())
    })
}

/// Where a Lucidos-**source** session roots when no `Lucidos` repository row
/// resolved and the spawn isn't an app spawn.
///
/// `dev_root` comes from [`main_worktree`], which falls back to the process cwd
/// when there is no source checkout — on a packaged install that is the
/// **workspace** directory, itself a git repo. Returning it there would branch
/// the user's workspace git and label it platform source: exactly the silent
/// mis-rooting that let a packaged install report "Claude Code session started"
/// for a request to change Lucidos itself.
///
/// So the fallback survives only for its real case — a dev build whose registry
/// row hasn't been written yet (very early startup) — and is a hard refusal
/// otherwise. Backstops every caller; the `run_coding_agent` tool refuses the
/// same case earlier so the model sees it in-turn.
///
/// Pure over `has_lucidos_source` so both branches are testable without a
/// packaged binary. See `docs/plans/2026-07-29-no-lucidos-source-agent-context.md`.
fn unregistered_lucidos_root(
    dev_root: PathBuf,
    has_lucidos_source: bool,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if has_lucidos_source {
        return Ok(dev_root);
    }
    Err(crate::engine::agent_session::err_no_lucidos_source())
}

impl LucidosEngine {
    /// Flush any reasoning accumulated in `buf` past `last_len` as a
    /// `CodingAgentThoughtStreamed`, advancing `last_len`. Idempotent (emits only
    /// the new tail; no-op when nothing new). Coalesces CC's per-token
    /// `thinking_delta`s — and Codex's reasoning deltas — into a handful of rows
    /// per turn, mirroring the assistant-text flush path. The buffer is sliced on
    /// a char boundary so multi-byte reasoning never panics.
    async fn flush_coding_agent_thought(
        &self,
        thread_id: Uuid,
        buf: &str,
        last_len: &mut usize,
        coding_agent: crate::runtime::CodingAgent,
        meta: &crate::engine::thread_events::EventMeta,
    ) {
        if buf.len() <= *last_len {
            return;
        }
        let delta = &buf[buf.floor_char_boundary(*last_len)..];
        if delta.is_empty() {
            return;
        }
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentThoughtStreamed {
                        text: delta.to_string(),
                        coding_agent,
                    },
                    meta: meta.clone(),
                },
                "[AgentSession] CodingAgentThoughtStreamed",
            )
            .await;
        *last_len = buf.len();
    }

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
        // Backend requested by the caller (chat API / spawn tool). Only
        // honored for a thread's FIRST session — afterwards the stored
        // `thread_summaries.coding_agent` wins so a thread can never flip
        // backends mid-conversation (the new backend has no session to
        // resume and would silently lose all context).
        requested_coding_agent: Option<CodingAgent>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let cc_start = std::time::Instant::now();
        let thread_id_str = thread_id.to_string();

        let coding_agent = {
            let stored: Option<String> = sqlx::query_scalar(
                "SELECT coding_agent FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_optional(&self.pool)
            .await?
            .flatten();
            match stored {
                Some(s) => {
                    let stored_agent = CodingAgent::parse(&s);
                    if let Some(req) = requested_coding_agent {
                        if req != stored_agent {
                            log!(
                                "[AgentSession] thread {} is locked to {:?} — ignoring requested backend {:?}",
                                thread_id,
                                stored_agent,
                                req
                            );
                        }
                    }
                    stored_agent
                }
                None => requested_coding_agent.unwrap_or(CodingAgent::ClaudeCode),
            }
        };
        let user_device_preferences_context =
            crate::engine::agent_context::build_user_device_preferences_context_for_origin(
                &self.pool,
                &self.event_store,
                origin_id,
            )
            .await;

        // Pre-spawn app-coding-agent-thread detection: `spawn_agent_thread`
        // stashes the app id here when the LLM picks `folder=data/apps/<id>`;
        // pop it once so the worktree dispatcher routes to sparse-checkout
        // and the system-prompt selector picks the app variant. Falls back
        // to thread_summaries on resume, so a follow-up message on an
        // existing app thread still routes correctly after the pending
        // stash was cleared by the initial spawn.
        let app_spawn_id: Option<String> = {
            let mut guard = self
                .pending_app_spawn
                .lock()
                .expect("pending_app_spawn poisoned");
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
                            "[AgentSession] thread {} has coding_agent_kind='app' but NULL folder — \
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

        // Mutable so the interrupt arm can stamp `meta.actor` with the device
        // that clicked Cancel — the terminal `ResponseCanceled` then carries it
        // for the Initiator popover's Device row. Cleared at the next turn
        // boundary so a resumed session doesn't inherit a stale actor.
        let mut meta = crate::engine::thread_events::EventMeta {
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
                // `is_live`, not `!process_exited`: a run future that was
                // dropped instead of completed leaves the entry behind with
                // that flag still false. Refusing the resume on a phantom is
                // what wedged thread 293f96d5 on 2026-07-28 — every follow-up
                // came back "A coding agent is already running for this
                // thread" with no subprocess anywhere on the box.
                if session.is_live() {
                    if session.is_waiting {
                        // Session is idle — route follow-up via msg_tx. The caller
                        // already emitted MessageReceived with the frontend UUID.
                        log!("[AgentSession] Session already running and idle — routing follow-up via msg_tx");
                        let images = user_images.map(|imgs| imgs.to_vec());
                        session
                            .pending_followups
                            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        if session
                            .msg_tx
                            .send(AgentUserInput {
                                text: crate::engine::agent_context::prepend_user_device_preferences_context(
                                    &user_device_preferences_context,
                                    user_message,
                                ),
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
                            return Err("Coding agent session ended while routing message. Please try again.".into());
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
                    return Err(crate::engine::claude_code::AGENT_ALREADY_RUNNING_ERROR.into());
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
                            "A coding-agent session was just started — ignoring duplicate request."
                                .into(),
                        );
                    }
                }
            }
            // Prune expired entries to prevent unbounded growth
            spawns.retain(|_, t| t.elapsed() < std::time::Duration::from_secs(10));
            spawns.insert(thread_id, std::time::Instant::now());
        }

        // The thread's pinned account — the config dir of its FIRST session.
        // Computed once here and injected on EVERY spawn below (see
        // `inject_config_dir`) so a live `CLAUDE_CONFIG_DIR` toggle can never move
        // an existing thread to another provider; it also scopes the auto-detected
        // resume session id to this account.
        let pinned_config_dir =
            crate::engine::agent_session::lookup_pinned_cc_config_dir(self.pool(), thread_id).await;
        let (resume_session_id, resume_branch) =
            if recovery_worktree.is_none() && conflict_change_id.is_none() {
                resolve_resume_context(
                    self.pool(),
                    self.changes(),
                    thread_id,
                    resume_session_id,
                    pinned_config_dir.as_deref(),
                )
                .await
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
            "[AgentSession] [TIMING] resume lookup: {:?}",
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

        // Coding-agent sessions must branch from main, not from a stale worktree branch.
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

        let (repo_id, repo_root, is_external_repo, external_repo_name, repo_name) = if is_app_spawn
        {
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
            // No repo resolved and not an app spawn — i.e. "edit Lucidos
            // itself". Only legitimate when a source checkout exists.
            (
                None,
                unregistered_lucidos_root(dev_root, crate::paths::has_lucidos_source())?,
                false,
                None,
                None,
            )
        };

        let workspace_name = self.workspace_name();
        let last_idle_sha = crate::engine::agent_session::resume::lookup_latest_worktree_head_sha(
            self.pool(),
            thread_id,
        )
        .await;
        let SpawnWorktreeContext {
            cwd,
            system_prompt,
            branch_name,
            worktree_path,
            interactive_session,
            adoption_note,
            resume_session_id,
            worktree_created,
            branch_created,
        } = self
            .resolve_run_worktree_context(
                recovery_worktree,
                &conflict_change,
                system_prompt_override,
                &app_spawn_id,
                is_app_spawn,
                is_external_repo,
                external_repo_name,
                repo_name.clone(),
                &workspace_name,
                &repo_root,
                &repo_id,
                &last_idle_sha,
                resume_worktree_path,
                resume_branch,
                resume_session_id,
                thread_id,
                cc_start,
                coding_agent,
            )
            .await?;

        let system_prompt = if user_device_preferences_context.is_empty() {
            system_prompt
        } else {
            format!("{}\n\n{}", system_prompt, user_device_preferences_context)
        };

        // Append thread history as context so new coding-agent sessions in an existing thread
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
                            "Coding agent"
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
            .or_else(|| {
                // The Claude Code settings files are a CC-only fallback —
                // their effort vocabulary must not leak into other backends.
                (coding_agent == CodingAgent::ClaudeCode)
                    .then(crate::runtime::claude_code::read_cc_default_effort)
                    .flatten()
            });

        // Acquire startup semaphore — limits concurrent CC process initializations.
        // Hold the permit until Init event is received (process is initialized and mostly idle).
        let startup_permit = self
            .cc_startup_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("Startup semaphore closed: {}", e))?;
        log!(
            "[AgentSession] [TIMING] Startup semaphore acquired: {:?}",
            cc_start.elapsed()
        );

        if resume_session_id.is_some() {
            log!("[AgentSession] Resuming session for thread {}", thread_id);
        }
        let agent_cancel = tokio_util::sync::CancellationToken::new();
        let allowed_tools = crate::engine::claude_code::cc_allowed_tools(self.user_dir());
        // User-managed env vars injected into the coding-agent subprocess
        // alongside the CRED_*/OAUTH_* the script path already gets. Applied
        // first in `apply_lucidos_env`, so engine-owned vars still win.
        let user_env_vars = crate::core::EnvironmentVariableStore::env_pairs(&self.pool)
            .await
            .unwrap_or_else(|e| {
                log!(
                    "[AgentSession] Failed to load user environment variables for spawn: {}",
                    e
                );
                Vec::new()
            });
        // Resolve the CLAUDE_CONFIG_DIR (provider/account) this session runs under.
        // A CC session's transcript lives at
        // `$CLAUDE_CONFIG_DIR/projects/<cwd>/<sid>.jsonl`, and the thread is PINNED
        // to the account of its FIRST session (`pinned_config_dir`, resolved above).
        //   * `inject_config_dir` — the engine-owned override, injected on EVERY
        //     spawn of an existing thread (resume, fresh, Tier-3 merge,
        //     post-stale-resume retry, recovery). This is what guarantees a thread
        //     never switches provider after turn 1: a live `CLAUDE_CONFIG_DIR`
        //     toggle change is ignored for any thread that already has a pin.
        //     `None` ONLY for the truly-first turn (no pin yet) — that spawn reads
        //     the live env and thereby establishes the pin. Injecting on resume
        //     also keeps `--resume` pointed at the dir CC wrote the transcript to
        //     (the dev/bf997e21 "No conversation found" fix).
        //   * `effective_config_dir` — the dir CC ACTUALLY runs under this spawn
        //     (the injected pin, else the user's live value on turn 1, else CC's
        //     default `$HOME/.claude`). Recorded at Init so the pin persists —
        //     including the unset→set case (turn 1 on the default, user later set a
        //     dir): the recorded default is re-injected on every later spawn.
        let inject_config_dir = pinned_config_dir.clone();
        let effective_config_dir = inject_config_dir.clone().or_else(|| {
            // Match CC's actual precedence for a fresh session so the recorded dir
            // never diverges from where CC writes the transcript: the user-managed
            // env var (DB table — `apply_lucidos_env` sets it over the inherited
            // env) wins, then the engine's own inherited process env (a shell that
            // exported CLAUDE_CONFIG_DIR without a DB row), then CC's default.
            user_env_vars
                .iter()
                .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
                .map(|(_, v)| v.clone())
                .or_else(|| {
                    std::env::var("CLAUDE_CONFIG_DIR")
                        .ok()
                        .filter(|v| !v.is_empty())
                })
                .or_else(default_claude_config_dir)
        });
        // User-configured agent binary path (Settings → Coding Agents).
        // Resolved here — the spawn orchestration has the pool — and validated
        // inside the runtime's spawn, which fails loud naming the setting on a
        // path that doesn't resolve (never a silent fallback to probing).
        let binary_override_key = match coding_agent {
            crate::runtime::CodingAgent::ClaudeCode => crate::core::PREF_CODING_AGENT_CLAUDE_PATH,
            crate::runtime::CodingAgent::Codex => crate::core::PREF_CODING_AGENT_CODEX_PATH,
        };
        let binary_override = crate::core::PreferenceStore::get(self.pool(), binary_override_key)
            .await
            .unwrap_or_else(|e| {
                log!(
                    "[AgentSession] Failed to load {} preference: {}",
                    binary_override_key,
                    e
                );
                None
            })
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let runtime = match spawn_or_resume(
            self,
            coding_agent,
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
                user_env_vars: &user_env_vars,
                binary_override: binary_override.as_deref(),
                // Override CLAUDE_CONFIG_DIR only on an actual resume (see
                // `inject_config_dir` above); a fresh session passes None so its
                // env / CC default is untouched.
                claude_config_dir: inject_config_dir.as_deref(),
                // Resume with no fresh input = the engine expects the agent
                // to pick up on its own (recovery / ContinuationRequested).
                // Mirrors the `has_content` gate below that skips the
                // initial input send.
                continuation: user_message.is_empty()
                    && user_images.is_none_or(|imgs| imgs.is_empty())
                    && resume_session_id.is_some(),
            },
            agent_cancel.clone(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                // Remove ONLY what this attempt created. The old shape ran
                // both git calls unconditionally under `let _ =` — a failed
                // RESUME force-removed the pre-existing worktree and
                // force-deleted the branch holding the thread's committed
                // work, silently.
                crate::engine::git_ops::cleanup_failed_spawn(
                    &repo_root,
                    worktree_path.as_deref(),
                    &branch_name,
                    worktree_created,
                    branch_created,
                )
                .await;
                return Err(format!("Failed to start the coding agent: {}", e).into());
            }
        };

        log!(
            "[AgentSession] [TIMING] CC process spawned: {:?}",
            cc_start.elapsed()
        );

        let crate::runtime::RunningAgent {
            mut events_rx,
            input_tx: agent_input_tx,
            control_tx: agent_control_tx,
            kind: _,
            // In-band approval requests (Codex app-server). `None` for CC and
            // the Codex exec escape hatch — the matching select arm below then
            // pends forever. Mutable so a closed channel can disable the arm.
            permission_rx: mut agent_permission_rx,
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
            let final_text = build_resume_prompt_text(
                &self.pool,
                thread_id,
                origin_id,
                user_message,
                ResumeSpawnContext {
                    worktree_path: worktree_path.as_deref(),
                    last_idle_sha: last_idle_sha.as_deref(),
                    adoption_note: adoption_note.as_deref(),
                    session_branch: Some(branch_name.as_str()),
                },
            )
            .await;

            if agent_input_tx
                .send(AgentInput {
                    text: final_text,
                    images,
                })
                .is_err()
            {
                // Same contract as the spawn_or_resume failure arm above:
                // remove ONLY what this attempt created — a resumed
                // worktree/branch holds the thread's committed work.
                crate::engine::git_ops::cleanup_failed_spawn(
                    &repo_root,
                    worktree_path.as_deref(),
                    &branch_name,
                    worktree_created,
                    branch_created,
                )
                .await;
                // The driver already wound down (its input receiver is gone),
                // so recover the real cause it flushed onto events_rx rather
                // than reporting a bare "channel closed" that hides what failed.
                let cause = drain_startup_failure_reason(&mut events_rx);
                return Err(match cause {
                    Some(c) => format!(
                        "Coding agent exited during startup before the first prompt could be sent: {c}"
                    ),
                    None => "Coding agent exited during startup before the first prompt could be sent"
                        .to_string(),
                }
                .into());
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
        let external_continuation_requested =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut normalized_model = cc_model.clone();
        // Initial input (when has_content) produces one expected `Result` event;
        // see AgentSession.pending_followups for the full rationale.
        let pending_followups =
            std::sync::Arc::new(std::sync::atomic::AtomicU32::new(if has_content {
                1
            } else {
                0
            }));
        // Cloned into the session struct so the external watchdog reads
        // the same atomic the loop below mutates.
        let tools_in_flight_shared = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
        {
            let mut sessions = self.agent_sessions.lock().await;
            let session = AgentSession {
                msg_tx: msg_tx.clone(),
                is_waiting: false,
                has_changes: false,
                requires_restart: false,
                pending_stop: None,
                cancel_actor: None,
                redirect_followup: false,
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
                external_continuation_requested: external_continuation_requested.clone(),
                control_tx: agent_control_tx.clone(),
                builtin_commands: prev_builtin,
                skill_commands: prev_skill,
                current_model: normalized_model.clone(),
                current_reasoning_effort: cc_reasoning_effort.clone(),
                pending_followups: pending_followups.clone(),
                question_resume_pending: false,
                tools_in_flight: tools_in_flight_shared.clone(),
                coding_agent,
                // Clone so the external watchdog can cancel from outside this
                // loop — the in-loop paths use the original `agent_cancel`.
                agent_cancel: agent_cancel.clone(),
            };
            sessions.insert(thread_id, session);
        }

        // From here on the entry is owned by this run. The guard is the
        // cancellation backstop: every *completion* path removes the entry
        // itself, but a dropped future runs none of them and would leave a
        // phantom that wedges the thread (see `entry_guard`). Declared after
        // `msg_rx` so it drops first — irrelevant for correctness (identity is
        // the channel, not liveness) but it keeps the reap ordered ahead of the
        // channel close.
        let _entry_guard = super::entry_guard::SessionEntryGuard::new(
            self.agent_sessions.clone(),
            thread_id,
            msg_tx.clone(),
            shutting_down.clone(),
            self.event_bus.clone(),
            self.pool.clone(),
        );

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
                    coding_agent,
                },
                meta: meta.clone(),
            })
            .await
        {
            log!(
                "[AgentSession] Failed to emit initial SessionStarted for {}: {}",
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
        // Outside the projection tx by design: `git diff` inside a
        // Postgres tx is the wrong shape. The helper logs and continues on
        // failure — bootstrap must not block on git or the projection write.
        // It writes the same git-truth the Diff button computes
        // (`branch_changed_files`), so the button's visibility and its rendered
        // diff stay in lockstep; the recovery sweep / next commit hook also
        // reconcile it.
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
                        coding_agent,
                        // Fires before CC's Init — the session id isn't known
                        // yet. The Init handler emits a second SettingsChanged
                        // carrying it AND the config dir (see below).
                        cc_session_id: None,
                        claude_config_dir: None,
                    },
                    meta: meta.clone(),
                })
                .await
            {
                log!(
                    "[AgentSession] Failed to persist initial CodingAgentSettingsChanged for {}: {}",
                    thread_id,
                    e
                );
            }
        }

        let mut result_texts: Vec<String> = Vec::new();
        // Count tool calls seen before the first `Result`. Feeds the
        // stale-resume heuristic (`is_stale_resume_signal`): a resumed turn that
        // made ANY tool call is alive and working — even when it produced no
        // assistant text (terse models like Fable). The stale check only fires
        // on the first Result (`result_texts.is_empty()`), so a session-scoped
        // counter is exactly "tool calls before the first Result." Prevents the
        // 2026-07-02 false stale-resume → duplicate-process bug.
        let mut tool_calls_seen: u32 = 0;
        // Count backend->model API calls seen before the first `Result`, read as
        // `no_api_call_this_turn` by `is_resume_settle_result`. A `Usage` event is
        // emitted per real API call and ONLY for one: the parser drops all-zero
        // usage frames, and a backend's `<synthetic>` message (the injected
        // `Continue from where you left off.` / `No response requested.` pair, a
        // drained task-notification) carries exactly that. So zero here is
        // positive proof no model call happened, which is what separates "this
        // Result closes the backend's own resume-settle turn" from "the model was
        // asked our prompt and answered with nothing". Those two shapes are
        // otherwise identical, and skipping the second would strand the turn until
        // the inactivity watchdog fired. Session-scoped for the same reason
        // `tool_calls_seen` is: the settle skip only fires on the first Result.
        let mut api_calls_seen: u32 = 0;
        // The session id the backend reported at `Init` — i.e. the conversation
        // it ACTUALLY attached to. Compared against `resume_session_id` to prove
        // a `--resume` landed on the live conversation, which vetoes the
        // empty-echo stale-resume heuristic outright (see `StaleResumeInputs::
        // resume_attach_confirmed`). This is the structured signal the heuristic's
        // temporary-measures row asks for; without it a healthy resume that says
        // nothing is indistinguishable from a dead one.
        let mut init_session_id: Option<String> = None;
        let mut claude_text_buf = String::new();
        let mut last_text_persisted_len: usize = 0;
        // Reasoning/thinking stream buffer (coalesced like the text buffer above).
        // CC's `thinking_delta`s arrive per-token, so we flush on a paragraph
        // boundary OR once `THOUGHT_FLUSH_THRESHOLD` chars accumulate — the latter
        // bounds latency so a long unbroken reasoning paragraph still streams live
        // (the frozen-"Working" bug) instead of landing only at turn end.
        let mut claude_thought_buf = String::new();
        let mut last_thought_persisted_len: usize = 0;
        const THOUGHT_FLUSH_THRESHOLD: usize = 240;
        let mut is_waiting = false;
        let mut proposed_change = false;
        let mut emitted_terminal_event = false; // Track whether ResponseGenerated/ResponseCanceled was emitted
                                                // user_hit_stop: when true, the next Result emits ResponseCanceled (exchange:
                                                // "Canceled") instead of ResponseGenerated. Reset on next user follow-up.
        let mut user_hit_stop = false;
        // interrupt_is_redirect: set alongside user_hit_stop when the interrupt
        // came from a Codex mid-turn follow-up redirect (drained from the session
        // by `arm_codex_redirect`), so the Result classifies as
        // CancelCause::SupersededByFollowup (neutral render) instead of UserStop.
        // Cleared at the turn boundary in lockstep with user_hit_stop.
        let mut interrupt_is_redirect = false;
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
        // Set from `AgentEvent::Exited { killed_by_signal }` — true when the CC
        // child died from a signal the engine did NOT initiate (the exit=143
        // stray-SIGTERM bug). Consumed by the safety net to auto-resume.
        let mut killed_by_signal = false;

        // Bounded Esc fallback. A real Cancel (Stop) forwards CC's native
        // interrupt (Esc) and waits for CC to wind down and emit a `Result`. If
        // CC doesn't honor it within this window — a hung socket, or a control
        // request ignored while a long tool runs (the watchdog skips while a
        // tool is in flight, so it won't catch this) — escalate to the hard stop
        // so Cancel is always responsive. Armed when the interrupt is forwarded;
        // cleared once CC's terminal `Result` lands.
        let mut interrupt_escalate_at: Option<tokio::time::Instant> = None;
        const INTERRUPT_ESCALATE_AFTER: std::time::Duration = std::time::Duration::from_secs(8);

        'event_loop: loop {
            // Snapshot the (Copy) deadline so the escalation arm below polls a
            // value, not a borrow of `interrupt_escalate_at` that other arms mutate.
            let escalate_deadline = interrupt_escalate_at;
            tokio::select! {
                event_opt = events_rx.recv() => {
                    let Some(ev) = event_opt else {
                        // Driver task exited without sending Exited (defensive — should not happen).
                        log!(
                            "[AgentSession] events_rx closed without AgentEvent::Exited for thread {}",
                            thread_id
                        );
                        break;
                    };
                    if let AgentEvent::Exited { killed_by_signal: ev_killed_by_signal } = ev {
                        killed_by_signal = ev_killed_by_signal;
                        // Final flush of any pending reasoning (process exited
                        // mid-think — surface what it reasoned before dying).
                        self.flush_coding_agent_thought(
                            thread_id,
                            &claude_thought_buf,
                            &mut last_thought_persisted_len,
                            coding_agent,
                            &meta,
                        )
                        .await;
                        // Final flush of any pending text
                        if !claude_text_buf.is_empty() {
                            let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                            if !delta.is_empty() {
                                self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), coding_agent },
                                    meta: meta.clone(),
                                }, "[AgentSession] CodingAgentTextStreamed (final flush on exit)").await;
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
                            log!("[AgentSession] CC process exited while idle — releasing thread {}", thread_id);

                            // Half-assed work (Failed / Canceled / Aborted / safety-net)
                            // skips the auto-commit so the post-commit hook doesn't fire
                            // a spurious ChangeProposed for partial work. should_discard
                            // is always false here (no user Discard click without breaking
                            // out via the stop arm); pass false to keep the gate purely
                            // about terminal kind.
                            if let Some(ref wt) = worktree_path {
                                if should_auto_commit_on_cleanup(false, &last_terminal_kind) {
                                    auto_commit_preserving_marker(&self.pool, wt, &repo_root, &branch_name, "Coding agent changes (auto-committed on idle exit)").await;
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

                            // A turn the backend ended on a transient upstream
                            // `API Error` resumes itself instead of leaving a red dot
                            // nobody is watching. This arm is the site that matters:
                            // a reported drop always arrives as a real Result, so the
                            // turn idles and returns HERE, never reaching the post-loop
                            // `finalize_direct_agent` where the recovery originally
                            // (and only) lived. See the helper's doc comment.
                            //
                            // Position is load-bearing: after the Result arm emitted
                            // this turn's `CodingAgentIdled`, after the subprocess is
                            // gone and the session dropped from `agent_sessions` just
                            // above, and after the follow-up drain, whose result
                            // decides whether anything else is already coming.
                            self.maybe_auto_resume_after_api_error(
                                thread_id,
                                &last_terminal_kind,
                                &meta,
                                conflict_change.is_some(),
                                !orphans.is_empty(),
                            )
                            .await;

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
                            "[AgentSession] CC exited without Result event for thread {} (buffered_text_len={})",
                            thread_id,
                            claude_text_buf.len()
                        );
                        break;
                    }
                    // Stamp liveness — used by apply_now's timeout. Also drain the
                    // question-answer resume signal in the SAME lock: a live
                    // subprocess woken by an answered AskUserQuestion continues its
                    // turn via the PreToolUse hook, never touching `msg_tx`, so the
                    // run loop never hit `reset_per_turn_flags` (the only place
                    // `emitted_terminal_event` clears). If the turn was
                    // terminal-armed, re-arm emission below so CC's first
                    // post-answer event is processed instead of dropped as a
                    // "post-terminal straggler" (see `question_resume_pending`).
                    let resume_after_answer = {
                        let mut guard = self.agent_sessions.lock().await;
                        if let Some(s) = guard.get_mut(&thread_id) {
                            s.last_event_at.store(now_epoch_millis(), std::sync::atomic::Ordering::Relaxed);
                            let armed = std::mem::take(&mut s.question_resume_pending);
                            let reset = armed && emitted_terminal_event;
                            if reset {
                                // Mirror the msg_rx arm's session-side clear.
                                s.is_waiting = false;
                            }
                            reset
                        } else {
                            false
                        }
                    };
                    if resume_after_answer {
                        log!(
                            "[AgentSession] Question answered on a terminal-armed turn for thread {} — re-arming emission for the resumed turn",
                            thread_id
                        );
                        reset_per_turn_flags(
                            &mut is_waiting,
                            &mut last_emitted_idle,
                            &mut emitted_terminal_event,
                            &mut user_hit_stop,
                            &mut interrupt_is_redirect,
                            &mut last_terminal_kind,
                            &mut meta.actor,
                        );
                    }
                    match ev {
                        AgentEvent::Init { session_id: cc_sid, model: init_model, slash_commands: cmds, skills } => {
                            log!("[AgentSession] [TIMING] Init event received: {:?}", cc_start.elapsed());
                            // Record what the backend actually attached to before
                            // anything else — the stale-resume veto reads it.
                            init_session_id = Some(cc_sid.clone());
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
                                    coding_agent,
                                    cc_session_id: Some(cc_sid.clone()),
                                    // Pin the session↔config-dir pairing at Init:
                                    // the dir this session was created under is what
                                    // a later resume must re-inject to find its
                                    // transcript. See `effective_config_dir` above
                                    // and `lookup_pinned_cc_config_dir`.
                                    claude_config_dir: effective_config_dir.clone(),
                                },
                                meta: meta.clone(),
                            }).await {
                                log!("[AgentSession] Failed to persist Init CodingAgentSettingsChanged for {}: {}", thread_id, e);
                            }
                            // Release startup semaphore — CC process is initialized and mostly idle now.
                            if let Some(permit) = startup_permit.take() {
                                drop(permit);
                                log!("[AgentSession] [TIMING] Startup semaphore released: {:?}", cc_start.elapsed());
                            }
                            if let Some(ref effort) = cc_reasoning_effort {
                                log!("[AgentSession] Setting initial reasoning effort: {}", effort);
                                if agent_control_tx
                                    .send(crate::runtime::ControlRequest::SetReasoningEffort {
                                        effort: effort.clone(),
                                    })
                                    .is_err()
                                {
                                    log!("[AgentSession] Failed to forward reasoning effort: agent control channel closed");
                                }
                            }
                        }
                        // Straggler guard: the agent runtime sometimes delivers a
                        // trailing Message AFTER this turn's terminal event (e.g. a
                        // final "Understood." arriving on `events_rx` once the
                        // `Result` arm already emitted ResponseGenerated +
                        // CodingAgentIdled). Emitting it as CodingAgentTextStreamed
                        // would re-flip the projection to status='running' (the
                        // activity-event arm in `event_bus_projection_thread.rs`
                        // bumps running), stranding an already-idled thread as
                        // "running" forever — the "thread reports running when it's
                        // not" bug. Drop it; a real follow-up turn re-arms emission
                        // via `reset_per_turn_flags` when its prompt lands on msg_rx.
                        AgentEvent::Message { text, .. } if emitted_terminal_event => {
                            log!(
                                "[AgentSession] Dropping post-terminal straggler text ({} chars) for thread {} — would resurrect 'running' on an idled thread",
                                text.len(),
                                thread_id
                            );
                        }
                        AgentEvent::Message { text, .. } => {
                            // CC resumed after waiting — clear waiting state
                            if is_waiting {
                                is_waiting = false;
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                            }
                            // Reasoning precedes the answer — flush any pending
                            // thought so the "Thinking" step resolves before the
                            // text it was reasoning toward.
                            self.flush_coding_agent_thought(
                                thread_id,
                                &claude_thought_buf,
                                &mut last_thought_persisted_len,
                                coding_agent,
                                &meta,
                            )
                            .await;
                            claude_text_buf.push_str(&text);
                            // Persist + broadcast at natural boundaries
                            if should_flush(&claude_text_buf) {
                                let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                                if !delta.is_empty() {
                                    self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                        thread_id,
                                        event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), coding_agent },
                                        meta: meta.clone(),
                                    }, "[AgentSession] CodingAgentTextStreamed (Message flush)").await;
                                    last_text_persisted_len = claude_text_buf.len();
                                }
                            }
                        }
                        // Reasoning/thinking stream. Same post-terminal straggler
                        // guard as Message/ToolUse: CodingAgentThoughtStreamed is
                        // per-token-streaming, and its projection arm bumps
                        // status='running', so a thought arriving after the turn's
                        // terminal event would resurrect an idled thread. Drop it.
                        AgentEvent::Thought { .. } if emitted_terminal_event => {}
                        AgentEvent::Thought { text } => {
                            // A thought is the first sign of life on a resumed turn —
                            // clear waiting state like Message/ToolUse do.
                            if is_waiting {
                                is_waiting = false;
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                            }
                            claude_thought_buf.push_str(&text);
                            // Flush on a paragraph boundary OR once enough has piled
                            // up — the threshold bounds latency so a long unbroken
                            // reasoning paragraph still streams live instead of
                            // landing only at turn end (the frozen-"Working" bug).
                            if should_flush(&claude_thought_buf)
                                || claude_thought_buf.len() - last_thought_persisted_len
                                    >= THOUGHT_FLUSH_THRESHOLD
                            {
                                self.flush_coding_agent_thought(
                                    thread_id,
                                    &claude_thought_buf,
                                    &mut last_thought_persisted_len,
                                    coding_agent,
                                    &meta,
                                )
                                .await;
                            }
                        }
                        // Straggler guard (see the Message arm above): a tool call
                        // arriving after the turn's terminal event would resurrect
                        // status='running' on an idled thread. Drop it without
                        // touching the in-flight counter — a post-terminal turn has
                        // no live watchdog to disarm.
                        AgentEvent::ToolUse { .. } if emitted_terminal_event => {
                            log!(
                                "[AgentSession] Dropping post-terminal straggler tool call for thread {} — would resurrect 'running' on an idled thread",
                                thread_id
                            );
                        }
                        AgentEvent::ToolUse { name, input, id } => {
                            // CC resumed after waiting — clear waiting state
                            if is_waiting {
                                is_waiting = false;
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                            }
                            // Interleaved-thinking turns reason right up to a tool
                            // call — flush any pending thought before the tool step.
                            self.flush_coding_agent_thought(
                                thread_id,
                                &claude_thought_buf,
                                &mut last_thought_persisted_len,
                                coding_agent,
                                &meta,
                            )
                            .await;
                            {
                                let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                                if !delta.is_empty() {
                                    self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                        thread_id,
                                        event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), coding_agent },
                                        meta: meta.clone(),
                                    }, "[AgentSession] CodingAgentTextStreamed (pre-ToolUse flush)").await;
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
                            // A tool call this turn proves the session is alive —
                            // even a terse model that emits no text. Gates the
                            // stale-resume heuristic below so a working Fable
                            // resume is never misread as a dead session.
                            tool_calls_seen = tool_calls_seen.saturating_add(1);
                            if crate::engine::agent_session::lifecycle::is_user_question_tool(&name) {
                                // Question flow — the agent subprocess blocks until the user
                                // answers and the engine renders the card from the
                                // `UserQuestionAsked` the internal endpoint emits. CC routes
                                // here via its PreToolUse hook (`crate::engine::cc_settings` +
                                // `api/internal.rs::ask_user_question`); Codex via the
                                // `mcp__lucidos__ask_user_question` MCP tool hitting the same
                                // endpoint. run_session has nothing to do with this `tool_use`
                                // event — no emit (a tool-call step would double-surface the
                                // question), no kill (the subprocess keeps running), no
                                // session removal.
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
                                        coding_agent,
                                        tool_use_id: id,
                                    },
                                    meta: meta.clone(),
                                }, "[AgentSession] CodingAgentToolCalled").await;
                            }
                        }
                        // Straggler guard (see the Message arm above): a tool result
                        // arriving after the turn's terminal event would resurrect
                        // status='running' on an idled thread. Still release the
                        // in-flight slot in case a pre-terminal ToolUse incremented
                        // it (release_tool_slot floors at 0, so an unpaired result is
                        // a no-op), then drop the event without emitting.
                        AgentEvent::ToolResult { .. } if emitted_terminal_event => {
                            release_tool_slot(&tools_in_flight);
                            log!(
                                "[AgentSession] Dropping post-terminal straggler tool result for thread {} — would resurrect 'running' on an idled thread",
                                thread_id
                            );
                        }
                        AgentEvent::ToolResult { output, status: _, id } => {
                            let summary: String = output.chars().take(200).collect();
                            // Re-arm the watchdog if this was the last in-flight
                            // tool. Floored at 0 so an unpaired ToolResult (CC
                            // oddity / replay) can't underflow — see
                            // `release_tool_slot`.
                            release_tool_slot(&tools_in_flight);
                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentToolResult {
                                    name: String::new(),
                                    result: summary,
                                    coding_agent,
                                    tool_use_id: id,
                                },
                                meta: meta.clone(),
                            }, "[AgentSession] CodingAgentToolResult").await;
                        }
                        AgentEvent::Usage {
                            model: cc_msg_model,
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_creation_tokens,
                        } => {
                            // A usage frame reaches us only for a real API call
                            // (the parser drops all-zero ones), so this is the
                            // proof-of-model-call the resume-settle skip reads.
                            api_calls_seen = api_calls_seen.saturating_add(1);
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
                            let context_window = self.context_window_for(&snapshot_model);
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
                                            producer: crate::engine::ContextProducer::from_coding_agent(coding_agent),
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
                                    "[AgentSession] ContextCaptured",
                                )
                                .await;
                        }
                        // Liveness-only ping from a streaming delta. The
                        // top-of-loop heartbeat bump already recorded that the
                        // subprocess is alive — which is precisely what stops the
                        // watchdog from euthanizing a long single step (extended
                        // thinking, a large generation) that streams for longer
                        // than WATCHDOG_INACTIVITY_LIMIT_MS without finishing a
                        // step. Nothing to persist: the complete text / tool call
                        // arrives separately as Message / ToolUse.
                        AgentEvent::StreamActivity => {}
                        AgentEvent::Exited { .. } => unreachable!("Exited handled above"),
                        AgentEvent::Result { text, error: cc_error, .. } => {
                                        let err_suffix = cc_error.as_deref().map(|e| format!(" (error: {})", e)).unwrap_or_default();
                                        log!("[AgentSession] Result event received — entering waiting state{}", err_suffix);
                                        // Final flush of any pending reasoning (a turn
                                        // that reasoned then ended without text/tools,
                                        // or the tail below the coalescing threshold).
                                        self.flush_coding_agent_thought(
                                            thread_id,
                                            &claude_thought_buf,
                                            &mut last_thought_persisted_len,
                                            coding_agent,
                                            &meta,
                                        )
                                        .await;
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
                                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), coding_agent },
                                                    meta: meta.clone(),
                                                }, "[AgentSession] CodingAgentTextStreamed (Result flush)").await;
                                            }
                                        } else if !text.trim().is_empty() {
                                            // Slash commands (e.g. /model) produce a Result
                                            // without any preceding Message events. Emit the
                                            // result text as CodingAgentTextStreamed so the
                                            // frontend displays it.
                                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: text.trim().to_string(), coding_agent },
                                                meta: meta.clone(),
                                            }, "[AgentSession] CodingAgentTextStreamed (slash command result)").await;
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
                                        //
                                        // Two independent signals, both meaning "the id we --resume'd is
                                        // gone": the empty-echo heuristic (`is_stale_resume_signal`,
                                        // gated on !cc_error so a transient 5xx never deletes user work),
                                        // AND CC's EXPLICIT "No conversation found with session ID" error.
                                        // The explicit error arrives AS a cc_error, so it bypasses the
                                        // heuristic's gate — but it's deterministic (re-resuming can never
                                        // succeed), so it's the one whitelisted error string that still
                                        // recovers. Both take the same path: shadow the dead sid via
                                        // SessionEnded(StaleResume), remove the worktree, keep the branch,
                                        // return STALE_RESUME_ERROR so the caller retries fresh (chat →
                                        // process_cc.rs; merge/apply → the Tier-2 Err arm falls through to
                                        // a fresh Tier-3 merge session). Root cause: dev/bf997e21, a
                                        // mid-flight CLAUDE_CONFIG_DIR switch relocated CC's transcript
                                        // store.
                                        let explicit_session_not_found = resume_session_id.is_some()
                                            && is_definitive_session_not_found(cc_error.as_deref());
                                        // Did the backend attach to the conversation we
                                        // asked for? Its Init echoes the id it actually
                                        // opened, and a FAILED resume yields a different
                                        // one — so a match is structural proof of life
                                        // and vetoes the output-shape heuristic. Compared
                                        // only when we requested a resume; a fresh spawn
                                        // has nothing to match against.
                                        let stale_inputs = StaleResumeInputs {
                                            has_resume_session: resume_session_id.is_some(),
                                            resume_attach_confirmed: resume_session_id.is_some()
                                                && init_session_id.as_deref()
                                                    == resume_session_id.as_deref(),
                                            result_text_empty,
                                            buffered_text_empty,
                                            no_prior_results_this_turn: result_texts.is_empty(),
                                            no_tool_calls_this_turn: tool_calls_seen == 0,
                                            user_message_present: !user_message.is_empty(),
                                            cc_error: cc_error.is_some(),
                                        };
                                        // The turn's SHAPE said "stale", the confirmed
                                        // attach overruled it, and no API call was made:
                                        // this Result closes the backend's own
                                        // resume-settle turn (an orphaned tool_use being
                                        // closed out, a queued task-notification being
                                        // drained), not ours. Our prompt may not even be
                                        // dequeued yet, so ending the turn here reports a
                                        // failure that did not happen AND kills the
                                        // subprocess mid-answer (the 2026-08-05 swallowed
                                        // follow-up). Skip it and keep reading; the real
                                        // Result is still coming.
                                        if is_resume_settle_result(stale_inputs, api_calls_seen == 0) {
                                            log!(
                                                "[AgentSession] thread {} resumed sid={} and the backend confirmed the attach, but this Result carries no text, no tool call and no error: it closes the resume-settle turn, not ours. Skipping it and waiting for the real Result.",
                                                thread_id,
                                                resume_session_id.as_deref().unwrap_or("")
                                            );
                                            // Record it so the skip cannot repeat:
                                            // `no_prior_results_this_turn` goes false, so
                                            // any further Result classifies normally.
                                            result_texts.push(text.clone());
                                            continue 'event_loop;
                                        }
                                        // Same empty shape, same confirmed attach, but the
                                        // backend DID call the model, so whatever came back
                                        // is an answer to our prompt however empty. It
                                        // classifies below (an empty one lands
                                        // `EMPTY_RESPONSE_ERROR`). Logged because it is the
                                        // one shape a wrong skip here would strand at
                                        // `running` until the watchdog: if a spurious
                                        // empty-response failure is ever reported, this
                                        // line is what says the settle skip considered the
                                        // turn and the API call is what ruled it out.
                                        if api_calls_seen > 0
                                            && is_resume_settle_result(stale_inputs, true)
                                        {
                                            log!(
                                                "[AgentSession] thread {} resumed sid={} and produced an empty Result with no tool calls, but {} API call(s) were made: it answers our prompt, not a resume-settle turn. Classifying it.",
                                                thread_id,
                                                resume_session_id.as_deref().unwrap_or(""),
                                                api_calls_seen
                                            );
                                        }
                                        if is_stale_resume_signal(stale_inputs)
                                            || explicit_session_not_found
                                        {
                                            log!("[AgentSession] Stale resume detected (empty-echo heuristic or explicit session-not-found) — aborting session to retry with a fresh spawn.");
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
                                            }, "[AgentSession] SessionEnded (stale resume)").await;
                                            // NEVER CREATE A DUPLICATE: await the old process
                                            // group's full teardown before signalling the retry.
                                            // `agent_cancel.cancel()` drives the driver_task's
                                            // `graceful_kill_child_process_group` (SIGTERM →
                                            // GROUP_TEARDOWN_GRACE → SIGKILL → reap); the driver
                                            // drops `events_tx` only AFTER that completes, so
                                            // draining `events_rx` to a closed channel is a precise
                                            // await of "the old CC process group is dead." Without
                                            // it the fresh retry (`run_direct_agent`) could spawn
                                            // while the wedged old process is still alive on the
                                            // SHARED deterministic worktree — the 2026-07-02
                                            // double-process (2x quota) bug. Bounded so a stuck
                                            // teardown can't wedge the loop (grace is 3s; 15s is
                                            // generous headroom).
                                            let teardown_deadline = tokio::time::Instant::now()
                                                + std::time::Duration::from_secs(15);
                                            loop {
                                                match tokio::time::timeout_at(
                                                    teardown_deadline,
                                                    events_rx.recv(),
                                                )
                                                .await
                                                {
                                                    // Trailing events from the dying process — drop them.
                                                    Ok(Some(_)) => continue,
                                                    // Channel closed → the old process group is dead.
                                                    Ok(None) => break,
                                                    Err(_) => {
                                                        log!("[AgentSession] stale-resume teardown wait timed out for thread {} — proceeding with retry (old process may briefly linger)", thread_id);
                                                        break;
                                                    }
                                                }
                                            }
                                            // NEVER DELETE A POSSIBLY-USEFUL WORKTREE. The
                                            // deterministic `thread-<id>` worktree is SHARED per
                                            // thread and holds the user's warm working copy; we do
                                            // NOT remove it here. The retry (`run_direct_agent` with
                                            // `resume_session_id: None`) REUSES the existing worktree
                                            // — `spawn_context` runs `worktree_add` only when the dir
                                            // is absent, and `clear_stranded_worktree_dir` handles a
                                            // genuinely-broken one. The branch is the thread's
                                            // durable anchor and is kept regardless. The background
                                            // WorktreeCleanup worker is the SOLE deleter of a valid
                                            // worktree (gated on is_active, dirty, unmerged commits,
                                            // retention). The old inline `git worktree remove
                                            // --force` here deleted a live worktree out from under a
                                            // concurrent process and needlessly wiped node_modules —
                                            // removed 2026-07-02.
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
                                            interrupt_is_redirect,
                                            is_shutdown,
                                            cc_error,
                                            buffered_text_empty && result_text_empty,
                                        );
                                        // Capture before the `if let Some(kind)` below moves out —
                                        // `may_touch_change_state_at_idle` and the post-loop cleanup
                                        // both read this to refuse half-assed work.
                                        last_terminal_kind = terminal_kind.clone();
                                        if let Some(kind) = terminal_kind {
                                            // A Result is a turn boundary — both
                                            // the user-stop latch and the cancel
                                            // actor on `meta` must clear here so an
                                            // interrupt kept alive by inflight
                                            // follow-ups can't relabel the
                                            // follow-ups' successful completion as a
                                            // second Canceled, and a resumed turn
                                            // doesn't inherit the cancelling device.
                                            let clears = terminal_clears_user_hit_stop(&kind);
                                            if clears {
                                                user_hit_stop = false;
                                                interrupt_is_redirect = false;
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
                                                }, "[AgentSession] terminal event (Result classify)").await;
                                            }
                                            // Clear AFTER the emit so the cancel
                                            // terminal still carries the device.
                                            if clears {
                                                meta.actor = None;
                                            }
                                        }
                                        emitted_terminal_event = true;
                                        // CC honored the interrupt (a Result landed) — disarm the
                                        // bounded Esc fallback so it can't escalate to a hard stop.
                                        interrupt_escalate_at = None;
                                        claude_text_buf.clear();
                                        last_text_persisted_len = 0;
                                        claude_thought_buf.clear();
                                        last_thought_persisted_len = 0;
                                        // Auto-commit any dirty files before checking for changes.
                                        // CC may create/edit files via Bash without committing. Without
                                        // this, the three-dot diff below sees no committed changes and
                                        // wt_has_changes is false, preventing ChangeProposed from firing.
                                        if let Some(ref wt) = worktree_path {
                                            auto_commit_preserving_marker(&self.pool, wt, &repo_root, &branch_name, "Coding agent changes (auto-committed)").await;
                                        }
                                        // Check for worktree changes before entering waiting.
                                        // `branch_changed_files` is a three-dot merge-base diff against
                                        // `default_diff_base` (the SAME base the Diff button uses) so we
                                        // only see changes introduced ON this branch, not changes the
                                        // base received after the branch was created. Without this, a
                                        // branch whose changes were already merged — or whose local
                                        // default diverged — appears to have changes because the base
                                        // moved ahead, lighting the Diff button on an empty diff.
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
                                                            log!("[AgentSession] Carry-forward skipped — branch has no actual diff (likely commit+revert)");
                                                            (false, false)
                                                        } else {
                                                            log!("[AgentSession] Carrying forward has_changes=true from previous idle (worktree diff was empty)");
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
                                                coding_agent,
                                            ).await;
                                            last_emitted_idle = true;
                                        }

                                        // Propose the change at idle time so the Apply button
                                        // shows immediately (propose_change deduplicates). When
                                        // CC skipped /harden, hardened=false propagates to the
                                        // change record and Apply runs hardening at click time.
                                        // Background bash deliberately does NOT gate this — see
                                        // `may_touch_change_state_at_idle` for the rationale and
                                        // the shutdown / external-repo / conflict-session guards.
                                        //
                                        // Gated on `may_touch_change_state_at_idle` (the propose
                                        // gate WITHOUT its `wt_has_changes` term) so the
                                        // empty-diff arm below is reachable: a branch whose diff
                                        // cancelled out still has a pending row to reconcile, and
                                        // gating the whole block on `wt_has_changes` left that row
                                        // claiming files the branch no longer had.
                                        if may_touch_change_state_at_idle(
                                            is_external_repo,
                                            is_shutdown,
                                            conflict_change.is_some(),
                                            &last_terminal_kind,
                                        ) {
                                            let changed_files = branch_changed_files(&repo_root, &branch_name).await;
                                            if changed_files.is_empty() {
                                                // No committed diff against the base (nothing was
                                                // done, or a commit + revert cancelled out). Never
                                                // auto-discard an existing pending row — the user
                                                // resolves it from Review — but DO re-sync it to
                                                // zero files so the card stops advertising work the
                                                // branch no longer carries. No pending row → nothing
                                                // to reconcile and nothing to propose.
                                                self.reconcile_emptied_pending_change(thread_id, &repo_root, &branch_name).await;
                                            } else {
                                                // Only the propose path needs the marker — the
                                                // reconcile above re-reads it itself, and only if
                                                // it actually has a row to correct.
                                                let hardened = is_harden_marker_present(&self.pool, &repo_root, &branch_name).await;
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
                                                    // Always false now: `may_touch_change_state_at_idle`
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
                                                        log!("[AgentSession] Failed to propose change at idle: {}", e);
                                                    }
                                                }
                                                self.broadcast_changes_updated().await;
                                            }
                                        }

                                        match idle_action(conflict_change.is_some(), is_shutdown) {
                                            IdleAction::EndSession => {
                                                log!("[AgentSession] Conflict-resolution session idle for thread {} — ending loop", thread_id);
                                                break 'event_loop;
                                            }
                                            IdleAction::ExitSubprocess => {
                                                // Hold the `agent_sessions` lock across the whole
                                                // read-decide-act so the chat fast-path's
                                                // check-increment-send (`chat::process`, which
                                                // takes the SAME lock) is mutually exclusive with
                                                // this terminate decision. Without that serialization
                                                // a follow-up could `fetch_add` + `msg_tx.send` into
                                                // a subprocess this arm is about to cancel — the
                                                // idle-termination race that silently drops the
                                                // follow-up (the subprocess dies before producing a
                                                // Result; see docs/plans/2026-06-27-cc-idle-
                                                // termination-followup-race.md). No `.await` runs
                                                // inside this section other than the lock acquire,
                                                // and every operation (swap / decide / field set /
                                                // notify / cancel / log) is sync, so holding the
                                                // lock cannot deadlock.
                                                let mut sessions = self.agent_sessions.lock().await;
                                                // `swap(0)` resets per-Result (turn boundary).
                                                // AcqRel pairs with the fast-path `fetch_add` in
                                                // `chat::process` so a racing increment is
                                                // observed; now serialized by the lock above. The
                                                // pure decision lives in `terminate_decision` so the
                                                // precedence rules and the per-reason log line both
                                                // read from one place (see `TerminateDecision`).
                                                let prev = pending_followups
                                                    .swap(0, std::sync::atomic::Ordering::AcqRel);
                                                match terminate_decision(
                                                    prev,
                                                    bg_bash_running,
                                                ) {
                                                    TerminateDecision::KeepAliveForFollowup { inflight } => {
                                                        log!("[AgentSession] Skipping subprocess termination for thread {} — {} follow-up(s) inflight (queued or merged)", thread_id, inflight);
                                                    }
                                                    TerminateDecision::KeepAliveForBgBash => {
                                                        log!("[AgentSession] Skipping subprocess termination for thread {} — background bash still running (auto-wake will resume CC on completion)", thread_id);
                                                    }
                                                    TerminateDecision::Terminate => {
                                                        // Mark the session exited BEFORE cancelling
                                                        // so a follow-up landing during the graceful-
                                                        // shutdown window (process is alive until
                                                        // `AgentEvent::Exited`, up to ~3s later) sees
                                                        // `process_exited == true` and routes via the
                                                        // slow `--resume` path instead of `msg_tx`
                                                        // into the dying subprocess. Mirrors the
                                                        // pre-exit mark in `completion.rs`.
                                                        if let Some(s) = sessions.get_mut(&thread_id) {
                                                            s.process_exited = true;
                                                            s.idle_notify.notify_waiters();
                                                        }
                                                        log!("[AgentSession] Idle reached — terminating the coding-agent subprocess for thread {} so next turn resumes via --resume", thread_id);
                                                        agent_cancel.cancel();
                                                    }
                                                }
                                            }
                                            IdleAction::Nothing => {}
                                        }
                                    }
                                }
                }

                // Codex app-server approval bridge. Each request spawns its
                // own waiter task so this loop NEVER blocks on the user — a
                // pending card must not stall event processing, interrupts,
                // or the watchdog (run.rs is the engine's highest-traffic
                // loop; keep this arm minimal).
                perm_req = async {
                    match agent_permission_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        // No in-band permission channel (CC / codex exec) —
                        // pend forever so the arm never fires.
                        None => std::future::pending().await,
                    }
                } => {
                    match perm_req {
                        Some(req) => {
                            let pool = self.pool.clone();
                            let bus = self.event_bus.clone();
                            let pending = self.pending_cc_permission.clone();
                            // Unattended (trigger-rooted) sessions auto-resolve
                            // the card from the inherited side-effect grant
                            // instead of hanging — `prompt_coding_agent_permission`
                            // needs the trigger registry + workspace root to
                            // decide (see `cc_permission::resolve_attend_mode`).
                            let trigger_configs = self.trigger_configs.clone();
                            let workspace_path = self.workspace_path.clone();
                            // A file write landing inside this session's own
                            // worktree skips the card entirely. This local IS
                            // what seeded `AgentSession.worktree_path`, so it
                            // matches what the CC MCP path resolves via
                            // `cc_permission::lookup_session_worktree` — and
                            // reading it directly keeps this arm lock-free.
                            let session_worktree = worktree_path.clone();
                            // Disarm the watchdog while the card waits — the
                            // approval may arrive BEFORE the item's ToolUse
                            // (codex raises it pre-execution), so the paired
                            // tool counter alone can't be relied on to cover
                            // the wait.
                            let tools = tools_in_flight.clone();
                            tokio::spawn(async move {
                                tools.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                let mut respond = req.respond;
                                tokio::select! {
                                    outcome = crate::engine::cc_permission::prompt_coding_agent_permission(
                                        &pool,
                                        &bus,
                                        &pending,
                                        &trigger_configs,
                                        &workspace_path,
                                        session_worktree.as_deref(),
                                        crate::engine::cc_permission::CodingAgentPermissionInput {
                                            thread_id,
                                            tool_use_id: req.id,
                                            tool_name: req.tool_name,
                                            input: req.input,
                                        },
                                    ) => {
                                        // Send failure = driver died between answer
                                        // and delivery; nothing to deliver to.
                                        let _ = respond.send(outcome.allowed);
                                    }
                                    // Driver died (interrupt escalation, child
                                    // crash) while the card was pending — the
                                    // driver's JoinSet aborts its waiter task,
                                    // dropping the oneshot receiver, which fires
                                    // this arm. Abandoning the wait drops our
                                    // broadcast receiver so the next prompt's
                                    // gc_dead_entries sweep evicts the entry. The
                                    // persisted card resolves via the same recovery
                                    // paths CC uses (supersede-on-new-message,
                                    // orphan sweep).
                                    _ = respond.closed() => {
                                        log!(
                                            "[AgentSession] agent died with permission card pending for thread {} — abandoning waiter",
                                            thread_id
                                        );
                                    }
                                }
                                release_tool_slot(&tools);
                            });
                        }
                        None => {
                            // Driver dropped its sender — disable the arm so a
                            // closed channel doesn't re-resolve every loop tick.
                            agent_permission_rx = None;
                        }
                    }
                }

                Some(user_input) = msg_rx.recv() => {
                    reset_per_turn_flags(
                        &mut is_waiting,
                        &mut last_emitted_idle,
                        &mut emitted_terminal_event,
                        &mut user_hit_stop,
                        &mut interrupt_is_redirect,
                        &mut last_terminal_kind,
                        &mut meta.actor,
                    );
                    {
                        let mut sessions = self.agent_sessions.lock().await;
                        if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                    }
                    // Flush any pending reasoning before the new user input opens a
                    // fresh turn, then reset so the next turn's thinking starts clean.
                    self.flush_coding_agent_thought(
                        thread_id,
                        &claude_thought_buf,
                        &mut last_thought_persisted_len,
                        coding_agent,
                        &meta,
                    )
                    .await;
                    claude_thought_buf.clear();
                    last_thought_persisted_len = 0;
                    if !claude_text_buf.is_empty() {
                        let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                        if !delta.is_empty() {
                            self.event_bus.emit_or_log(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), coding_agent },
                                meta: meta.clone(),
                            }, "[AgentSession] CodingAgentTextStreamed (flush before user_input)").await;
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
                                coding_agent,
                                // Audit trail for a user-driven prompt — origin is
                                // carried by the MessageReceived emitted at the API
                                // boundary.
                                origin: None,
                            },
                            meta: meta.clone(),
                        }, "[AgentSession] CodingAgentPromptSent").await;
                    }
                }

                _ = interrupt.notified() => {
                    // Cancel button = Esc in Claude Code CLI.
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
                        // Distinguish a follow-up redirect (set by
                        // `arm_codex_redirect`) from a real Stop click: the
                        // former classifies as SupersededByFollowup (neutral),
                        // the latter as UserStop ("Canceled ✕"). Drained on read;
                        // cleared at the Result turn boundary below alongside
                        // user_hit_stop.
                        interrupt_is_redirect =
                            self.take_session_redirect_followup(thread_id).await;
                        // Drain the device that clicked Cancel (stamped by
                        // `interrupt_agent`) onto `meta` so the terminal
                        // `ResponseCanceled` records it — the popover's Device row.
                        // Cleared again at the Result turn boundary below.
                        if let Some(cancel_actor) = self.take_session_cancel_actor(thread_id).await {
                            meta.actor = Some(cancel_actor);
                        }
                        log!("[AgentSession] Sending control_request interrupt to CC process");
                        if agent_control_tx
                            .send(crate::runtime::ControlRequest::Interrupt)
                            .is_err()
                        {
                            log!("[AgentSession] Failed to forward interrupt: agent control channel closed");
                        }
                        // Arm the bounded fallback: if CC doesn't emit a Result
                        // within the window, the escalation arm hard-stops below.
                        interrupt_escalate_at =
                            Some(tokio::time::Instant::now() + INTERRUPT_ESCALATE_AFTER);
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
                        false, // the stop channel is never a redirect (redirects use `interrupt`)
                        &agent_cancel,
                        &claude_text_buf,
                        last_text_persisted_len,
                        &meta,
                        &external_terminal_emitted,
                        &normalized_model,
                        &cc_reasoning_effort,
                        coding_agent,
                    ).await;
                    emitted_terminal_event = true;
                    break;
                }

                // Bounded Esc fallback: CC did not emit a Result within the
                // window after we forwarded the interrupt. Escalate to the hard
                // stop so Cancel is always responsive. Stamp `Canceled(UserStop)`
                // so `finalize` keeps the branch (KeepCanceledBranch) — the
                // session is still best-effort resumable via the `cc_session_id`
                // recorded on `CodingAgentSettingsChanged`. No-worse than the old
                // immediate hard-kill, just delayed for the (rare) ignored-Esc case.
                _ = async move {
                    match escalate_deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    log!(
                        "[AgentSession] CC did not honor interrupt within {}s — escalating to hard stop for thread {}",
                        INTERRUPT_ESCALATE_AFTER.as_secs(),
                        thread_id
                    );
                    last_terminal_kind = Some(TerminalKind::Canceled(
                        if interrupt_is_redirect {
                            crate::engine::thread_events::CancelCause::SupersededByFollowup
                        } else {
                            crate::engine::thread_events::CancelCause::UserStop
                        },
                    ));
                    let is_shutdown = shutting_down.load(std::sync::atomic::Ordering::Relaxed);
                    self.emit_stop_terminal(
                        "interrupt_escalate",
                        thread_id,
                        is_waiting,
                        is_shutdown,
                        false, // suppress: a real Cancel — not Apply/Discard/Archive
                        interrupt_is_redirect, // redirect → SupersededByFollowup cause
                        &agent_cancel,
                        &claude_text_buf,
                        last_text_persisted_len,
                        &meta,
                        &external_terminal_emitted,
                        &normalized_model,
                        &cc_reasoning_effort,
                        coding_agent,
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
                        false, // chat_cancel is engine shutdown / request abort, not a redirect
                        &agent_cancel,
                        &claude_text_buf,
                        last_text_persisted_len,
                        &meta,
                        &external_terminal_emitted,
                        &normalized_model,
                        &cc_reasoning_effort,
                        coding_agent,
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
                        WATCHDOG_HUNG_TOOL_CEILING_MS,
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
                                "[AgentSession] [Watchdog DIAG] thread={} elapsed={}s is_waiting={} tools_in_flight={} session_present={} gate={}",
                                thread_id,
                                elapsed_ms / 1000,
                                is_waiting,
                                tools_in_flight_snapshot,
                                session_present,
                                gate.diag_tag(),
                            );
                        }
                        let should_fire = match gate {
                            WatchdogGate::Fire => true,
                            WatchdogGate::FirePastCeiling(tif) => {
                                // A tool has been "in flight" past the hung-tool
                                // ceiling. Distinguish a genuinely hung tool
                                // (thread still `running`) from a pending
                                // question/permission card the user simply
                                // hasn't answered yet — those flip the thread to
                                // `waiting_for_user_answer` but are deliberately
                                // counted in `tools_in_flight` to disarm the
                                // normal watchdog. Only the former is
                                // euthanizable; the user may take arbitrarily
                                // long to answer.
                                let still_running =
                                    crate::engine::claude_code::thread_is_running(
                                        self.pool(),
                                        thread_id,
                                    )
                                    .await
                                    .unwrap_or(false);
                                if !still_running {
                                    log!(
                                        "[AgentSession] Watchdog: thread {} past hung-tool ceiling with {} tool(s) in flight but no longer `running` (awaiting user answer / already settled) — not firing",
                                        thread_id,
                                        tif
                                    );
                                }
                                still_running
                            }
                            _ => false,
                        };
                        if should_fire {
                            log!(
                                "[AgentSession] Watchdog ({}): no events for {}s while mid-turn for thread {} (hung subprocess / hung tool) — killing the coding-agent subprocess; will auto-resume via ContinuationRequested once events_rx EOFs",
                                gate.diag_tag(),
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
            external_continuation_requested,
            &agent_cancel,
            emitted_terminal_event,
            watchdog_fired,
            killed_by_signal,
            last_emitted_idle,
            is_external_repo,
            proposed_change,
            coding_agent,
        )
        .await
    }
}

/// What the spawn knows about the worktree it is resuming into. Grouped rather
/// than passed as four more parameters: they all come from the same
/// `SpawnWorktreeContext` resolution and are only ever read together.
pub(super) struct ResumeSpawnContext<'a> {
    /// The session's worktree on disk, when it has one.
    pub worktree_path: Option<&'a Path>,
    /// `worktree_head_sha` from the last `CodingAgentIdled`, the baseline the
    /// external-edit detector diffs against. `None` on a first turn.
    pub last_idle_sha: Option<&'a str>,
    /// Pre-built note from `try_adopt_renegade_branch`, when the worktree was
    /// switched to a branch holding this agent's work.
    pub adoption_note: Option<&'a str>,
    /// The branch this session resumes on, so a discard elsewhere can be told
    /// apart from a discard of the agent's own work.
    pub session_branch: Option<&'a str>,
}

/// Build the text handed to the resumed agent's input channel: the user's
/// message, optionally prefixed with up to three resume-time notes that
/// reconcile what changed while the agent was idle:
///
/// 1. **branch adoption**: the worktree was switched to a new branch holding
///    the agent's work (`try_adopt_renegade_branch`),
/// 2. **turn gap**: the user or the engine resolved one of the agent's changes
///    (Apply / Discard / Revert / a failed Apply) or the cleanup worker
///    reclaimed the worktree (`turn_gap::compute_turn_gap_note`),
/// 3. **external edits**: the worktree changed under the agent
///    (`external_edits::compute_external_edit_note`).
///
/// The turn-gap note is computed FIRST because it decides whether the
/// external-edit note is allowed to report a HEAD move as unexplained. An
/// Apply, a Discard or a tier-2 worktree clean moves HEAD without anyone
/// editing a file, and the edit detector cannot tell the difference: left to
/// itself it reports "the user edited files in your worktree … HEAD moved (no
/// log available)", blaming a hand edit for something the engine did. The
/// turn-gap note states the real cause, so it passes `explains_worktree_reset`
/// down and the edit note drops that one line (and only that line).
///
/// The notes are folded into one block (joined by `\n`) and prepended to the
/// message. An empty `user_message` (warm-up/continue-signal resume) is passed
/// through untouched: notes only ride on a real turn so they can't trigger an
/// otherwise-empty LLM call.
///
/// This single assembly point serves both Claude Code and Codex.
pub(super) async fn build_resume_prompt_text(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    current_origin_id: Uuid,
    user_message: &str,
    spawn: ResumeSpawnContext<'_>,
) -> String {
    if user_message.is_empty() {
        return user_message.to_string();
    }

    let ResumeSpawnContext {
        worktree_path,
        last_idle_sha,
        adoption_note,
        session_branch,
    } = spawn;

    // What the user and the engine did to this agent's work in the gap before
    // this turn. `current_origin_id` bounds the lookup to the previous turn
    // boundary: stateless and self-clearing, see `compute_turn_gap_note`.
    let gap_note = crate::engine::agent_session::turn_gap::compute_turn_gap_note(
        pool,
        thread_id,
        current_origin_id,
        session_branch,
    )
    .await;

    let edit_note = match (worktree_path, last_idle_sha) {
        (Some(wt), Some(sha)) => {
            crate::engine::agent_session::external_edits::compute_external_edit_note(
                wt,
                Some(sha),
                gap_note.as_ref().is_some_and(|n| n.explains_worktree_reset),
            )
            .await
        }
        _ => None,
    };

    // Fold the resume-time notes into one prepended block, cause before
    // observation: the gap note explains a reset the edit note can only see the
    // effects of.
    let combined: Vec<&str> = [
        adoption_note,
        gap_note.as_ref().map(|n| n.note.as_str()),
        edit_note.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();

    if combined.is_empty() {
        return user_message.to_string();
    }

    let block = combined.join("\n");
    log!(
        "[AgentSession] Injecting resume note (adoption/turn-gap/external-edit) for thread {} ({} chars)",
        thread_id,
        block.len()
    );
    format!("{}\n\n{}", block, user_message)
}

#[cfg(test)]
mod unregistered_lucidos_root_tests {
    use super::*;

    /// The regression. On a packaged install `main_worktree()` resolves to the
    /// WORKSPACE dir; handing that back would branch the user's workspace git
    /// and call it Lucidos platform source. Refuse instead.
    #[test]
    fn refuses_when_there_is_no_source_checkout() {
        let workspace_dir = PathBuf::from("/Users/me/Library/Application Support/lucidos/ws");
        let err = unregistered_lucidos_root(workspace_dir, false)
            .expect_err("a Lucidos-source spawn with no source checkout must be refused")
            .to_string();
        assert!(
            err.contains("no source checkout"),
            "refusal must say why: {err}"
        );
        assert!(
            err.contains("data/apps/<id>"),
            "refusal must name the routes that still work: {err}"
        );
    }

    /// The fallback's legitimate case survives untouched: a dev build whose
    /// `Lucidos` registry row hasn't been written yet (very early startup).
    #[test]
    fn keeps_the_early_startup_fallback_on_a_dev_build() {
        let dev_root = PathBuf::from("/Users/me/src/lucidos");
        assert_eq!(
            unregistered_lucidos_root(dev_root.clone(), true)
                .expect("dev build must still resolve"),
            dev_root
        );
    }
}

#[cfg(test)]
mod startup_failure_tests {
    use super::*;

    #[test]
    fn recovers_driver_result_error() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        tx.send(AgentEvent::Result {
            text: String::new(),
            duration_ms: 0,
            error: Some("Failed to start Codex: No such file or directory".to_string()),
        })
        .unwrap();
        tx.send(AgentEvent::Exited {
            killed_by_signal: false,
        })
        .unwrap();
        drop(tx); // driver wound down

        assert_eq!(
            drain_startup_failure_reason(&mut rx).as_deref(),
            Some("Failed to start Codex: No such file or directory")
        );
    }

    #[test]
    fn falls_back_to_signal_kill_when_no_result_error() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        tx.send(AgentEvent::Exited {
            killed_by_signal: true,
        })
        .unwrap();
        drop(tx);

        assert_eq!(
            drain_startup_failure_reason(&mut rx).as_deref(),
            Some("coding agent process was killed by a signal during startup")
        );
    }

    #[test]
    fn none_when_no_failure_signal() {
        // Clean exit, no error, no signal — caller uses its generic message.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        tx.send(AgentEvent::Exited {
            killed_by_signal: false,
        })
        .unwrap();
        drop(tx);

        assert_eq!(drain_startup_failure_reason(&mut rx), None);
    }

    #[test]
    fn ignores_empty_error_strings() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        tx.send(AgentEvent::Result {
            text: String::new(),
            duration_ms: 0,
            error: Some("   ".to_string()),
        })
        .unwrap();
        drop(tx);

        assert_eq!(drain_startup_failure_reason(&mut rx), None);
    }
}
