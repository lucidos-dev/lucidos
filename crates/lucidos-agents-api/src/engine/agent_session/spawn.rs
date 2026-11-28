use crate::engine::agent_session::external_edits::try_adopt_renegade_branch;
use crate::engine::agent_session::resume::IdleAnchor;
use crate::engine::git_ops::{
    allocate_coding_agent_branch, branch_name_is_taken, git_answer, worktree_add, BranchScope,
    GitAnswer,
};
use crate::engine::LucidosEngine;
use crate::runtime::{CodingAgent, RunningAgent, SpawnArgs};
use std::path::Path;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResumeAttempt {
    NotAttempted,
    Resumed,
    FellBack,
}

pub(super) async fn spawn_or_resume(
    engine: &LucidosEngine,
    agent: CodingAgent,
    args: SpawnArgs<'_>,
    cancel: CancellationToken,
) -> Result<(RunningAgent, ResumeAttempt), Box<dyn std::error::Error + Send + Sync>> {
    let runtime = engine
        .agent_runtimes
        .get(&agent)
        .ok_or_else(|| format!("No registered runtime for agent {:?}", agent))?;
    engine.session_spawn_gate.wait().await;
    if args.resume_session_id.is_some() {
        let mut fresh = args.clone();
        match runtime.spawn(args, cancel.clone()).await {
            Ok(rt) => return Ok((rt, ResumeAttempt::Resumed)),
            Err(e) => {
                log_legacy!(
                    "[AgentSession] Resume failed ({}), falling back to fresh spawn",
                    e
                );
            }
        }
        fresh.resume_session_id = None;
        let rt = runtime.spawn(fresh, cancel).await?;
        return Ok((rt, ResumeAttempt::FellBack));
    }
    let rt = runtime.spawn(args, cancel).await?;
    Ok((rt, ResumeAttempt::NotAttempted))
}

const MAX_BRANCH_CREATE_ATTEMPTS: usize = 10;

pub(super) struct FreshBranch<'a> {
    pub(super) repo_root: &'a Path,
    pub(super) agent: CodingAgent,
    pub(super) scope: BranchScope,
    pub(super) thread_name: &'a str,
    pub(super) thread_id: uuid::Uuid,
}

impl FreshBranch<'_> {
    pub(super) async fn allocate(&self) -> String {
        allocate_coding_agent_branch(
            self.repo_root,
            self.agent,
            &self.scope,
            self.thread_name,
            self.thread_id,
        )
        .await
    }

    async fn resolution(&self) -> BranchResolution {
        BranchResolution {
            branch_name: self.allocate().await,
            reusing_branch: false,
            resume_session_id: None,
            gone: None,
        }
    }

    pub(super) async fn create_worktree(
        &self,
        wt_path: &Path,
        base_ref: Option<&str>,
        allocated: String,
    ) -> Result<String, String> {
        let mut branch_name = allocated;
        for attempt in 1..=MAX_BRANCH_CREATE_ATTEMPTS {
            let mut args = vec!["-b", &branch_name];
            if let Some(base) = base_ref {
                args.push(base);
            }
            let stderr = match worktree_add(self.repo_root, wt_path, &args).await {
                Ok(o) if o.status.success() => return Ok(branch_name),
                Ok(o) => String::from_utf8_lossy(&o.stderr).trim().to_string(),
                Err(e) => return Err(e),
            };
            if !branch_name_is_taken(&stderr) {
                return Err(stderr);
            }
            log_legacy!(
                "[AgentSession] Branch {} was taken by a concurrent spawn (attempt {}/{}), re-deriving",
                branch_name,
                attempt,
                MAX_BRANCH_CREATE_ATTEMPTS
            );
            branch_name = self.allocate().await;
        }
        Err(format!(
            "gave up after {} attempts to create a branch for this thread: a concurrent spawn took every name, last tried {}",
            MAX_BRANCH_CREATE_ATTEMPTS, branch_name
        ))
    }
}

pub(super) struct BranchResolution {
    pub branch_name: String,
    pub reusing_branch: bool,
    pub resume_session_id: Option<String>,
    pub gone: Option<GoneBranch>,
}

pub(super) struct GoneBranch {
    pub branch_name: String,
    pub session_id: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RecoveredBranch {
    pub branch_name: String,
    pub adoption_note: String,
    pub session_id: Option<String>,
}

pub(super) async fn recover_renamed_branch(
    repo_root: &Path,
    worktree_path: &Path,
    gone: GoneBranch,
    anchor: &IdleAnchor,
) -> Option<RecoveredBranch> {
    let (branch_name, adoption_note) =
        try_adopt_renegade_branch(repo_root, worktree_path, &gone.branch_name, anchor).await?;
    log_legacy!(
        "[AgentSession] Recorded branch {} was renamed to {}, resuming on it",
        gone.branch_name,
        branch_name
    );
    Some(RecoveredBranch {
        branch_name,
        adoption_note,
        session_id: gone.session_id,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum BranchDecision {
    Reuse {
        branch_name: String,
        resume_session_id: Option<String>,
    },
    Gone {
        resume_session_id: Option<String>,
    },
}

pub(super) async fn resolve_branch_for_resume(
    fresh: &FreshBranch<'_>,
    resume_session_id: Option<String>,
    resume_branch: Option<&str>,
) -> BranchResolution {
    let Some(rb) = resume_branch else {
        return fresh.resolution().await;
    };
    let answer = git_answer(&["rev-parse", "--verify", rb], fresh.repo_root).await;
    match decide_branch_resolution(answer, rb, resume_session_id) {
        BranchDecision::Reuse {
            branch_name,
            resume_session_id,
        } => BranchResolution {
            branch_name,
            reusing_branch: true,
            resume_session_id,
            gone: None,
        },
        BranchDecision::Gone { resume_session_id } => BranchResolution {
            gone: Some(GoneBranch {
                branch_name: rb.to_string(),
                session_id: resume_session_id,
            }),
            ..fresh.resolution().await
        },
    }
}

pub(super) fn decide_branch_resolution(
    branch_exists: GitAnswer,
    resume_branch: &str,
    resume_session_id: Option<String>,
) -> BranchDecision {
    if branch_exists.is_unknown() {
        log_legacy!(
            "[AgentSession] Could not verify resume branch {} (git gave no answer). Reusing it rather than starting fresh, since dropping a branch that exists loses the session",
            resume_branch
        );
    }
    if branch_exists.or_unknown(true) {
        if !branch_exists.is_unknown() {
            log_legacy!(
                "[AgentSession] Reusing existing branch {} for resumed session",
                resume_branch
            );
        }
        BranchDecision::Reuse {
            branch_name: resume_branch.to_string(),
            resume_session_id,
        }
    } else {
        log_legacy!(
            "[AgentSession] Resume branch {} no longer exists, creating fresh branch unless the worktree proves a rename",
            resume_branch
        );
        BranchDecision::Gone { resume_session_id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::git_ops::{git_cmd, LUCIDOS_BRANCH_PREFIX};

    struct RecordingRuntime(std::sync::atomic::AtomicBool);

    #[async_trait::async_trait]
    impl crate::runtime::AgentRuntime for RecordingRuntime {
        fn kind(&self) -> CodingAgent {
            CodingAgent::Codex
        }

        async fn spawn(
            &self,
            _args: SpawnArgs<'_>,
            _cancel: CancellationToken,
        ) -> Result<RunningAgent, Box<dyn std::error::Error + Send + Sync>> {
            self.0.store(true, std::sync::atomic::Ordering::Relaxed);
            Err("recorded spawn".into())
        }
    }

    #[tokio::test]
    async fn spawn_waits_for_the_api_listener() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        use std::sync::{atomic::Ordering, Arc};

        let (engine, workspace) = crate::test_support::boot_mock_engine(&db).await;
        let mut engine = Arc::into_inner(engine).expect("the fixture owns the engine");
        let runtime = Arc::new(RecordingRuntime(Default::default()));
        engine
            .agent_runtimes
            .insert(CodingAgent::Codex, runtime.clone());
        engine.session_spawn_gate.hold();
        let thread_id = uuid::Uuid::new_v4();
        let path = workspace.path();
        let sandbox = crate::runtime::agent_sandbox::Sandbox::resolve(&pool, path)
            .await
            .unwrap();
        {
            let spawn = spawn_or_resume(
                &engine,
                CodingAgent::Codex,
                SpawnArgs {
                    worktree_path: path,
                    coding_agent_kind: Default::default(),
                    workspace_path: path,
                    allowed_tools: None,
                    system_prompt: None,
                    session_context: None,
                    resume_session_id: None,
                    model: None,
                    reasoning_effort: None,
                    thread_id,
                    spawning_event_id: None,
                    repo_name: None,
                    interactive: false,
                    build_slot_class: lucidos_build_slot::SlotClass::Ordinary,
                    user_env_vars: &[],
                    binary_override: None,
                    permission_mode: None,
                    account_pin: None,
                    additional_directories: &[],
                    auto_compact: None,
                    keep_alive: false,
                    agent_run: crate::runtime::agent_run_marker::AgentRun::for_thread(
                        path,
                        thread_id,
                        engine.teardown_handover.clone(),
                    ),
                    sandbox,
                },
                CancellationToken::new(),
            );
            tokio::pin!(spawn);
            assert!(futures::poll!(&mut spawn).is_pending());
            assert!(!runtime.0.load(Ordering::Relaxed));
            engine.session_spawn_gate.open();
            let error = tokio::time::timeout(std::time::Duration::from_secs(2), spawn)
                .await
                .expect("opening the gate releases the spawn")
                .err()
                .expect("the fake runtime returns its sentinel error");
            assert_eq!(error.to_string(), "recorded spawn");
            assert!(runtime.0.load(Ordering::Relaxed));
        }
        drop(engine);
        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    async fn make_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let _ = git_cmd(&["init"], &repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
        tokio::fs::write(repo.join("init.txt"), "initial")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "initial commit"], &repo).await;
        (tmp, repo)
    }

    fn fresh_branch(repo: &std::path::Path) -> FreshBranch<'_> {
        fresh_branch_for(repo, uuid::Uuid::new_v4())
    }

    fn fresh_branch_for(repo: &std::path::Path, thread_id: uuid::Uuid) -> FreshBranch<'_> {
        FreshBranch {
            repo_root: repo,
            agent: CodingAgent::ClaudeCode,
            scope: BranchScope::Repo(BranchScope::LUCIDOS_REPO.to_string()),
            thread_name: "fix the auth timeout",
            thread_id,
        }
    }

    fn expected_name(thread_id: uuid::Uuid) -> String {
        format!(
            "lucidos-claude-code-repo-lucidos-fix-the-auth-timeout-{}",
            crate::engine::git_ops::short_thread_id(thread_id)
        )
    }

    #[tokio::test]
    async fn missing_resume_branch_drops_stale_session_id() {
        let (_tmp, repo) = make_test_repo().await;
        let fresh = fresh_branch(&repo);
        let expected = expected_name(fresh.thread_id);
        let resolution = resolve_branch_for_resume(
            &fresh,
            Some("e4a3d60a-ea4d-4592-b252-0558f8798cf3".into()),
            Some("claude-code/already-merged-and-pruned"),
        )
        .await;
        assert!(
            !resolution.reusing_branch,
            "must not reuse a branch that doesn't exist"
        );
        assert_eq!(
            resolution.branch_name, expected,
            "must mint a fresh thread-named branch"
        );
        assert_eq!(
            resolution.resume_session_id, None,
            "stale session_id must be dropped, otherwise CC dies with 'No conversation found'"
        );
        let gone = resolution
            .gone
            .expect("a recorded branch git lacks is reported gone");
        assert_eq!(gone.branch_name, "claude-code/already-merged-and-pruned");
        assert_eq!(
            gone.session_id.as_deref(),
            Some("e4a3d60a-ea4d-4592-b252-0558f8798cf3")
        );
    }

    async fn make_thread_worktree(
        tmp: &tempfile::TempDir,
        repo: &std::path::Path,
        branch: &str,
    ) -> std::path::PathBuf {
        let wt = tmp.path().join("wt");
        let wt_arg = wt.to_string_lossy().to_string();
        let out = git_cmd(&["worktree", "add", "-b", branch, &wt_arg], repo)
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        tokio::fs::write(wt.join("work.txt"), "thread work")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &wt).await;
        let _ = git_cmd(&["commit", "-m", "thread work"], &wt).await;
        wt
    }

    #[tokio::test]
    async fn a_renamed_branch_keeps_its_session() {
        let (tmp, repo) = make_test_repo().await;
        let recorded = "lucidos-claude-code-repo-example-repo-flag-stale-docs-1a2b3c4d";
        let wt = make_thread_worktree(&tmp, &repo, recorded).await;
        let out = git_cmd(
            &["branch", "-m", recorded, "ticket-1234-flag-stale-docs"],
            &wt,
        )
        .await
        .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let resolution = resolve_branch_for_resume(
            &fresh_branch(&repo),
            Some("live-sid".into()),
            Some(recorded),
        )
        .await;
        let gone = resolution
            .gone
            .expect("the recorded name is gone after the rename");
        let recovered = recover_renamed_branch(&repo, &wt, gone, &IdleAnchor::Absent)
            .await
            .expect("git's reflog proves the rename, so the session must survive it");
        assert_eq!(recovered.branch_name, "ticket-1234-flag-stale-docs");
        assert_eq!(recovered.session_id.as_deref(), Some("live-sid"));
    }

    #[tokio::test]
    async fn a_deleted_branch_beside_an_unrelated_one_starts_fresh() {
        let (tmp, repo) = make_test_repo().await;
        let _ = git_cmd(&["branch", "someone-elses-branch"], &repo).await;
        let recorded = "lucidos-claude-code-repo-example-repo-flag-stale-docs-1a2b3c4d";
        let wt = make_thread_worktree(&tmp, &repo, recorded).await;
        let _ = git_cmd(&["checkout", "someone-elses-branch"], &wt).await;
        let out = git_cmd(&["branch", "-D", recorded], &repo).await.unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let fresh = fresh_branch(&repo);
        let resolution =
            resolve_branch_for_resume(&fresh, Some("stale-sid".into()), Some(recorded)).await;
        assert_eq!(resolution.branch_name, expected_name(fresh.thread_id));
        assert_eq!(resolution.resume_session_id, None);
        let gone = resolution.gone.expect("the recorded name is gone");
        assert_eq!(
            recover_renamed_branch(&repo, &wt, gone, &IdleAnchor::Absent).await,
            None,
            "absence is not evidence of a rename"
        );
    }

    #[tokio::test]
    async fn existing_legacy_resume_branch_keeps_its_name_and_session_id() {
        let (_tmp, repo) = make_test_repo().await;
        let _ = git_cmd(&["branch", "claude-code/active-session"], &repo).await;
        let resolution = resolve_branch_for_resume(
            &fresh_branch(&repo),
            Some("good-sid".into()),
            Some("claude-code/active-session"),
        )
        .await;
        assert!(resolution.reusing_branch);
        assert_eq!(resolution.branch_name, "claude-code/active-session");
        assert_eq!(resolution.resume_session_id, Some("good-sid".into()));
    }

    #[test]
    fn unverifiable_resume_branch_is_reused_not_replaced() {
        let decision = decide_branch_resolution(
            GitAnswer::Unknown,
            "lucidos-claude-code-repo-lucidos-live-session",
            Some("good-sid".into()),
        );
        assert_eq!(
            decision,
            BranchDecision::Reuse {
                branch_name: "lucidos-claude-code-repo-lucidos-live-session".into(),
                resume_session_id: Some("good-sid".into()),
            },
            "an unanswered rev-parse must not be read as a deleted branch, and keeping \
             the branch is pointless if the session id is dropped with it"
        );
    }

    #[test]
    fn answered_probes_still_decide_branch_reuse() {
        assert_eq!(
            decide_branch_resolution(
                GitAnswer::Yes,
                "lucidos-codex-repo-lucidos-there",
                Some("sid".into())
            ),
            BranchDecision::Reuse {
                branch_name: "lucidos-codex-repo-lucidos-there".into(),
                resume_session_id: Some("sid".into()),
            }
        );
        assert_eq!(
            decide_branch_resolution(GitAnswer::No, "claude-code/pruned", Some("sid".into())),
            BranchDecision::Gone {
                resume_session_id: Some("sid".into())
            },
            "a pruned branch starts over, and hands the session id on for a rename to win back"
        );
    }

    #[tokio::test]
    async fn no_resume_branch_generates_fresh_with_no_session_id() {
        let (_tmp, repo) = make_test_repo().await;
        let resolution =
            resolve_branch_for_resume(&fresh_branch(&repo), Some("ignored-sid".into()), None).await;
        assert!(!resolution.reusing_branch);
        assert!(resolution.branch_name.starts_with(LUCIDOS_BRANCH_PREFIX));
        assert_eq!(
            resolution.resume_session_id, None,
            "without a resume branch, the session_id has no anchor and must be dropped"
        );
    }

    #[tokio::test]
    async fn two_threads_with_the_same_title_get_different_branches() {
        let (_tmp, repo) = make_test_repo().await;
        let first = resolve_branch_for_resume(&fresh_branch(&repo), None, None).await;
        let _ = git_cmd(&["branch", &first.branch_name], &repo).await;

        let second = resolve_branch_for_resume(&fresh_branch(&repo), None, None).await;
        assert_ne!(first.branch_name, second.branch_name);
        assert!(
            !second.branch_name.ends_with("-2"),
            "numbering must not be what separates two threads: {}",
            second.branch_name
        );
    }

    #[tokio::test]
    async fn one_thread_minting_twice_is_numbered() {
        let (_tmp, repo) = make_test_repo().await;
        let thread_id = uuid::Uuid::new_v4();
        let first =
            resolve_branch_for_resume(&fresh_branch_for(&repo, thread_id), None, None).await;
        assert_eq!(first.branch_name, expected_name(thread_id));
        let _ = git_cmd(&["branch", &first.branch_name], &repo).await;

        let second =
            resolve_branch_for_resume(&fresh_branch_for(&repo, thread_id), None, None).await;
        assert_eq!(
            second.branch_name,
            format!("{}-2", expected_name(thread_id))
        );
    }

    #[tokio::test]
    async fn minted_names_are_valid_git_refs() {
        let (_tmp, repo) = make_test_repo().await;
        let names = [
            "fix the auth timeout",
            "🎉🎉🎉",
            "!!!",
            "日本語のスレッド",
            "Refs/../../etc/passwd",
            "a name ending in .lock",
            &"very long thread name that keeps going ".repeat(12),
        ];
        for name in names {
            let mut fresh = fresh_branch(&repo);
            fresh.thread_name = name;
            let resolution = resolve_branch_for_resume(&fresh, None, None).await;
            let out = git_cmd(
                &["check-ref-format", "--branch", &resolution.branch_name],
                &repo,
            )
            .await
            .unwrap();
            assert!(
                out.status.success(),
                "git rejected {:?} minted from {:?}",
                resolution.branch_name,
                name
            );
        }
    }

    const SPAWNS: usize = 8;

    async fn race_spawns(
        repo: &std::path::Path,
        id_for: impl Fn(usize) -> uuid::Uuid,
    ) -> Vec<Result<String, String>> {
        let wt_root = tempfile::tempdir().unwrap();
        let mut handles = Vec::new();
        for i in 0..SPAWNS {
            let repo = repo.to_path_buf();
            let wt_path = wt_root.path().join(format!("wt-{i}"));
            let thread_id = id_for(i);
            handles.push(tokio::spawn(async move {
                let fresh = FreshBranch {
                    repo_root: &repo,
                    agent: CodingAgent::ClaudeCode,
                    scope: BranchScope::Repo(BranchScope::LUCIDOS_REPO.to_string()),
                    thread_name: "you are one of six parallel partitions",
                    thread_id,
                };
                let allocated = fresh.allocate().await;
                fresh
                    .create_worktree(&wt_path, Some("main"), allocated)
                    .await
            }));
        }
        let mut results = Vec::new();
        for h in handles {
            results.push(h.await.expect("spawn task panicked"));
        }
        results
    }

    async fn assert_distinct_and_real(
        repo: &std::path::Path,
        results: Vec<Result<String, String>>,
    ) {
        let mut names = Vec::new();
        for r in results {
            names.push(r.expect("a concurrent spawn failed to create its branch"));
        }
        let unique: std::collections::HashSet<&String> = names.iter().collect();
        assert_eq!(
            unique.len(),
            SPAWNS,
            "two spawns ended up on one branch: {names:?}"
        );
        for name in &names {
            let out = git_cmd(&["rev-parse", "--verify", name], repo)
                .await
                .unwrap();
            assert!(out.status.success(), "branch {name} was never created");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_spawns_from_one_prompt_get_distinct_branches() {
        let (_tmp, repo) = make_test_repo().await;
        let results = race_spawns(&repo, |_| uuid::Uuid::new_v4()).await;
        assert_distinct_and_real(&repo, results).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_lost_branch_race_retries_until_it_wins() {
        let (_tmp, repo) = make_test_repo().await;
        let thread_id = uuid::Uuid::new_v4();
        let results = race_spawns(&repo, |_| thread_id).await;
        assert_distinct_and_real(&repo, results).await;
    }

    #[tokio::test]
    async fn a_lost_create_leaves_the_winners_branch_untouched() {
        let (tmp, repo) = make_test_repo().await;
        let thread_id = uuid::Uuid::new_v4();
        let winner = expected_name(thread_id);
        let _ = git_cmd(&["branch", &winner], &repo).await;
        let sha_of = |name: String| {
            let repo = repo.clone();
            async move {
                let out = git_cmd(&["rev-parse", "--verify", &name], &repo)
                    .await
                    .unwrap();
                assert!(out.status.success(), "branch {name} is gone");
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
        };
        let before = sha_of(winner.clone()).await;

        let wt_path = tmp.path().join("loser-wt");
        let got = fresh_branch_for(&repo, thread_id)
            .create_worktree(&wt_path, Some("main"), winner.clone())
            .await
            .expect("the loser re-derives and creates its own branch");

        assert_ne!(got, winner);
        assert_eq!(sha_of(winner).await, before);
        sha_of(got).await;
    }
}
