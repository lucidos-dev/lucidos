use crate::core::changes;
use crate::engine::agentic_loop::should_flush;
use crate::engine::change_ops::{branch_is_hardened, now_epoch_millis};
use crate::engine::claude_code::{STALE_RESUME_ERROR, WORKTREE_WORKSPACE_MARKER};
use crate::engine::git_ops::{
    auto_commit_preserving_marker, branch_changed_files, catchup_with_main, commits_in_range,
    consume_harden_marker, default_local_branch, describe_branch_changes,
    detect_origin_default_branch, ff_merge_to_main, files_have_client_update,
    files_require_restart, git_cmd, has_branch_commits, is_external_repo_path,
    is_harden_marker_present, parse_worktree_list, resolve_main_worktree, worktree_current_branch,
    worktrees_dir,
};
use crate::engine::thread_events::{EventChannel, SessionEndReason};
use crate::engine::{AgentSession, AgentUserInput, CognosEngine, ProcessResult};
use crate::runtime::{AgentEvent, AgentInput, AgentKind};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::io_helpers::{drain_lost_followups, lost_followups_to_orphans};
use super::lifecycle::{classify_result, should_auto_end_on_idle, should_clear_waiting, TerminalKind};
use super::parsing::parse_ask_user_question_input;
use super::prompts::{
    conflict_resolution_system_prompt, external_repo_recovery_system_prompt,
    external_repo_system_prompt, recovery_system_prompt, worktree_system_prompt,
};
use super::resume::{change_description_fallback, resolve_resume_context, CC_TURN_CLOSER_EVENTS};
use super::spawn::{resolve_branch_for_resume, spawn_or_resume, BranchResolution};

impl CognosEngine {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_direct_agent(
        &self,
        request_id: Uuid,
        thread_id: Uuid,
        user_message: &str,
        user_images: Option<&[crate::api::ChatImage]>,
        origin_id: Uuid,
        cancel_token: &tokio_util::sync::CancellationToken,
        conflict_change_id: Option<Uuid>,
        recovery_worktree: Option<(PathBuf, String)>,
        repo_id: Option<String>,
        system_prompt_override: Option<String>,
        resume_session_id: Option<String>,
        cc_model: Option<String>,
        cc_reasoning_effort: Option<String>,
    ) -> Result<ProcessResult, Box<dyn std::error::Error + Send + Sync>> {
        let cc_start = std::time::Instant::now();
        let thread_id_str = thread_id.to_string();

        let meta = crate::engine::thread_events::EventMeta {
            request_event_id: Some(origin_id),
            channel: Some(EventChannel::CodingAgent),
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
                        if session
                            .msg_tx
                            .send(AgentUserInput {
                                text: user_message.to_string(),
                                images,
                                origin_event_id: Some(origin_id),
                            })
                            .is_err()
                        {
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

        // Debounce: reject if a CC session was spawned very recently for THIS thread
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

        let (mut resume_session_id, resume_branch) =
            if recovery_worktree.is_none() && conflict_change_id.is_none() {
                resolve_resume_context(self.pool(), thread_id, resume_session_id).await
            } else {
                (resume_session_id, None)
            };

        // Conflict resolution mode: run in the merge worktree (not repo root)
        let conflict_change = if let Some(cid) = conflict_change_id {
            Some(
                changes::get_by_id(&self.pool, cid)
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

        // CARGO_MANIFEST_DIR is baked at compile time. If the engine was built from
        // a git worktree (e.g. a Claude Code session worktree), it points to that
        // worktree — not the main working tree. Resolve to the main working tree at
        // runtime so CC sessions branch from main, not from a stale worktree branch.
        let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let dev_root = resolve_main_worktree(&manifest_root).await;

        // Determine repo root: external repo if repo_id is set AND its path differs
        // from the dev repo. Aliases at the dev path (e.g. a "Lucidos" repo entry
        // pointing at the cognos checkout during the rebrand) stay internal — the
        // propose-on-idle path is gated by !is_external_repo, so flagging them
        // external silently disables the Apply panel.
        let (repo_root, is_external_repo, external_repo_name) = if let Some(ref rid) = repo_id {
            let repo_uuid = Uuid::parse_str(rid)?;
            let repo = crate::core::repositories::RepositoryStore::get(&self.pool, repo_uuid)
                .await?
                .ok_or_else(|| format!("Repository {} not found", rid))?;
            let path = PathBuf::from(&repo.path);
            if !path.exists() {
                return Err(format!("Repository path does not exist: {}", repo.path).into());
            }
            let is_external = is_external_repo_path(&path, &dev_root);
            let name = if is_external { Some(repo.name) } else { None };
            (path, is_external, name)
        } else {
            (dev_root, false, None)
        };

        let workspace_name = self.workspace_name();
        let (cwd, system_prompt, branch_name, worktree_path) = if let Some((wt_path, branch)) =
            recovery_worktree
        {
            // Recovery mode: reuse an orphaned worktree from a previous session
            log!(
                "[ClaudeCode] Starting recovery session on orphaned worktree: {} (branch {})",
                wt_path.display(),
                branch
            );
            let system_prompt = if let Some(sp) = system_prompt_override {
                sp
            } else if let Some(ref name) = external_repo_name {
                external_repo_recovery_system_prompt(name, &branch)
            } else {
                recovery_system_prompt(&branch, &workspace_name)
            };
            (wt_path.clone(), system_prompt, branch, Some(wt_path))
        } else if let Some(ref change) = conflict_change {
            // Conflict mode: run in the merge worktree where the merge is in progress
            let wt_path_str = change
                .merge_worktree_path
                .as_ref()
                .ok_or("Conflict change has no merge worktree path — was the merge started?")?;
            let temp_branch = change
                .merge_temp_branch
                .as_ref()
                .ok_or("Conflict change has no merge temp branch")?;
            let cwd = PathBuf::from(wt_path_str);
            let system_prompt = conflict_resolution_system_prompt().to_string();
            (cwd.clone(), system_prompt, temp_branch.clone(), Some(cwd))
        } else {
            // Normal mode: create an isolated worktree.
            // If resuming an idle session, reuse the previous branch (preserving its changes)
            // instead of creating a fresh branch that starts empty.
            let BranchResolution {
                branch_name,
                reusing_branch,
                resume_session_id: validated_sid,
            } = resolve_branch_for_resume(
                &repo_root,
                resume_session_id,
                resume_branch.as_deref(),
            )
            .await;
            resume_session_id = validated_sid;

            // If reusing a branch, check if a worktree already exists for it
            // (e.g. left over from a previous engine session that wasn't cleaned up).
            // Reuse the existing worktree path instead of trying to create a second one.
            let existing_worktree = if reusing_branch {
                match git_cmd(&["worktree", "list", "--porcelain"], &repo_root).await {
                    Ok(o) if o.status.success() => {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        let map = parse_worktree_list(&stdout);
                        map.get(&branch_name).cloned()
                    }
                    Ok(o) => {
                        log!(
                            "[ClaudeCode] git worktree list failed: {}",
                            String::from_utf8_lossy(&o.stderr).trim()
                        );
                        None
                    }
                    Err(e) => {
                        log!("[ClaudeCode] Failed to run git worktree list: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            let wt_path = if let Some(ref existing) = existing_worktree {
                log!(
                    "[ClaudeCode] Reusing existing worktree at {} for branch {}",
                    existing.display(),
                    branch_name
                );
                existing.clone()
            } else {
                worktrees_dir(self.workspace_path())
                    .join(format!("cc-{}", uuid::Uuid::new_v4().as_simple()))
            };

            // For external repos, detect the default branch from origin.
            // Returns None when there is no origin remote (branches from HEAD instead).
            let origin_default_branch = if is_external_repo {
                detect_origin_default_branch(&repo_root).await
            } else {
                None
            };

            // Create worktree if we're not reusing an existing one
            if existing_worktree.is_none() {
                let wt_args = if reusing_branch {
                    // Existing branch — no -b flag
                    vec![
                        "-c",
                        "filter.git-crypt.smudge=",
                        "-c",
                        "filter.git-crypt.clean=",
                        "-c",
                        "filter.git-crypt.required=false",
                        "worktree",
                        "add",
                        wt_path.to_str().unwrap(),
                        &branch_name,
                    ]
                } else {
                    let mut args = vec![
                        "-c",
                        "filter.git-crypt.smudge=",
                        "-c",
                        "filter.git-crypt.clean=",
                        "-c",
                        "filter.git-crypt.required=false",
                        "worktree",
                        "add",
                        wt_path.to_str().unwrap(),
                        "-b",
                        &branch_name,
                    ];
                    if let Some(ref base_ref) = origin_default_branch {
                        args.push(base_ref);
                    }
                    args
                };
                match git_cmd(&wt_args, &repo_root).await {
                    Ok(o) if o.status.success() => {
                        if reusing_branch {
                            log!(
                                "[ClaudeCode] Resumed worktree at {} on existing branch {}",
                                wt_path.display(),
                                branch_name
                            );
                        } else {
                            log!(
                                "[ClaudeCode] Created worktree at {} on branch {}{}",
                                wt_path.display(),
                                branch_name,
                                origin_default_branch
                                    .as_ref()
                                    .map(|b| format!(" (from {})", b))
                                    .unwrap_or_default()
                            );
                        }
                    }
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                        log!("[ClaudeCode] Failed to create worktree: {}", stderr);
                        return Err(format!(
                            "Failed to create git worktree for Claude Code isolation: {}",
                            stderr
                        )
                        .into());
                    }
                    Err(e) => {
                        log!("[ClaudeCode] Failed to run git worktree: {}", e);
                        return Err(format!(
                            "Failed to create git worktree for Claude Code isolation: {}",
                            e
                        )
                        .into());
                    }
                }
            }

            log!(
                "[ClaudeCode] [TIMING] worktree created: {:?}",
                cc_start.elapsed()
            );

            // Resumed/reused worktrees may be behind main. Catch them up so they
            // run the latest scripts/, configs, and source rather than stale
            // copies that pre-date recent fixes. New worktrees branched from
            // origin/main are already up to date and skip this.
            if !is_external_repo && (reusing_branch || existing_worktree.is_some()) {
                if let Err(e) = catchup_with_main(&wt_path).await {
                    log!(
                        "[ClaudeCode] catchup_with_main failed for {} ({}) -- worktree may be running stale scripts",
                        wt_path.display(),
                        e
                    );
                }
            }

            // Copy node_modules from main repo to worktree so frontend tests work.
            // Much faster than `npm ci` (~2s copy vs 2-10min install).
            // Can't symlink: npm install in the worktree follows the symlink
            // and corrupts/deletes the main repo's real node_modules.
            if !is_external_repo {
                let wt_node_modules = wt_path.join("crates/cognos-app/node_modules");
                if !wt_node_modules.exists() {
                    let src_node_modules = repo_root.join("crates/cognos-app/node_modules");
                    if src_node_modules.exists() {
                        log!("[ClaudeCode] Linking node_modules to worktree...");
                        // Try hardlinks first (instant, zero disk space).
                        // Falls back to full copy if hardlinks fail (e.g. cross-filesystem).
                        let hl = tokio::process::Command::new("cp")
                            .args([
                                "-al",
                                &src_node_modules.to_string_lossy(),
                                &wt_node_modules.to_string_lossy(),
                            ])
                            .output()
                            .await;
                        let ok = matches!(&hl, Ok(o) if o.status.success());
                        if ok {
                            log!("[ClaudeCode] node_modules hardlinked to worktree");
                        } else {
                            match tokio::process::Command::new("cp")
                                .args([
                                    "-a",
                                    &src_node_modules.to_string_lossy(),
                                    &wt_node_modules.to_string_lossy(),
                                ])
                                .output()
                                .await
                            {
                                Ok(o) if o.status.success() => {
                                    log!("[ClaudeCode] node_modules copied to worktree");
                                }
                                Ok(o) => log!(
                                    "[ClaudeCode] cp node_modules failed: {}",
                                    String::from_utf8_lossy(&o.stderr).trim()
                                ),
                                Err(e) => log!("[ClaudeCode] Failed to copy node_modules: {}", e),
                            }
                        }
                    } else {
                        log!(
                            "[ClaudeCode] No node_modules in main repo to copy, running npm ci..."
                        );
                        match tokio::process::Command::new("npm")
                            .args(["ci", "--prefer-offline"])
                            .current_dir(wt_path.join("crates/cognos-app"))
                            .output()
                            .await
                        {
                            Ok(o) if o.status.success() => {
                                log!("[ClaudeCode] node_modules installed in worktree");
                            }
                            Ok(o) => log!(
                                "[ClaudeCode] npm ci failed in worktree: {}",
                                String::from_utf8_lossy(&o.stderr).trim()
                            ),
                            Err(e) => log!("[ClaudeCode] Failed to run npm ci in worktree: {}", e),
                        }
                    }
                }
            }

            log!(
                "[ClaudeCode] [TIMING] node_modules setup done: {:?}",
                cc_start.elapsed()
            );

            // Tag the worktree with the owning workspace so orphan recovery
            // only cleans up worktrees belonging to this workspace.
            let marker = wt_path.join(WORKTREE_WORKSPACE_MARKER);
            let ws_id = self.workspace_path.to_string_lossy().to_string();
            let marker_content = if let Some(ref rid) = repo_id {
                format!("{}\n{}", ws_id, rid)
            } else {
                ws_id
            };
            if let Err(e) = tokio::fs::write(&marker, &marker_content).await {
                log!("[ClaudeCode] Failed to write workspace marker: {}", e);
            }

            // Add marker to worktree's git exclude so external repos don't
            // see it as an untracked file.
            let git_dir = wt_path.join(".git");
            let exclude_dir = if tokio::fs::metadata(&git_dir)
                .await
                .map(|m| m.is_file())
                .unwrap_or(false)
            {
                if let Ok(content) = tokio::fs::read_to_string(&git_dir).await {
                    content
                        .trim()
                        .strip_prefix("gitdir: ")
                        .map(|p| wt_path.join(p).join("info"))
                } else {
                    None
                }
            } else {
                Some(git_dir.join("info"))
            };
            if let Some(info_dir) = exclude_dir {
                if let Err(e) = tokio::fs::create_dir_all(&info_dir).await {
                    log!("[ClaudeCode] Failed to create git info dir: {}", e);
                } else {
                    let exclude_file = info_dir.join("exclude");
                    let already_excluded = tokio::fs::read_to_string(&exclude_file)
                        .await
                        .map(|c| c.lines().any(|l| l.trim() == WORKTREE_WORKSPACE_MARKER))
                        .unwrap_or(false);
                    if !already_excluded {
                        match tokio::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&exclude_file)
                            .await
                        {
                            Ok(mut f) => {
                                if let Err(e) = f
                                    .write_all(
                                        format!("\n{}\n", WORKTREE_WORKSPACE_MARKER).as_bytes(),
                                    )
                                    .await
                                {
                                    log!("[ClaudeCode] Failed to write git exclude: {}", e);
                                }
                            }
                            Err(e) => log!("[ClaudeCode] Failed to open git exclude: {}", e),
                        }
                    }
                }
            }

            let cwd = wt_path.clone();
            let system_prompt = if let Some(ref name) = external_repo_name {
                let base = origin_default_branch.as_deref().unwrap_or("origin/main");
                external_repo_system_prompt(name, &branch_name, base)
            } else {
                worktree_system_prompt(&branch_name, &workspace_name)
            };
            (cwd, system_prompt, branch_name, Some(wt_path))
        };

        // Append thread history as context so new CC sessions in an existing thread
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
            AgentKind::ClaudeCode,
            crate::runtime::SpawnArgs {
                worktree_path: &cwd,
                workspace_path: self.workspace_path(),
                allowed_tools: Some(&allowed_tools),
                system_prompt: Some(&system_prompt),
                resume_session_id: resume_session_id.as_deref(),
                model: cc_model.as_deref(),
                reasoning_effort: cc_reasoning_effort.as_deref(),
                thread_id,
            },
            agent_cancel.clone(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                if let Some(ref wt) = worktree_path {
                    let _ = git_cmd(
                        &["worktree", "remove", "--force", wt.to_str().unwrap()],
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
        let has_content =
            !user_message.is_empty() || user_images.is_some_and(|imgs| !imgs.is_empty());
        if has_content {
            let images = user_images.map(|imgs| imgs.to_vec()).unwrap_or_default();
            if agent_input_tx
                .send(AgentInput {
                    text: user_message.to_string(),
                    images,
                })
                .is_err()
            {
                if let Some(ref wt) = worktree_path {
                    let _ = git_cmd(
                        &["worktree", "remove", "--force", wt.to_str().unwrap()],
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
        let cancel = Arc::new(tokio::sync::Notify::new());
        let interrupt = Arc::new(tokio::sync::Notify::new());
        let idle_notify = Arc::new(tokio::sync::Notify::new());
        let shutting_down = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut normalized_model = cc_model.clone();
        {
            let mut sessions = self.agent_sessions.lock().await;
            let session = AgentSession {
                msg_tx: msg_tx.clone(),
                is_waiting: false,
                has_changes: false,
                requires_restart: false,
                auto_apply: false,
                discard: false,
                cancel: cancel.clone(),
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
                control_tx: agent_control_tx.clone(),
                builtin_commands: prev_builtin,
                skill_commands: prev_skill,
                current_model: normalized_model.clone(),
                current_reasoning_effort: cc_reasoning_effort.clone(),
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
        let _ = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::SessionStarted {
                    session_id: String::new(),
                    branch: branch_name.clone(),
                    repo_id: repo_id.clone(),
                },
                meta: meta.clone(),
            })
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
                        agent: crate::runtime::AgentKind::ClaudeCode,
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

        loop {
            tokio::select! {
                event_opt = events_rx.recv() => {
                    let Some(ev) = event_opt else {
                        // Driver task exited without sending Exited (defensive — should not happen).
                        break;
                    };
                    if let AgentEvent::Exited = ev {
                        // Final flush of any pending text
                        if !claude_text_buf.is_empty() {
                            let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                            if !delta.is_empty() {
                                let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), agent: crate::runtime::AgentKind::ClaudeCode },
                                    meta: meta.clone(),
                                }).await;
                            }
                        }
                        if is_waiting {
                            // CC process exited after producing a Result — session is idle.
                            // Don't hold the ThreadGuard waiting for follow-ups. Instead,
                            // auto-commit, remove from sessions map, and return. The worktree
                            // and branch persist on disk so follow-ups can reuse them via a
                            // new run_direct_agent call. This makes engine shutdown
                            // instant (no idle loop to cancel).
                            log!("[ClaudeCode] CC process exited while idle — releasing thread {}", thread_id);

                            // Auto-commit any uncommitted changes so they survive on disk.
                            if let Some(ref wt) = worktree_path {
                                auto_commit_preserving_marker(&self.pool, wt, &repo_root, &branch_name, "Claude Code changes (auto-committed on idle exit)").await;
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
                        break;
                    }
                    // Stamp liveness — used by apply_now's timeout
                    {
                        let guard = self.agent_sessions.lock().await;
                        if let Some(s) = guard.get(&thread_id) {
                            s.last_event_at.store(now_epoch_millis(), std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    let was_waiting = is_waiting;
                    let mut saw_result = false;
                    match ev {
                        AgentEvent::Init { session_id: cc_sid, model: init_model, slash_commands: cmds, skills } => {
                            log!("[ClaudeCode] [TIMING] Init event received: {:?}", cc_start.elapsed());
                            // Enable --resume for follow-ups and engine restart
                            let (cache_update, needs_settings_event) = {
                                let mut sessions = self.agent_sessions.lock().await;
                                if let Some(s) = sessions.get_mut(&thread_id) {
                                    s.cc_session_id = Some(cc_sid.clone());
                                    // Always update from Init — CC reports the actual
                                    // full model ID (e.g. "claude-opus-4-6"), which is
                                    // authoritative over any alias the user selected.
                                    let mut needs_event = false;
                                    if let Some(ref m) = init_model {
                                        let norm = crate::runtime::claude_code::normalize_cc_model_id(m).to_string();
                                        let changed = normalized_model.as_deref() != Some(norm.as_str());
                                        s.current_model = Some(norm.clone());
                                        normalized_model = Some(norm);
                                        needs_event = changed;
                                    }
                                    let skill_set: std::collections::HashSet<&str> = skills.iter().map(String::as_str).collect();
                                    s.builtin_commands = cmds.into_iter()
                                        .filter(|c: &String| !skill_set.contains(c.as_str()))
                                        .collect();
                                    s.skill_commands = skills;
                                    (Some(s.to_commands_info()), needs_event)
                                } else {
                                    (None, false)
                                }
                            };
                            // Update per-repo cache outside sessions lock to avoid nested locks
                            if let Some(info) = cache_update {
                                let repo_key = repo_root.to_string_lossy().to_string();
                                self.upsert_cc_commands_cache(repo_key, info).await;
                            }
                            // Persist the authoritative model ID from Init so idle
                            // exchanges show the full model name in the frontend.
                            if needs_settings_event {
                                if let Err(e) = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentSettingsChanged {
                                        model: normalized_model.clone(),
                                        reasoning_effort: cc_reasoning_effort.clone(),
                                        permission_mode: None,                                    agent: crate::runtime::AgentKind::ClaudeCode,
                                    },
                                    meta: meta.clone(),
                                }).await {
                                    log!("[ClaudeCode] Failed to persist Init-resolved CodingAgentSettingsChanged for {}: {}", thread_id, e);
                                }
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
                                    let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                        thread_id,
                                        event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), agent: crate::runtime::AgentKind::ClaudeCode },
                                        meta: meta.clone(),
                                    }).await;
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
                                    let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                        thread_id,
                                        event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), agent: crate::runtime::AgentKind::ClaudeCode },
                                        meta: meta.clone(),
                                    }).await;
                                    last_text_persisted_len = claude_text_buf.len();
                                }
                            }
                            if !claude_text_buf.is_empty() {
                                claude_text_buf.push_str("\n\n");
                            }
                            if name == "AskUserQuestion" {
                                // Resume relies on the tool_use_id; an empty one breaks
                                // the answer round-trip silently.
                                if id.is_empty() {
                                    log!("[ClaudeCode] WARNING: AskUserQuestion arrived without tool_use id for thread {} — resume will fail", thread_id);
                                }
                                // Single-lock: snapshot cc_session_id and remove the
                                // session entry in one critical section. The removed
                                // value still holds an Arc to idle_notify, so waiters
                                // see the notification even after the entry is gone.
                                let cc_sid = {
                                    let mut guard = self.agent_sessions.lock().await;
                                    guard.remove(&thread_id)
                                        .map(|s| {
                                            s.idle_notify.notify_waiters();
                                            s.cc_session_id.unwrap_or_default()
                                        })
                                        .unwrap_or_default()
                                };
                                let (question, options) = parse_ask_user_question_input(&input);
                                let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::UserQuestionAsked {
                                        tool_use_id: id,
                                        cc_session_id: cc_sid,
                                        question,
                                        options,
                                    },
                                    meta: meta.clone(),
                                }).await;
                                log!("[ClaudeCode] AskUserQuestion intercepted for thread {} — killing subprocess and entering waiting_for_user_answer", thread_id);
                                // Kill CC so it stops blocking on a tool_result that
                                // will never arrive over this stdin. Resume happens via
                                // POST /api/claude-code/answer-question, which spawns a fresh
                                // subprocess with --resume + matching tool_result.
                                agent_cancel.cancel();
                                return Ok(ProcessResult {
                                    response: String::new(),
                                    steps: vec![],
                                    images: vec![],
                                    request_id,
                                    thread_id,
                                    proposed_change,
                                    auto_apply: false,
                                    orphaned_injections: vec![],
                                });
                            } else {
                                let description = crate::core::describe_cc_tool(&name, &input);
                                let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentToolCalled {
                                        name,
                                        args: input,
                                        description,                                    agent: crate::runtime::AgentKind::ClaudeCode,
                                    },
                                    meta: meta.clone(),
                                }).await;
                            }
                        }
                        AgentEvent::ToolResult { output, status: _ } => {
                            let summary: String = output.chars().take(200).collect();
                            let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentToolResult {
                                    name: String::new(),
                                    result: summary,                                    agent: crate::runtime::AgentKind::ClaudeCode,
                                },
                                meta: meta.clone(),
                            }).await;
                        }
                        AgentEvent::Exited => unreachable!("Exited handled above"),
                        AgentEvent::Result { text, .. } => {
                                        saw_result = true;
                                        log!("[ClaudeCode] Result event received — entering waiting state");
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
                                                let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                                    thread_id,
                                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), agent: crate::runtime::AgentKind::ClaudeCode },
                                                    meta: meta.clone(),
                                                }).await;
                                            }
                                        } else if !text.trim().is_empty() {
                                            // Slash commands (e.g. /model) produce a Result
                                            // without any preceding Message events. Emit the
                                            // result text as CodingAgentTextStreamed so the
                                            // frontend displays it.
                                            let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: text.trim().to_string(), agent: crate::runtime::AgentKind::ClaudeCode },
                                                meta: meta.clone(),
                                            }).await;
                                        }
                                        // Detect stale resume: CC returned empty Result immediately
                                        // after resume. The session was expired and produced no output.
                                        // Abort without emitting ResponseGenerated/CodingAgentIdled so
                                        // the caller can retry with a fresh session.
                                        if resume_session_id.is_some()
                                            && text.trim().is_empty()
                                            && claude_text_buf.trim().is_empty()
                                            && result_texts.is_empty()
                                            && !user_message.is_empty()
                                        {
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
                                            // Emit SessionEnded so the auto-detect resume query
                                            // finds SessionEnded (not CodingAgentIdled) and starts
                                            // a fresh session on retry.
                                            let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: crate::engine::thread_events::ThreadEvent::SessionEnded {
                                                    reason: SessionEndReason::StaleResume,
                                                },
                                                meta: meta.clone(),
                                            }).await;
                                            // Clean up the worktree and branch so the retry
                                            // starts fresh (otherwise orphaned on disk until
                                            // engine restart).
                                            if let Some(ref wt) = worktree_path {
                                                let _ = git_cmd(&["worktree", "remove", "--force", wt.to_str().unwrap()], &repo_root).await;
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
                                            user_message.is_empty(),
                                            user_hit_stop,
                                            is_shutdown,
                                        );
                                        if let Some(kind) = terminal_kind {
                                            if kind == TerminalKind::Aborted {
                                                // Reset on next user follow-up.
                                                user_hit_stop = false;
                                            }
                                            let terminal_event = Self::make_terminal_event(
                                                kind,
                                                text.clone(),
                                                normalized_model.clone(),
                                                cc_reasoning_effort.clone(),
                                            );
                                            let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                                thread_id,
                                                event: terminal_event,
                                                meta: meta.clone(),
                                            }).await;
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
                                            let diff = git_cmd(&["diff", "--name-only", "main...HEAD"], worktree_path.as_ref().unwrap()).await;
                                            let changed_files: Vec<String> = diff
                                                .as_ref()
                                                .ok()
                                                .filter(|o| o.status.success())
                                                .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(|l| l.to_string()).collect())
                                                .unwrap_or_default();
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
                                        // Empty message (silent resume / warm-up): the previous
                                        // CodingAgentIdled already has the correct cc_session_id.
                                        // Shutdown: emitting idle would make recover_orphaned_worktrees
                                        // skip this session as "truly idle" and break recovery.
                                        if emit_idle {
                                            self.emit_coding_agent_idled(
                                                thread_id,
                                                wt_has_changes,
                                                is_external_repo,
                                                wt_requires_restart,
                                                &meta,
                                            ).await;
                                            last_emitted_idle = true;
                                        }

                                        // Propose the change at idle time so the Apply button
                                        // shows immediately (propose_change deduplicates). When
                                        // CC skipped /harden, hardened=false propagates to the
                                        // change record and Apply runs hardening at click time.
                                        //
                                        // Skip during shutdown: the session is being killed mid-work,
                                        // not genuinely idle. Snapshotting partial work as a pending
                                        // change creates a spurious "Change proposed" panel that the
                                        // resumed session would have to clean up on restart.
                                        if wt_has_changes && !is_external_repo && !is_shutdown {
                                            let hardened = is_harden_marker_present(&self.pool, &repo_root, &branch_name).await;
                                            let changed_files = branch_changed_files(&repo_root, &branch_name).await;
                                            if changed_files.is_empty() {
                                                log!("[ClaudeCode] Skipping proposal — branch has no changed files");
                                                if let Ok(Some(stale)) = changes::get_pending_by_branch(self.pool(), &branch_name).await {
                                                    log!("[ClaudeCode] Discarding stale pending change {} for branch {}", stale.id, branch_name);
                                                    if let Err(e) = changes::apply_change_discarded(self.pool(), stale.id).await {
                                                        log!("[ClaudeCode] Failed to discard stale pending change {}: {}", stale.id, e);
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
                                                if let Err(e) = self.propose_change(crate::engine::change_ops::ProposeChangeInput {
                                                    request_id,
                                                    thread_id,
                                                    branch_name: &branch_name,
                                                    repo_root: &repo_root_str,
                                                    description: &description,
                                                    files: &changed_files,
                                                    requires_restart,
                                                    channel: EventChannel::CodingAgent,
                                                    hardened,
                                                    // Live agent proposal — origin is
                                                    // carried by the surrounding
                                                    // MessageReceived. Engine-internal
                                                    // recovery paths stamp Engine origin
                                                    // via propose_branch_changes.
                                                    origin: None,
                                                }).await {
                                                    log!("[ClaudeCode] Failed to propose change at idle: {}", e);
                                                }
                                                self.broadcast_changes_updated().await;
                                            }
                                        }

                                        // Auto-end autonomous sessions that have no user at the keyboard:
                                        // - Conflict resolution: merge is committed, nothing to review
                                        // - Orphan recovery: runs autonomously, nobody sends follow-ups
                                        //   (with or without changes — cleanup proposes changes if any)
                                        if should_auto_end_on_idle(conflict_change.is_some()) {
                                            cancel.notify_one();
                                        }
                                    }
                                }
                            // Safety net: if CC was waiting and produced any non-Result event
                            // it has resumed work. This catches ToolResult, Init, or anything
                            // else that didn't already clear waiting above (Message/ToolUse).
                            if should_clear_waiting(was_waiting && is_waiting, saw_result) {
                                log!("[ClaudeCode] Clearing waiting — CC produced an event");
                                is_waiting = false;
                                last_emitted_idle = false;
                                {
                                    let mut sessions = self.agent_sessions.lock().await;
                                    if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                                }
                                // Update thread status to 'running' so the thread moves
                                // from REVIEW back to IN PROGRESS. Without this, the
                                // status stays 'waiting' (from CodingAgentIdled) and the
                                // thread sits in REVIEW while CC is actively working.
                                let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::CodingAgentPromptSent {
                                        text: String::new(),
                                        agent: crate::runtime::AgentKind::ClaudeCode,
                                        // Status-transition marker emitted from a live
                                        // session — origin is carried by the surrounding
                                        // MessageReceived.
                                        origin: None,
                                    },
                                    meta: meta.clone(),
                                }).await;
                                // Notify frontend that CC resumed (TextStreaming
                                // triggers cc-waiting → cc-working transition)
                                let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::TextStreaming { text: claude_text_buf.clone() },
                                    meta: crate::engine::thread_events::EventMeta::NONE,
                                }).await;
                            }
                }

                Some(user_input) = msg_rx.recv() => {
                    is_waiting = false;
                    last_emitted_idle = false;
                    // Reset cancel flag on user interaction so the session resumes normally.
                    user_hit_stop = false;
                    {
                        let mut sessions = self.agent_sessions.lock().await;
                        if let Some(s) = sessions.get_mut(&thread_id) { s.is_waiting = false; }
                    }
                    if !claude_text_buf.is_empty() {
                        let delta = &claude_text_buf[claude_text_buf.floor_char_boundary(last_text_persisted_len)..];
                        if !delta.is_empty() {
                            let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed { text: delta.to_string(), agent: crate::runtime::AgentKind::ClaudeCode },
                                meta: meta.clone(),
                            }).await;
                        }
                        claude_text_buf.clear();
                        last_text_persisted_len = 0;
                    }

                    let images = user_input.images.clone().unwrap_or_default();
                    if agent_input_tx.send(AgentInput {
                        text: user_input.text.clone(),
                        images,
                    }).is_err() {
                        log!("Failed to forward user input to agent runtime — channel closed");
                        break;
                    }
                    // chat.rs already emitted MessageReceived with the frontend's UUID
                    // for optimistic rendering. Here we just emit CodingAgentPromptSent
                    // as an audit trail for the CC event loop.
                    let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::CodingAgentPromptSent {
                            text: user_input.text,
                            agent: crate::runtime::AgentKind::ClaudeCode,
                            // Audit trail for a user-driven prompt — origin is
                            // carried by the MessageReceived emitted at the API
                            // boundary.
                            origin: None,
                        },
                        meta: meta.clone(),
                    }).await;
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

                _ = cancel.notified() => {
                    // Kill CC process and emit terminal event
                    Self::kill_cc_and_flush(&agent_cancel, &claude_text_buf, last_text_persisted_len, &self.event_bus, thread_id, &meta).await;
                    if !is_waiting {
                        let kind = if shutting_down.load(std::sync::atomic::Ordering::Relaxed) {
                            TerminalKind::Aborted
                        } else {
                            TerminalKind::Canceled
                        };
                        let terminal_event = Self::make_terminal_event(kind, claude_text_buf.clone(), normalized_model.clone(), cc_reasoning_effort.clone());
                        let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                            thread_id, event: terminal_event, meta: meta.clone(),
                        }).await;
                    } else {
                        log!("[ClaudeCode] Shutdown: session {} was idle, skipping terminal event", thread_id);
                    }
                    emitted_terminal_event = true;
                    break;
                }

                _ = chat_cancel.cancelled() => {
                    // Upstream chat handler cancelled (engine shutdown / request abort).
                    // Cancel the runtime, flush partial text, emit terminal event.
                    Self::kill_cc_and_flush(
                        &agent_cancel,
                        &claude_text_buf,
                        last_text_persisted_len,
                        &self.event_bus,
                        thread_id,
                        &meta,
                    )
                    .await;
                    if !is_waiting {
                        let kind = if shutting_down.load(std::sync::atomic::Ordering::Relaxed) {
                            TerminalKind::Aborted
                        } else {
                            TerminalKind::Canceled
                        };
                        let terminal_event = Self::make_terminal_event(
                            kind,
                            claude_text_buf.clone(),
                            normalized_model.clone(),
                            cc_reasoning_effort.clone(),
                        );
                        let _ = self
                            .event_bus
                            .emit(crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: terminal_event,
                                meta: meta.clone(),
                            })
                            .await;
                    } else {
                        log!(
                            "[ClaudeCode] Cancel: session {} was idle, skipping terminal event",
                            thread_id
                        );
                    }
                    emitted_terminal_event = true;
                    break;
                }
            }
        }

        // Mark exited before emitting terminal events — forces follow-ups arriving
        // after the event loop exits to spawn a new session instead of routing to
        // a dead channel (closes the process_exited race window in chat.rs fast-path).
        {
            let mut guard = self.agent_sessions.lock().await;
            if let Some(s) = guard.get_mut(&thread_id) {
                s.process_exited = true;
                s.idle_notify.notify_waiters();
            }
        }
        self.clear_cc_debounce(thread_id);

        // Drain follow-ups queued while CC was busy. Convert to orphaned injections
        // so the caller re-processes them instead of showing "interrupted".
        let cc_orphans = lost_followups_to_orphans(drain_lost_followups(&mut msg_rx));

        // Safety net: if the CC event loop ended without emitting a terminal event
        // (e.g., process crashed, EOF without producing a Result, stream error).
        // Check whether the CC actually completed its work (committed + hardened)
        // before dying — this happens when CC hits its context limit after finishing.
        // In that case, emit ResponseGenerated (the work IS done) instead of
        // ResponseAborted (which would misleadingly show the exchange as "Aborted").
        if !emitted_terminal_event {
            let completed_without_result = worktree_path.is_some()
                && has_branch_commits(&repo_root, &branch_name).await
                && is_harden_marker_present(&self.pool, &repo_root, &branch_name).await;

            if completed_without_result {
                log!("[ClaudeCode] CC exited without Result but work is committed and hardened — treating as completed for thread {}", thread_id);
                if let Err(e) = self
                    .event_bus
                    .emit(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                            text: claude_text_buf.clone(),
                            images: vec![],
                            model: normalized_model.clone(),
                            reasoning_effort: cc_reasoning_effort.clone(),
                        },
                        meta: meta.clone(),
                    })
                    .await
                {
                    log!("[ClaudeCode] Failed to emit safety-net ResponseGenerated for thread {}: {}", thread_id, e);
                }
            } else {
                if let Err(e) = self
                    .event_bus
                    .emit(crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ResponseAborted {
                            text: claude_text_buf.clone(),
                            images: vec![],
                            model: normalized_model.clone(),
                            reasoning_effort: cc_reasoning_effort.clone(),
                        },
                        meta: meta.clone(),
                    })
                    .await
                {
                    log!(
                        "[ClaudeCode] Failed to emit safety-net ResponseAborted for thread {}: {}",
                        thread_id,
                        e
                    );
                }
            }
        }

        // Make sure the runtime task tears down its child process — driver
        // already drained and logged stderr inside its own task.
        agent_cancel.cancel();

        // During engine shutdown, skip all cleanup — preserve the worktree and branch
        // so recover_orphaned_worktrees can resume the session after restart.
        // Read session flags before cleanup. Read discard early so we can skip
        // unnecessary work (auto-commit, hardening) when the user chose to discard.
        let should_discard = {
            let guard = self.agent_sessions.lock().await;
            guard.get(&thread_id).map(|s| s.discard).unwrap_or(false)
        };

        // Auto-commit any uncommitted changes so they survive on disk.
        {
            let is_shutdown = {
                let guard = self.agent_sessions.lock().await;
                guard
                    .get(&thread_id)
                    .map(|s| s.shutting_down.load(std::sync::atomic::Ordering::Relaxed))
                    .unwrap_or(false)
            };
            if is_shutdown {
                if let Some(ref wt) = worktree_path {
                    auto_commit_preserving_marker(
                        &self.pool,
                        wt,
                        &repo_root,
                        &branch_name,
                        "Claude Code changes (auto-committed on shutdown)",
                    )
                    .await;
                }
                let mut guard = self.agent_sessions.lock().await;
                guard.remove(&thread_id);
                log!(
                    "[Shutdown] Skipping cleanup for thread {} — session will resume after restart",
                    thread_id
                );
                return Ok(ProcessResult {
                    response: String::new(),
                    steps: vec![],
                    images,
                    request_id,
                    thread_id,
                    proposed_change: false,
                    auto_apply: false,
                    orphaned_injections: vec![],
                });
            }
        }

        if let Some(change) = conflict_change {
            // Conflict resolution cleanup — merge happened in a worktree, ff-merge to main
            let has_unmerged = git_cmd(&["diff", "--name-only", "--diff-filter=U"], &cwd)
                .await
                .map(|o| !o.stdout.is_empty())
                .unwrap_or(true);

            let wt_str = cwd.to_str().unwrap();
            let temp_branch = change
                .merge_temp_branch
                .as_deref()
                .unwrap_or(&change.branch_name);

            if has_unmerged {
                let _ = git_cmd(&["merge", "--abort"], &cwd).await;
                log!(
                    "Conflict resolution incomplete for {} — merge aborted in worktree",
                    change.branch_name
                );
                let _ = git_cmd(&["worktree", "remove", "--force", wt_str], &repo_root).await;
                let _ = git_cmd(&["branch", "-D", temp_branch], &repo_root).await;
                let _ = changes::clear_merge_worktree(&self.pool, change.id).await;
                let _ = self.event_bus.emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id: change.thread_id.unwrap_or(thread_id),
                    event: crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                        change_id: change.id.to_string(),
                        error: "Conflict resolution incomplete — merge aborted. The change is still pending; try applying again.".to_string(),
                        actor: None,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                }).await;
            } else {
                // Ensure merge is committed
                let merge_committed = git_cmd(&["rev-parse", "MERGE_HEAD"], &cwd)
                    .await
                    .map(|o| !o.status.success())
                    .unwrap_or(false);
                if !merge_committed {
                    let _ = git_cmd(&["commit", "--no-edit"], &cwd).await;
                }

                // Remove worktree and ff-merge to main
                match ff_merge_to_main(&repo_root, wt_str, temp_branch, &change.branch_name).await {
                    Ok((pre_sha, post_sha)) => {
                        let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                        if let Err(e) =
                            changes::set_merge_shas(&self.pool, change.id, &pre_sha, &post_sha)
                                .await
                        {
                            log!(
                                "[Changes] Failed to store merge SHAs for {}: {}",
                                change.id,
                                e
                            );
                        }
                        // The conflict-resolution path drops the original
                        // applier's actor across the async gap (Apply HTTP
                        // call returned long ago, CC then resolved conflicts,
                        // we're now ff-merging). Stamp `None` rather than
                        // fabricate an Engine attribution — there is no
                        // engine-driven apply; this gap is a known limitation
                        // until the actor is plumbed through the session.
                        self.emit_change_applied(
                            change.thread_id.unwrap_or(thread_id),
                            change.id,
                            change.requires_restart,
                            files_have_client_update(&change.files),
                            commits,
                            change.thread_title.clone(),
                            None,
                        )
                        .await;
                        let _ = changes::clear_merge_worktree(&self.pool, change.id).await;
                        log!(
                            "Conflict resolution complete — change {} applied via ff-merge",
                            change.id
                        );
                    }
                    Err(e) => {
                        let _ = changes::clear_merge_worktree(&self.pool, change.id).await;
                        log!("ff-merge failed after conflict resolution: {}", e);
                        let _ = self
                            .event_bus
                            .emit(crate::engine::event_bus::BusEvent::Thread {
                                thread_id: change.thread_id.unwrap_or(thread_id),
                                event:
                                    crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                        change_id: change.id.to_string(),
                                        error: format!(
                                            "Merge failed after conflict resolution: {}",
                                            e
                                        ),
                                        actor: None,
                                    },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            })
                            .await;
                    }
                }
            }
        } else {
            // Normal worktree cleanup
            let wt = worktree_path.as_ref().unwrap();

            if !should_discard {
                // Only auto-commit if we're keeping the changes.
                self.commit_dirty_logged("Claude Code changes", "Claude Code cleanup")
                    .await;
                auto_commit_preserving_marker(&self.pool, wt, &repo_root, &branch_name, "Claude Code changes (auto-committed)").await;
            }

            if should_discard {
                // User chose "Discard & End Session" — remove worktree, delete branch
                // Discard any pending change for this branch so the frontend doesn't show it as waiting
                if let Ok(Some(change)) =
                    changes::get_pending_by_branch(&self.pool, &branch_name).await
                {
                    let _ = self
                        .event_bus
                        .emit(crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ChangeDiscarded {
                                change_id: change.id.to_string(),
                                actor: None,
                                path: String::new(),
                            },
                            meta: meta.clone(),
                        })
                        .await;
                    let _ = changes::apply_change_discarded(&self.pool, change.id).await;
                }
                let wt_path_str = wt.to_str().unwrap();
                if let Err(e) =
                    git_cmd(&["worktree", "remove", "--force", wt_path_str], &repo_root).await
                {
                    log!("{}", e);
                }
                log!("[ClaudeCode] Discarding changes (branch {})", branch_name);
                if let Err(e) = git_cmd(&["branch", "-D", &branch_name], &repo_root).await {
                    log!(
                        "[ClaudeCode] Failed to delete branch {}: {}",
                        branch_name,
                        e
                    );
                }
            } else {
                // Detect if the CC session switched to a different branch inside the
                // worktree (e.g. created a feature branch in an external repo). If so,
                // use the actual branch for the change proposal instead of the tracked
                // claude-code/* branch — that branch has no commits.
                let actual_branch = worktree_current_branch(wt).await;
                let base = default_local_branch(&repo_root).await;
                let effective_branch = if let Some(ref actual) = actual_branch {
                    // Only use the actual branch if CC switched to a real feature branch,
                    // not if it ended up on the default branch (main/master) — otherwise
                    // the cleanup path could delete main.
                    if actual != &branch_name && actual != &base {
                        log!("[ClaudeCode] Worktree is on branch '{}', tracked branch was '{}' — using actual branch",
                            actual, branch_name);
                        actual.as_str()
                    } else {
                        branch_name.as_str()
                    }
                } else {
                    branch_name.as_str()
                };

                let was_hardened = branch_is_hardened(&self.pool, &repo_root, effective_branch).await;
                let has_commits = has_branch_commits(&repo_root, effective_branch).await;

                // Remove the worktree directory (the branch stays)
                let wt_path_str = wt.to_str().unwrap();
                if let Err(e) =
                    git_cmd(&["worktree", "remove", "--force", wt_path_str], &repo_root).await
                {
                    log!("{}", e);
                }

                if has_commits && is_external_repo {
                    log!(
                        "[ClaudeCode] External repo branch {} — keeping branch, no change proposed",
                        effective_branch
                    );
                } else if has_commits {
                    let changed_files = branch_changed_files(&repo_root, effective_branch).await;
                    let requires_restart = files_require_restart(&changed_files);

                    log!(
                        "Storing change on branch {}{}",
                        effective_branch,
                        if requires_restart {
                            " (requires restart)"
                        } else {
                            ""
                        }
                    );
                    let repo_root_str = repo_root.to_string_lossy().to_string();

                    let fallback =
                        change_description_fallback(self.pool(), thread_id, effective_branch).await;
                    let base = default_local_branch(&repo_root).await;
                    let log_range = format!("{}..{}", base, effective_branch);
                    let description =
                        describe_branch_changes(&repo_root, &log_range, &fallback, None).await;

                    match self
                        .propose_change(crate::engine::change_ops::ProposeChangeInput {
                            request_id,
                            thread_id,
                            branch_name: effective_branch,
                            repo_root: &repo_root_str,
                            description: &description,
                            files: &changed_files,
                            requires_restart,
                            channel: EventChannel::CodingAgent,
                            hardened: was_hardened,
                            // Live agent proposal at session end — origin is
                            // carried by the surrounding MessageReceived.
                            origin: None,
                        })
                        .await
                    {
                        Ok(_change_id) => {
                            proposed_change = true;
                            if was_hardened {
                                consume_harden_marker(&self.pool, &repo_root, effective_branch).await;
                            }
                            if !last_emitted_idle {
                                self.emit_coding_agent_idled(
                                    thread_id,
                                    !changed_files.is_empty(),
                                    is_external_repo,
                                    requires_restart,
                                    &meta,
                                )
                                .await;
                            }
                        }
                        Err(e) => {
                            log!("Failed to propose change: {}", e);
                        }
                    }
                } else {
                    // No commits on the effective branch — clean up the tracked branch
                    // if it's different (CC switched branches, original has no changes).
                    if effective_branch != branch_name {
                        let tracked_has_pending =
                            changes::has_pending_for_branch(&self.pool, &branch_name)
                                .await
                                .unwrap_or(true);
                        if !tracked_has_pending {
                            let _ = git_cmd(&["branch", "-D", &branch_name], &repo_root).await;
                        }
                    }
                    let has_pending =
                        changes::has_pending_for_branch(&self.pool, effective_branch)
                            .await
                            .unwrap_or(true);
                    if !has_pending {
                        let _ = git_cmd(&["branch", "-D", effective_branch], &repo_root).await;
                    }
                }
            }
        }

        // Read auto_apply and remove session at the very end of cleanup.
        // Keeping the session in the map during git/change-proposal work lets the
        // cancel endpoint set auto_apply even if the user clicks "Apply Now" while
        // cleanup is already in progress (avoids a 404 race).
        let auto_apply = {
            let mut guard = self.agent_sessions.lock().await;
            let val = guard.get(&thread_id).map(|s| s.auto_apply).unwrap_or(false);
            guard.remove(&thread_id);
            val
        };

        // SessionEnded is emitted after all cleanup (change proposal, worktree removal)
        // so it's always the last event in the thread. This ensures ChangeProposed
        // appears before SessionEnded, and the frontend sees the correct final state.
        // Shutdown sessions bail out earlier (skip cleanup), so this only runs for
        // normal completions.
        let session_reason = if should_discard {
            SessionEndReason::Discarded
        } else if proposed_change {
            SessionEndReason::ChangesProposed
        } else {
            SessionEndReason::Completed
        };
        let _ = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::SessionEnded {
                    reason: session_reason,
                },
                meta: meta.clone(),
            })
            .await;

        // CC text was already streamed via ClaudeCodeText progress events.
        // Return empty response — frontend uses streamingResponse (ccText) as final content.
        Ok(ProcessResult {
            response: String::new(),
            steps: vec![],
            images,
            request_id,
            thread_id,
            proposed_change,
            auto_apply,
            orphaned_injections: cc_orphans,
        })
    }

    async fn emit_coding_agent_idled(
        &self,
        thread_id: Uuid,
        has_changes: bool,
        is_external_repo: bool,
        requires_restart: bool,
        meta: &crate::engine::thread_events::EventMeta,
    ) {
        let cc_session_id = {
            let guard = self.agent_sessions.lock().await;
            guard.get(&thread_id).and_then(|s| s.cc_session_id.clone())
        };
        let _ = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                    has_changes,
                    is_external_repo,
                    requires_restart,
                    cc_session_id,
                    agent: crate::runtime::AgentKind::ClaudeCode,
                },
                meta: meta.clone(),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    /// Verifies that `try_wait()` detects a dead child process even when
    /// the stdout pipe hasn't produced EOF. This is the watchdog that
    /// prevents threads from getting stuck in RUNNING state after the CC
    /// process is killed (e.g. macOS sleep killing the process).
    #[tokio::test]
    async fn try_wait_detects_dead_cc_process() {
        use tokio::process::Command;

        // Spawn a short-lived process
        let mut child = Command::new("echo")
            .arg("done")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn 'echo'");

        // Take stdin/stdout (same as the real CC code does)
        let _stdin = child.stdin.take();
        let _stdout = child.stdout.take();

        // Wait for process to exit (with explicit wait, not just sleep)
        let _ = child.wait().await;

        // try_wait should report the exit status
        let status = child.try_wait().expect("try_wait should not error");
        assert!(
            status.is_some(),
            "try_wait must detect dead process even after stdin/stdout are taken"
        );
        assert!(
            status.unwrap().success(),
            "process should have exited successfully"
        );
    }

    /// Verifies that `try_wait()` returns None for a still-running process.
    /// This ensures the watchdog doesn't false-positive on healthy CC sessions.
    #[tokio::test]
    async fn try_wait_returns_none_for_running_process() {
        use tokio::process::Command;

        let mut child = Command::new("sleep")
            .arg("10")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn 'sleep'");

        let _stdin = child.stdin.take();
        let _stdout = child.stdout.take();

        let status = child.try_wait().expect("try_wait should not error");
        assert!(
            status.is_none(),
            "try_wait must return None for a running process"
        );

        // Clean up
        let _ = child.kill().await;
    }
}
