use super::{git_cmd, short_thread_id, GIT_TIMEOUT};
use crate::core::slug::{slugify_kebab, truncate_slug};
use crate::runtime::CodingAgent;
use std::collections::HashSet;
use std::path::Path;
use uuid::Uuid;

pub const LUCIDOS_BRANCH_PREFIX: &str = "lucidos-";

/// INVARIANT: Legacy prefix for coding-agent branches created before the thread-named
/// scheme. Still recognised forever: old branches are never renamed, and one may
/// hold committed work that has not been applied yet.
pub(crate) const LEGACY_BRANCH_PREFIX: &str = "claude-code/";

const MAX_SLUG_CHARS: usize = 48;

const MAX_DUPLICATE_NUMBER: u32 = 99;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchScope {
    App(String),
    Repo(String),
}

impl BranchScope {
    /// INVARIANT: The registered name of the Lucidos source repo
    /// (`LucidosEngine::DEFAULT_REPO_NAME`), slugified. Used when the source
    /// checkout is not in the repo registry at all, so an unregistered checkout
    /// still produces the same branch shape as a registered one.
    pub const LUCIDOS_REPO: &'static str = "lucidos";

    fn segments(&self) -> (&'static str, &str) {
        match self {
            Self::App(id) => ("app", id.as_str()),
            Self::Repo(name) => ("repo", name.as_str()),
        }
    }
}

/// INVARIANT: Is this a branch the engine created for a coding-agent thread?
///
/// Accepts the current `lucidos-` shape and the legacy `claude-code/` one. Used
/// by the orphan-recovery worktree scan to tell its own branches from a user's;
/// recovery applies the workspace-marker check on top, so this is a filter, not
/// an authorization.
pub fn is_coding_agent_branch(branch: &str) -> bool {
    branch.starts_with(LUCIDOS_BRANCH_PREFIX) || branch.starts_with(LEGACY_BRANCH_PREFIX)
}

pub use crate::core::git::{is_lane_branch, lane_branch};
pub(crate) use crate::core::git::{lane_branch_batch, LANE_BRANCH_PREFIX};

pub(crate) fn branch_slug(thread_name: &str) -> String {
    let s = truncate_slug(&slugify_kebab(thread_name), MAX_SLUG_CHARS);
    if s.is_empty() {
        "thread".to_string()
    } else {
        s
    }
}

pub(crate) fn coding_agent_branch_base(
    agent: CodingAgent,
    scope: &BranchScope,
    slug: &str,
    thread_id: Uuid,
) -> String {
    let (kind, name) = scope.segments();
    let name = truncate_slug(&slugify_kebab(name), MAX_SLUG_CHARS);
    let id = short_thread_id(thread_id);
    if name.is_empty() {
        format!(
            "{LUCIDOS_BRANCH_PREFIX}{}-{kind}-{slug}-{id}",
            agent.as_str()
        )
    } else {
        format!(
            "{LUCIDOS_BRANCH_PREFIX}{}-{kind}-{name}-{slug}-{id}",
            agent.as_str()
        )
    }
}

/// INVARIANT: Pick the first free name in the `base`, `base-2`, `base-3`, … series.
///
/// `existing` is `None` when git could not be asked which branches exist (a
/// timeout or spawn failure, routine on a saturated host). That is NOT the same
/// as "no branches exist": guessing `base` is free would hand two concurrent
/// spawns the same name. So an unanswered probe falls back to
/// `base-<unique_suffix>`, which is ugly but cannot collide. Same rule as
/// [`super::GitAnswer`], applied to a listing rather than a yes/no.
pub(crate) fn pick_free_branch_name(
    base: &str,
    existing: Option<&HashSet<String>>,
    unique_suffix: &str,
) -> String {
    let Some(taken) = existing else {
        return format!("{base}-{unique_suffix}");
    };
    if !taken.contains(base) {
        return base.to_string();
    }
    for n in 2..=MAX_DUPLICATE_NUMBER {
        let candidate = format!("{base}-{n}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    format!("{base}-{unique_suffix}")
}

/// INVARIANT: Did `git worktree add` fail because the branch name is already taken?
///
/// Three git messages, because git checks the name twice and can lose at
/// either point:
///
/// - `a branch named 'x' already exists`, its own pre-flight check.
/// - `'x' is already used by worktree at ...`, the branch exists and is
///   checked out somewhere.
/// - `cannot lock ref 'refs/heads/x'`, the ref transaction losing the race.
///   Git's pre-flight check is itself a check-then-act, the very shape this
///   module exists to survive. Under real contention it passes, and the
///   transaction below it is what fails. The tail varies (`reference already
///   exists`, or a `.lock` file another process holds) and both mean the same
///   thing: somebody else is creating this exact ref right now.
///
/// Deliberately narrow otherwise. A retry only helps when a *different name*
/// would succeed. So the other "already exists" git can emit here, the one
/// about the worktree *path*, must not match: re-deriving the name would burn
/// every attempt on a failure the name has nothing to do with.
pub fn branch_name_is_taken(stderr: &str) -> bool {
    (stderr.contains("a branch named") && stderr.contains("already exists"))
        || stderr.contains("is already used by worktree at")
        || stderr.contains("cannot lock ref 'refs/heads/")
}

async fn existing_branches_for(repo_root: &Path, base: &str) -> Option<HashSet<String>> {
    let exact = format!("refs/heads/{base}");
    let numbered = format!("refs/heads/{base}-*");
    let out = git_cmd(
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            &exact,
            &numbered,
        ],
        repo_root,
    )
    .await
    .ok()?;
    if !out.status.success() {
        log_legacy!(
            "[Git] for-each-ref for {} returned non-zero in {}: {}",
            base,
            repo_root.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

/// INVARIANT: Allocate a branch name for a fresh coding-agent session:
/// `lucidos-<agent>-<app|repo>-<name>-<slug>-<id>`, numbered if it is taken.
///
/// Reads refs only; the branch itself is still created by the caller's
/// `git worktree add -b` (or `create_sparse_app_worktree`), which keeps the
/// `branch_created` bookkeeping the failure-path cleanup depends on honest.
///
/// **This is a proposal, not a reservation.** Nothing serializes the read here
/// with the create later, so the answer can be stale by the time it is used.
/// Two things stop that mattering. The name carries the thread's short id, so a
/// sibling spawn is asking for a different name in the first place. And the
/// caller retries: `FreshBranch::create_worktree` calls back in here when a
/// create loses, and the fresh listing then sees the winner's branch. The
/// create is the source of truth, this is only a good first guess.
pub async fn allocate_coding_agent_branch(
    repo_root: &Path,
    agent: CodingAgent,
    scope: &BranchScope,
    thread_name: &str,
    thread_id: Uuid,
) -> String {
    let base = coding_agent_branch_base(agent, scope, &branch_slug(thread_name), thread_id);
    let existing = existing_branches_for(repo_root, &base).await;
    if existing.is_none() {
        log_legacy!(
            "[Git] Could not list branches matching {} in {} (git gave no answer within {}s). Using a unique suffix rather than assuming the name is free",
            base,
            repo_root.display(),
            GIT_TIMEOUT.as_secs()
        );
    }
    let unique: String = Uuid::new_v4()
        .as_simple()
        .to_string()
        .chars()
        .take(6)
        .collect();
    pick_free_branch_name(&base, existing.as_ref(), &unique)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn id() -> Uuid {
        Uuid::parse_str("401a2d19-1111-2222-3333-444444444444").unwrap()
    }

    #[test]
    fn base_carries_agent_and_scope() {
        assert_eq!(
            coding_agent_branch_base(
                CodingAgent::ClaudeCode,
                &BranchScope::Repo("Lucidos".into()),
                "fix-auth-timeout",
                id()
            ),
            "lucidos-claude-code-repo-lucidos-fix-auth-timeout-401a2d19"
        );
        assert_eq!(
            coding_agent_branch_base(
                CodingAgent::ClaudeCode,
                &BranchScope::App("habit-tracker".into()),
                "add-streaks",
                id()
            ),
            "lucidos-claude-code-app-habit-tracker-add-streaks-401a2d19"
        );
        assert_eq!(
            coding_agent_branch_base(
                CodingAgent::Codex,
                &BranchScope::Repo("example-repo".into()),
                "fix-auth",
                id()
            ),
            "lucidos-codex-repo-example-repo-fix-auth-401a2d19"
        );
    }

    #[test]
    fn identical_prompts_from_different_threads_derive_different_names() {
        let scope = BranchScope::Repo("lucidos".into());
        let slug = branch_slug("you are one of six parallel partitions");
        let a = coding_agent_branch_base(CodingAgent::ClaudeCode, &scope, &slug, Uuid::new_v4());
        let b = coding_agent_branch_base(CodingAgent::ClaudeCode, &scope, &slug, Uuid::new_v4());
        assert_ne!(a, b, "the thread id must separate two identical prompts");
    }

    #[test]
    fn the_branch_id_matches_the_worktree_directory_id() {
        let thread_id = Uuid::new_v4();
        let workspace = std::path::Path::new("/tmp/does-not-need-to-exist");
        let branch = coding_agent_branch_base(
            CodingAgent::ClaudeCode,
            &BranchScope::Repo("lucidos".into()),
            "fix-auth",
            thread_id,
        );
        let dir_name = format!("thread-{}", short_thread_id(thread_id));
        assert!(
            branch.ends_with(&format!("-{}", short_thread_id(thread_id))),
            "branch {branch} must end in the short thread id"
        );
        assert!(
            crate::engine::session_queries::deterministic_worktree_path(workspace, thread_id)
                .ends_with(&dir_name),
            "worktree dir must carry the same short id as the branch"
        );
    }

    #[test]
    fn the_lucidos_repo_fallback_matches_the_registered_repo_name() {
        assert_eq!(
            slugify_kebab(crate::engine::LucidosEngine::DEFAULT_REPO_NAME),
            BranchScope::LUCIDOS_REPO,
            "an unregistered Lucidos checkout must slug to the same scope name \
             as the registered one"
        );
    }

    #[test]
    fn base_keeps_its_shape_when_the_scope_name_slugifies_away() {
        assert_eq!(
            coding_agent_branch_base(
                CodingAgent::ClaudeCode,
                &BranchScope::Repo("日本語".into()),
                "fix-thing",
                id()
            ),
            "lucidos-claude-code-repo-fix-thing-401a2d19"
        );
    }

    #[test]
    fn a_title_that_slugifies_away_leaves_just_the_thread_id() {
        assert_eq!(branch_slug("🎉🎉🎉"), "thread");
        assert_eq!(branch_slug(""), "thread");
        assert_eq!(branch_slug("!!!"), "thread");
        assert_eq!(
            coding_agent_branch_base(
                CodingAgent::ClaudeCode,
                &BranchScope::Repo("lucidos".into()),
                &branch_slug("🎉🎉🎉"),
                id()
            ),
            "lucidos-claude-code-repo-lucidos-thread-401a2d19"
        );
    }

    #[test]
    fn slug_is_capped_at_a_word_boundary() {
        let long = "we should name our coding agent branches after the thread that owns them";
        let slug = branch_slug(long);
        assert!(slug.len() <= MAX_SLUG_CHARS, "slug too long: {slug}");
        assert!(!slug.ends_with('-'), "slug must not end on a dash: {slug}");
        assert_eq!(slug, "we-should-name-our-coding-agent-branches-after");
    }

    #[test]
    fn first_of_a_slug_is_unnumbered_then_2_then_3() {
        let base = "lucidos-claude-code-repo-lucidos-fix-auth";
        assert_eq!(
            pick_free_branch_name(base, Some(&taken(&[])), "abc123"),
            base
        );
        assert_eq!(
            pick_free_branch_name(base, Some(&taken(&[base])), "abc123"),
            format!("{base}-2")
        );
        assert_eq!(
            pick_free_branch_name(base, Some(&taken(&[base, &format!("{base}-2")])), "abc123"),
            format!("{base}-3")
        );
    }

    #[test]
    fn a_freed_number_is_reused() {
        let base = "lucidos-codex-repo-lucidos-fix";
        let existing = taken(&[base, &format!("{base}-3")]);
        assert_eq!(
            pick_free_branch_name(base, Some(&existing), "abc123"),
            format!("{base}-2")
        );
    }

    #[test]
    fn an_unanswered_listing_falls_back_to_a_unique_suffix() {
        let base = "lucidos-claude-code-repo-lucidos-fix-auth";
        assert_eq!(
            pick_free_branch_name(base, None, "abc123"),
            format!("{base}-abc123")
        );
    }

    #[test]
    fn a_pathological_pile_of_duplicates_falls_back_rather_than_spinning() {
        let base = "lucidos-claude-code-repo-lucidos-x";
        let mut existing = taken(&[base]);
        for n in 2..=MAX_DUPLICATE_NUMBER {
            existing.insert(format!("{base}-{n}"));
        }
        assert_eq!(
            pick_free_branch_name(base, Some(&existing), "abc123"),
            format!("{base}-abc123")
        );
    }

    #[test]
    fn a_taken_branch_name_is_told_apart_from_every_other_failure() {
        assert!(branch_name_is_taken(
            "Preparing worktree (new branch 'lucidos-claude-code-repo-lucidos-fix')\n\
             fatal: a branch named 'lucidos-claude-code-repo-lucidos-fix' already exists"
        ));
        assert!(branch_name_is_taken(
            "fatal: 'lucidos-codex-repo-lucidos-fix' is already used by worktree at \
             '/ws/.lucidos/worktrees/thread-401a2d19'"
        ));
        assert!(branch_name_is_taken(
            "Preparing worktree (new branch 'lucidos-claude-code-repo-lucidos-fix')\n\
             fatal: cannot lock ref 'refs/heads/lucidos-claude-code-repo-lucidos-fix': \
             reference already exists"
        ));
        assert!(branch_name_is_taken(
            "fatal: cannot lock ref 'refs/heads/lucidos-claude-code-repo-lucidos-fix': \
             Unable to create '/repo/.git/refs/heads/lucidos-claude-code-repo-lucidos-fix.lock': \
             File exists."
        ));
        assert!(!branch_name_is_taken(
            "fatal: '/ws/.lucidos/worktrees/thread-401a2d19' already exists"
        ));
        assert!(!branch_name_is_taken(
            "fatal: invalid reference: origin/main"
        ));
        assert!(!branch_name_is_taken(""));
    }

    #[test]
    fn recognises_both_the_current_and_legacy_prefixes() {
        assert!(is_coding_agent_branch(
            "lucidos-claude-code-repo-lucidos-fix"
        ));
        assert!(is_coding_agent_branch("lucidos-codex-app-habit-tracker-x"));
        assert!(is_coding_agent_branch("claude-code/20260804-141257-e96461"));
        assert!(is_coding_agent_branch("claude-code/app/habit-tracker/x"));
        assert!(!is_coding_agent_branch("main"));
        assert!(!is_coding_agent_branch("feature/lucidos-integration"));
        assert!(!is_coding_agent_branch("merge-tmp/abc"));
        assert!(!is_coding_agent_branch("e2e-test/abc"));
    }

    #[test]
    fn a_lane_branch_names_its_batch() {
        let batch = Uuid::new_v4();
        let branch = lane_branch(batch);
        assert!(is_lane_branch(&branch));
        assert_eq!(lane_branch_batch(&branch), Some(batch));
        assert!(!is_coding_agent_branch(&branch));
        assert!(!is_lane_branch("lane/not-a-batch"));
        assert!(!is_lane_branch("main"));
    }
}
