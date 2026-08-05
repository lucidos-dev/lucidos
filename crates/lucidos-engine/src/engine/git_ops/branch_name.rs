//! Coding-agent branch naming: `lucidos-<agent>-<app|repo>-<name>-<slug>[-<n>]`.
//!
//! Every branch an *agent session* creates is named for the thread that owns it,
//! so `git branch -a` reads as a list of work rather than a wall of timestamps.
//! Four segments, each earning its place:
//!
//! - `lucidos` marks the branch as engine-created. It is the one segment every
//!   consumer may match on, and in an *external repo* it is what tells the user
//!   which of their branches Lucidos made.
//! - `<agent>` is [`CodingAgent::as_str`] (`claude-code` / `codex`), so the enum
//!   stays the single source of truth and a third backend needs no naming work.
//! - `<app|repo>-<name>` is the [`BranchScope`]: what the thread works on.
//! - `<slug>` is the thread's name, kebab-cased, with `-2` / `-3` appended when
//!   an earlier thread already took the name.
//!
//! Replaces the old `claude-code/<ts>-<uuid>` and
//! `claude-code/app/<id>/<ts>-<uuid>` shapes (ADR 0004 had recorded the
//! `claude-code/` prefix as a deliberate non-rename; see the ADR added with this
//! change for the reversal). Branches created under the old shapes are never
//! renamed and keep working; `is_coding_agent_branch` recognises both.

use super::{git_cmd, GIT_TIMEOUT};
use crate::core::slug::{slugify_kebab, truncate_slug};
use crate::runtime::CodingAgent;
use std::collections::HashSet;
use std::path::Path;
use uuid::Uuid;

/// Prefix marking a branch as engine-created. The single string every consumer
/// matches on, so widening it is one edit.
pub(crate) const LUCIDOS_BRANCH_PREFIX: &str = "lucidos-";

/// Legacy prefix for coding-agent branches created before the thread-named
/// scheme. Still recognised forever: old branches are never renamed, and one may
/// hold committed work that has not been applied yet.
pub(crate) const LEGACY_BRANCH_PREFIX: &str = "claude-code/";

/// Longest slug we put in a branch name. The fixed part can already run ~35
/// chars (`lucidos-claude-code-repo-lucidos-`), and thread titles are prose, so
/// an uncapped slug produces names no one can read or type. Cut at a dash so the
/// result ends on a whole word.
const MAX_SLUG_CHARS: usize = 48;

/// Highest duplicate number we will hand out before giving up on pretty names
/// and falling back to a unique-by-construction suffix. A repo with 99 live
/// branches for one slug is pathological; the cap just stops the scan spinning.
const MAX_DUPLICATE_NUMBER: u32 = 99;

/// What a coding-agent thread works on, and therefore the `<app|repo>-<name>`
/// segment of its branch. An enum rather than two `Option<String>`s so "both
/// set" and "neither set" are unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchScope {
    /// An *app coding-agent thread*: the app folder id.
    App(String),
    /// A repository thread, Lucidos-source or *external repo*: the registered
    /// repository's name.
    Repo(String),
}

impl BranchScope {
    /// The registered name of the Lucidos source repo
    /// (`LucidosEngine::DEFAULT_REPO_NAME`), slugified. Used when the source
    /// checkout is not in the repo registry at all, so an unregistered checkout
    /// still produces the same branch shape as a registered one.
    pub(crate) const LUCIDOS_REPO: &'static str = "lucidos";

    fn segments(&self) -> (&'static str, &str) {
        match self {
            Self::App(id) => ("app", id.as_str()),
            Self::Repo(name) => ("repo", name.as_str()),
        }
    }
}

/// Is this a branch the engine created for a coding-agent thread?
///
/// Accepts the current `lucidos-` shape and the legacy `claude-code/` one. Used
/// by the orphan-recovery worktree scan to tell its own branches from a user's;
/// recovery applies the workspace-marker check on top, so this is a filter, not
/// an authorization.
pub(crate) fn is_coding_agent_branch(branch: &str) -> bool {
    branch.starts_with(LUCIDOS_BRANCH_PREFIX) || branch.starts_with(LEGACY_BRANCH_PREFIX)
}

/// Kebab-case a thread name into the slug segment, falling back to
/// `thread-<8 hex>` when nothing survives slugification (an emoji-only or CJK
/// title, or a thread with neither a title nor a first message yet).
pub(crate) fn branch_slug(thread_name: &str, thread_id: Uuid) -> String {
    let s = truncate_slug(&slugify_kebab(thread_name), MAX_SLUG_CHARS);
    if s.is_empty() {
        // `chars().take` rather than a byte slice, matching
        // `slugify_trigger_name_with_fallback`: same rule, and the uuid's
        // ASCII-ness should not be what keeps an index from panicking.
        let short: String = thread_id.as_simple().to_string().chars().take(8).collect();
        format!("thread-{short}")
    } else {
        s
    }
}

/// Build the un-numbered branch name. Pure, so the shape is testable without a
/// repo. The scope name is slugified here rather than trusted: an app id is
/// already kebab-case, but a registered repository's `name` is free text the
/// user typed.
pub(crate) fn coding_agent_branch_base(
    agent: CodingAgent,
    scope: &BranchScope,
    slug: &str,
) -> String {
    let (kind, name) = scope.segments();
    let name = truncate_slug(&slugify_kebab(name), MAX_SLUG_CHARS);
    // A scope whose name slugifies away entirely still gets its kind segment,
    // so the shape stays parseable and the slug can never abut the agent.
    if name.is_empty() {
        format!("{LUCIDOS_BRANCH_PREFIX}{}-{kind}-{slug}", agent.as_str())
    } else {
        format!(
            "{LUCIDOS_BRANCH_PREFIX}{}-{kind}-{name}-{slug}",
            agent.as_str()
        )
    }
}

/// Pick the first free name in the `base`, `base-2`, `base-3`, … series.
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

/// List the local branches that could collide with `base`: `base` itself and
/// anything under `base-*`. `None` when git could not be asked (see
/// [`pick_free_branch_name`] for why that is not an empty set).
///
/// The `refs/heads/` scope is deliberate: only a local branch can make
/// `git worktree add -b` fail, and a remote-tracking ref of the same name is not
/// a collision.
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
        log!(
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

/// Allocate a branch name for a fresh coding-agent session:
/// `lucidos-<agent>-<app|repo>-<name>-<slug>`, numbered if the slug is taken.
///
/// Reads refs only; the branch itself is still created by the caller's
/// `git worktree add -b` (or `create_sparse_app_worktree`), which keeps the
/// `branch_created` bookkeeping the failure-path cleanup depends on honest.
///
/// That leaves a millisecond-wide race: two spawns allocating the same name at
/// the same instant both try `-b`, and the loser's `worktree add` fails with
/// "a branch named 'x' already exists". That failure is loud, non-destructive
/// and retryable (the spawn surfaces "resend the message to retry"), which is
/// the direction to fail in. Closing it would mean creating the ref here and
/// handing `worktree add` a pre-existing branch, which blurs "did this attempt
/// create it" exactly where `cleanup_failed_spawn` needs the answer.
pub(crate) async fn allocate_coding_agent_branch(
    repo_root: &Path,
    agent: CodingAgent,
    scope: &BranchScope,
    thread_name: &str,
    thread_id: Uuid,
) -> String {
    let base = coding_agent_branch_base(agent, scope, &branch_slug(thread_name, thread_id));
    let existing = existing_branches_for(repo_root, &base).await;
    if existing.is_none() {
        log!(
            "[Git] Could not list branches matching {} in {} (git gave no answer within {}s). Using a unique suffix rather than assuming the name is free",
            base,
            repo_root.display(),
            GIT_TIMEOUT.as_secs()
        );
    }
    let unique = &Uuid::new_v4().as_simple().to_string()[..6];
    pick_free_branch_name(&base, existing.as_ref(), unique)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn base_carries_agent_and_scope() {
        assert_eq!(
            coding_agent_branch_base(
                CodingAgent::ClaudeCode,
                &BranchScope::Repo("Lucidos".into()),
                "fix-auth-timeout"
            ),
            "lucidos-claude-code-repo-lucidos-fix-auth-timeout"
        );
        assert_eq!(
            coding_agent_branch_base(
                CodingAgent::ClaudeCode,
                &BranchScope::App("habit-tracker".into()),
                "add-streaks"
            ),
            "lucidos-claude-code-app-habit-tracker-add-streaks"
        );
        assert_eq!(
            coding_agent_branch_base(
                CodingAgent::Codex,
                &BranchScope::Repo("example-repo".into()),
                "fix-auth"
            ),
            "lucidos-codex-repo-example-repo-fix-auth"
        );
    }

    /// `LUCIDOS_REPO` is the fallback used when the source checkout is not in
    /// the repo registry, and the point of it is that a registered and an
    /// unregistered checkout produce the SAME branch. Nothing in the type system
    /// ties it to `DEFAULT_REPO_NAME`, so renaming that constant would silently
    /// split the two apart. Pin it here instead.
    #[test]
    fn the_lucidos_repo_fallback_matches_the_registered_repo_name() {
        assert_eq!(
            slugify_kebab(crate::engine::LucidosEngine::DEFAULT_REPO_NAME),
            BranchScope::LUCIDOS_REPO,
            "an unregistered Lucidos checkout must slug to the same scope name \
             as the registered one"
        );
    }

    /// A repository the user named in a non-Latin script still produces a
    /// well-formed name: the kind segment survives even when the name does not.
    #[test]
    fn base_keeps_its_shape_when_the_scope_name_slugifies_away() {
        assert_eq!(
            coding_agent_branch_base(
                CodingAgent::ClaudeCode,
                &BranchScope::Repo("日本語".into()),
                "fix-thing"
            ),
            "lucidos-claude-code-repo-fix-thing"
        );
    }

    #[test]
    fn slug_falls_back_to_the_thread_id_when_nothing_survives() {
        let id = Uuid::parse_str("401a2d19-1111-2222-3333-444444444444").unwrap();
        assert_eq!(branch_slug("🎉🎉🎉", id), "thread-401a2d19");
        assert_eq!(branch_slug("", id), "thread-401a2d19");
        assert_eq!(branch_slug("!!!", id), "thread-401a2d19");
    }

    #[test]
    fn slug_is_capped_at_a_word_boundary() {
        let id = Uuid::new_v4();
        let long = "we should name our coding agent branches after the thread that owns them";
        let slug = branch_slug(long, id);
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

    /// A name freed by a deleted branch is reused: numbering is allocated
    /// against what exists now, not against a high-water mark.
    #[test]
    fn a_freed_number_is_reused() {
        let base = "lucidos-codex-repo-lucidos-fix";
        let existing = taken(&[base, &format!("{base}-3")]);
        assert_eq!(
            pick_free_branch_name(base, Some(&existing), "abc123"),
            format!("{base}-2")
        );
    }

    /// The load-bearing one: an unanswered listing must not be read as "no
    /// branches exist", or two concurrent spawns get the same name.
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
    fn recognises_both_the_current_and_legacy_prefixes() {
        assert!(is_coding_agent_branch(
            "lucidos-claude-code-repo-lucidos-fix"
        ));
        assert!(is_coding_agent_branch("lucidos-codex-app-habit-tracker-x"));
        assert!(is_coding_agent_branch("claude-code/20260804-141257-e96461"));
        assert!(is_coding_agent_branch("claude-code/app/habit-tracker/x"));
        // A user's own branches, and the engine's non-coding-agent ones.
        assert!(!is_coding_agent_branch("main"));
        assert!(!is_coding_agent_branch("feature/lucidos-integration"));
        assert!(!is_coding_agent_branch("merge-tmp/abc"));
        assert!(!is_coding_agent_branch("e2e-test/abc"));
    }
}
