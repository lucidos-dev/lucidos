use super::plan_approval::{
    user_approved_plan, user_approved_plan_for, ApprovalTarget, PlanApprovalError,
};
use super::{canonical_repo_root, local_branch_exists};
use std::path::Path;

pub(crate) const MAX_BOUNDED_SECURITY_FIX_FILES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanMarkerKind {
    Proposed,
    Planned,
    PlanOnly,
    AcknowledgedSimple,
    /// INVARIANT: An unattended run (the nightly security pass) is committing a security
    /// fix confined to the files recorded alongside it. Satisfies, and the
    /// Apply floor then refuses any branch that went outside that list.
    ///
    /// Deliberately its own value rather than a reuse of the two satisfying
    /// states beside it. `AcknowledgedSimple` claims the work is local, which a
    /// cross-module credential fix is not. `Planned` claims a human approved,
    /// which nobody did. Both lies were available to the sessions that
    /// deadlocked here, and both were refused. The lane exists so the honest
    /// option is also the working one.
    BoundedSecurityFix,
}

impl PlanMarkerKind {
    pub fn as_db(&self) -> &'static str {
        match self {
            PlanMarkerKind::Proposed => "proposed",
            PlanMarkerKind::Planned => "planned",
            PlanMarkerKind::PlanOnly => "plan_only",
            PlanMarkerKind::AcknowledgedSimple => "acknowledged_simple",
            PlanMarkerKind::BoundedSecurityFix => "bounded_security_fix",
        }
    }

    pub(crate) fn parse(raw: &str) -> Self {
        Self::parse_strict(raw).unwrap_or(PlanMarkerKind::Planned)
    }

    /// INVARIANT: Parse for a WRITE, where an unrecognized value is a caller mistake
    /// rather than drift to tolerate.
    ///
    /// [`parse`](Self::parse)'s lenient default is right when reading a stored
    /// row and wrong when recording one. A typo would be written as `planned`,
    /// which claims a human approved the plan. `bounded-security-fix` is the
    /// live example. `CLAUDE.md` makes kebab-case the convention for public API
    /// values, so it is the spelling an agent reaches for. It would record a
    /// gate-satisfying marker with no bound to enforce.
    pub fn parse_strict(raw: &str) -> Option<Self> {
        match raw.trim() {
            "proposed" => Some(PlanMarkerKind::Proposed),
            "planned" => Some(PlanMarkerKind::Planned),
            "plan_only" => Some(PlanMarkerKind::PlanOnly),
            "acknowledged_simple" => Some(PlanMarkerKind::AcknowledgedSimple),
            "bounded_security_fix" => Some(PlanMarkerKind::BoundedSecurityFix),
            _ => None,
        }
    }

    pub fn satisfies_gate(&self) -> bool {
        match self {
            PlanMarkerKind::Planned
            | PlanMarkerKind::AcknowledgedSimple
            | PlanMarkerKind::BoundedSecurityFix => true,
            PlanMarkerKind::Proposed | PlanMarkerKind::PlanOnly => false,
        }
    }

    pub fn is_file_bounded(&self) -> bool {
        matches!(self, PlanMarkerKind::BoundedSecurityFix)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanMarkerState {
    Present(PlanMarkerKind),
    Changed,
    Missing,
    Unreadable,
}

pub(crate) async fn plan_marker_state(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> PlanMarkerState {
    plan_marker_snapshot(pool, repo_root, branch_name)
        .await
        .state
}

#[derive(Debug, PartialEq, Eq)]
pub struct PlanMarkerSnapshot {
    pub state: PlanMarkerState,
    pub file_bound: Option<Vec<String>>,
}

#[derive(sqlx::FromRow)]
struct StoredPlanMarker {
    state: String,
    plan_path: Option<String>,
    plan_version: Option<String>,
    carried_without_plan: bool,
    files: Option<Vec<String>>,
}

pub(crate) async fn plan_marker_snapshot(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> PlanMarkerSnapshot {
    let stored: Result<Option<StoredPlanMarker>, _> = sqlx::query_as(
        "SELECT state, plan_path, plan_version, carried_without_plan, files FROM planned_branches WHERE repo_root = $1 AND branch_name = $2",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .fetch_optional(pool)
    .await;
    let row = match stored {
        Ok(Some(row)) => row,
        Ok(None) => {
            return PlanMarkerSnapshot {
                state: PlanMarkerState::Missing,
                file_bound: None,
            }
        }
        Err(e) => {
            log_legacy!("[Plan] Failed to read planned_branches: {}", e);
            return PlanMarkerSnapshot {
                state: PlanMarkerState::Unreadable,
                file_bound: None,
            };
        }
    };
    let kind = PlanMarkerKind::parse(&row.state);
    let state = match (row.plan_path, row.plan_version) {
        (Some(path), Some(version)) => {
            use super::super::plan_lifecycle::committed_plan_version;
            match committed_plan_version(repo_root, branch_name, &path).await {
                Ok(Some(current)) if current == version => PlanMarkerState::Present(kind),
                Ok(None) if row.carried_without_plan => PlanMarkerState::Present(kind),
                Ok(_) => changed_plan_state(kind),
                Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                    changed_plan_state(kind)
                }
                Err(error) => {
                    log_legacy!("[Plan] Failed to verify the committed plan: {}", error);
                    PlanMarkerState::Unreadable
                }
            }
        }
        _ => PlanMarkerState::Present(kind),
    };
    let file_bound = match state {
        PlanMarkerState::Present(PlanMarkerKind::BoundedSecurityFix) => row.files,
        _ => None,
    };
    PlanMarkerSnapshot { state, file_bound }
}

fn changed_plan_state(kind: PlanMarkerKind) -> PlanMarkerState {
    match kind {
        PlanMarkerKind::Planned | PlanMarkerKind::PlanOnly => PlanMarkerState::Changed,
        _ => PlanMarkerState::Missing,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanMarkerRejection {
    BoundedFixNeedsFiles,
    BoundedFixTooManyFiles(usize),
    FilesOnlyForBoundedFix,
}

impl std::fmt::Display for PlanMarkerRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanMarkerRejection::BoundedFixNeedsFiles => write!(
                f,
                "A bounded security fix must name the files it is confined to: \
                 pass --files with the repo-relative paths you will touch. \
                 Without a bound there is nothing for Apply to check."
            ),
            PlanMarkerRejection::BoundedFixTooManyFiles(n) => write!(
                f,
                "A bounded security fix may name at most {MAX_BOUNDED_SECURITY_FIX_FILES} files, \
                 and this one named {n}. Work that wide is not bounded: write a plan with \
                 `lucidos planned mark --plan <path>` and report it as blocked on a decision."
            ),
            PlanMarkerRejection::FilesOnlyForBoundedFix => write!(
                f,
                "--files belongs to --security-fix only. The other plan states are not \
                 file-bounded, so a list recorded against one would never be enforced."
            ),
        }
    }
}

impl std::error::Error for PlanMarkerRejection {}

pub fn validate_plan_files(
    kind: PlanMarkerKind,
    files: &[String],
) -> Result<Vec<String>, PlanMarkerRejection> {
    // INVARIANT: Trim first. `--files a.rs, b.rs` splits on the comma and keeps the
    // space. The bound is later compared to git's output with `==`, so an
    // untrimmed entry matches nothing. It would then refuse the apply forever,
    // and an unattended run has nobody to re-mark it.
    let files: Vec<String> = files
        .iter()
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .collect();
    if !kind.is_file_bounded() {
        return if files.is_empty() {
            Ok(files)
        } else {
            Err(PlanMarkerRejection::FilesOnlyForBoundedFix)
        };
    }
    if files.is_empty() {
        return Err(PlanMarkerRejection::BoundedFixNeedsFiles);
    }
    if files.len() > MAX_BOUNDED_SECURITY_FIX_FILES {
        return Err(PlanMarkerRejection::BoundedFixTooManyFiles(files.len()));
    }
    Ok(files)
}

#[allow(clippy::too_many_arguments)]
pub async fn record_planned(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    kind: PlanMarkerKind,
    plan_path: Option<&str>,
    reason: Option<&str>,
    files: &[String],
    head_sha: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    record_planned_with_kind(
        pool,
        repo_root,
        branch_name,
        kind,
        plan_path,
        reason,
        files,
        head_sha,
        crate::engine::thread_events::PlanKind::Implementation,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn record_planned_with_kind(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    kind: PlanMarkerKind,
    plan_path: Option<&str>,
    reason: Option<&str>,
    files: &[String],
    head_sha: &str,
    plan_kind: crate::engine::thread_events::PlanKind,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    PlanMarkerWrite::new(
        repo_root,
        branch_name,
        kind,
        plan_path,
        reason,
        files,
        head_sha,
        plan_kind,
    )?
    .write(pool)
    .await?;
    Ok(())
}

pub struct PlanMarkerWrite {
    root: String,
    branch: String,
    kind: PlanMarkerKind,
    path: Option<String>,
    reason: Option<String>,
    files: Option<Vec<String>>,
    head: String,
    plan_kind: crate::engine::thread_events::PlanKind,
}

impl PlanMarkerWrite {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root: &Path,
        branch: &str,
        kind: PlanMarkerKind,
        path: Option<&str>,
        reason: Option<&str>,
        files: &[String],
        head: &str,
        plan_kind: crate::engine::thread_events::PlanKind,
    ) -> Result<Self, PlanMarkerRejection> {
        let files = validate_plan_files(kind, files)?;
        Ok(Self {
            root: canonical_repo_root(root),
            branch: branch.into(),
            kind,
            path: path.map(str::to_owned),
            reason: reason.map(str::to_owned),
            files: kind.is_file_bounded().then_some(files),
            head: head.into(),
            plan_kind,
        })
    }

    pub fn repo_root(&self) -> &str {
        &self.root
    }

    pub async fn write<'e>(self, executor: impl sqlx::PgExecutor<'e>) -> Result<(), sqlx::Error> {
        sqlx::query(
        "INSERT INTO planned_branches (repo_root, branch_name, state, plan_path, reason, files, head_sha, kind, planned_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW()) \
         ON CONFLICT (repo_root, branch_name) DO UPDATE SET \
            state = EXCLUDED.state, kind = EXCLUDED.kind, plan_path = EXCLUDED.plan_path, \
            reason = EXCLUDED.reason, files = EXCLUDED.files, \
            head_sha = EXCLUDED.head_sha, planned_at = NOW(), \
            plan_version = NULL, carried_without_plan = FALSE",
    )
    .bind(self.root)
    .bind(self.branch)
    .bind(self.kind.as_db())
    .bind(self.path)
    .bind(self.reason)
    .bind(self.files)
    .bind(self.head)
    .bind(self.plan_kind.as_str())
    .execute(executor)
    .await?;
        Ok(())
    }
}

pub async fn tracked_branch_elsewhere(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    caller_thread: &str,
    branch_name: &str,
) -> Result<Option<String>, sqlx::Error> {
    let tracked = tracked_branch_named_otherwise(pool, caller_thread, branch_name).await?;
    Ok(match tracked {
        // INVARIANT: Unknown never refuses: a stray marker costs a re-mark, a false
        // refusal strands the agent.
        Some(tracked)
            if local_branch_exists(repo_root, &tracked)
                .await
                .or_unknown(false) =>
        {
            Some(tracked)
        }
        _ => None,
    })
}

async fn tracked_branch_named_otherwise(
    pool: &sqlx::PgPool,
    caller_thread: &str,
    branch_name: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT tracked FROM ( \
            SELECT payload->>'branch' AS tracked, \
                   COALESCE(payload->>'coding_agent_kind', '') = 'external' AS external \
              FROM events \
             WHERE aggregate_id = $1 AND event_type = 'SessionStarted' \
             ORDER BY sequence DESC LIMIT 1 \
         ) newest \
         WHERE NOT external AND tracked <> $2 \
           AND NOT EXISTS (SELECT 1 FROM thread_summaries \
                            WHERE thread_id::text = $1 AND coding_agent_is_external_repo)",
    )
    .bind(caller_thread)
    .bind(branch_name)
    .fetch_optional(pool)
    .await
}

pub async fn carry_planned(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    from_branch: &str,
    to_branch: &str,
) -> Result<(), sqlx::Error> {
    copy_plan_marker(pool, repo_root, from_branch, to_branch, None, false).await
}

async fn copy_plan_marker(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    from_branch: &str,
    to_branch: &str,
    checked_at: Option<chrono::DateTime<chrono::Utc>>,
    child_without_plan: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO planned_branches (repo_root, branch_name, state, kind, plan_path, reason, files, head_sha, planned_at, plan_version, carried_without_plan) \
         SELECT repo_root, $3, state, kind, plan_path, reason, files, head_sha, NOW(), plan_version, \
                CASE WHEN $4::timestamptz IS NULL THEN carried_without_plan ELSE $5 END \
         FROM planned_branches WHERE repo_root = $1 AND branch_name = $2 \
           AND ($4::timestamptz IS NULL OR planned_at = $4) \
         ON CONFLICT (repo_root, branch_name) DO NOTHING",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(from_branch)
    .bind(to_branch)
    .bind(checked_at)
    .bind(child_without_plan)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn carry_child_planned(
    pool: &sqlx::PgPool,
    repo: &Path,
    child: uuid::Uuid,
    branch: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parent: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(child)
            .fetch_optional(pool)
            .await?
            .flatten();
    let Some(parent) = parent else {
        return Ok(());
    };
    let Some((Some(folder), Some(parent_branch))) =
        super::plan_approval::latest_plan_session(pool, parent).await?
    else {
        return Ok(());
    };
    let root = canonical_repo_root(repo);
    if folder.is_empty() || canonical_repo_root(Path::new(&folder)) != root {
        return Ok(());
    }
    let marker: Option<(String, String, chrono::DateTime<chrono::Utc>, bool)> = sqlx::query_as(
        "SELECT plan_path, plan_version, planned_at, carried_without_plan FROM planned_branches \
         WHERE repo_root = $1 AND branch_name = $2 AND state = 'planned' \
           AND kind = 'implementation' AND plan_path IS NOT NULL AND plan_version IS NOT NULL",
    )
    .bind(&root)
    .bind(&parent_branch)
    .fetch_optional(pool)
    .await?;
    let Some((path, version, marked_at, parent_without_plan)) = marker else {
        return Ok(());
    };
    use super::super::plan_lifecycle::committed_plan_version;
    let parent_version = committed_plan_version(repo, &parent_branch, &path).await?;
    let child_version = committed_plan_version(repo, branch, &path).await?;
    if (!parent_without_plan && parent_version.is_none())
        || parent_version.as_ref().is_some_and(|v| v != &version)
        || child_version.as_ref().is_some_and(|v| v != &version)
    {
        return Ok(());
    }
    copy_plan_marker(
        pool,
        repo,
        &parent_branch,
        branch,
        Some(marked_at),
        child_version.is_none(),
    )
    .await?;
    Ok(())
}

pub(crate) use crate::core::git::needs_hardening;
pub use crate::core::git::{is_plan_file, lands_plan_files_only};

/// INVARIANT: Which of `changed` fall outside the `bound` a `BoundedSecurityFix` named.
/// An empty result means the branch kept its promise.
///
/// `docs/plans/` is always in bounds, matching the `cc-plan-gate` hook's own
/// exemption. A session whose fix turns out wider still writes a plan, and that
/// file is how it reports the decision it wants. Refusing it for the plan alone
/// would re-close the escape hatch.
///
/// An EMPTY bound therefore refuses everything, which is the wanted direction.
/// A file-bounded marker whose list could not be read has no promise to check,
/// and refusing an apply costs a click.
pub(crate) fn bounded_fix_violations(bound: &[String], changed: &[String]) -> Vec<String> {
    changed
        .iter()
        .filter(|f| !is_plan_file(f))
        .filter(|f| !bound.iter().any(|b| b == *f))
        .cloned()
        .collect()
}

pub(crate) struct BoundedFixInputs {
    pub bound: Result<Vec<String>, String>,
    pub committed: Result<Vec<String>, String>,
    pub dirty: Result<Vec<String>, String>,
}

/// INVARIANT: Why a `BoundedSecurityFix` branch may not apply, or `None` when everything
/// it puts on main is inside the files it named.
///
/// `committed` and `dirty` are added together because both land: every apply
/// path `git add -A`s the worktree before merging. Reading the committed diff
/// alone let an out-of-bound edit through, and let a branch with no commits at
/// all pass on an empty list.
///
/// Fails CLOSED, in three ways that read differently on purpose. An unreadable
/// bound is the engine's fault, not the agent's. Told it broke a bound it kept,
/// an unattended agent would widen the bound for nothing.
pub(crate) fn bounded_fix_refusal_for(inputs: BoundedFixInputs) -> Option<String> {
    let bound = match inputs.bound {
        Ok(bound) => bound,
        Err(e) => {
            return Some(format!(
                "This branch carries a bounded security-fix marker and {e}, so the bound could \
                 not be checked. This is an engine-side failure, not a problem with your change: \
                 retry the apply."
            ));
        }
    };
    let mut landing = match inputs.committed {
        Ok(committed) => committed,
        Err(e) => {
            return Some(format!(
                "This branch carries a bounded security-fix marker, and git could not say which \
                 files it changed ({e}), so the bound could not be checked. Retry the apply; if \
                 it keeps failing, have the plan approved instead."
            ));
        }
    };
    match inputs.dirty {
        Ok(dirty) => landing.extend(dirty),
        Err(e) => {
            return Some(format!(
                "This branch carries a bounded security-fix marker, and git could not say what is \
                 uncommitted in its worktree ({e}). Apply commits that tree before merging, so \
                 the bound could not be checked. Retry the apply."
            ));
        }
    }
    let outside = bounded_fix_violations(&bound, &landing);
    if outside.is_empty() {
        return None;
    }
    Some(format!(
        "This branch was committed under the bounded security-fix lane, which skips the plan \
         decision only while the change stays inside the files it named. It named [{}] and also \
         puts [{}] on main. Either drop the extra files, or re-run \
         `lucidos planned mark --security-fix` with the full list, or write a plan and have the \
         user approve it.",
        bound.join(", "),
        outside.join(", "),
    ))
}

pub(crate) async fn plan_marker_files(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> Result<Vec<String>, String> {
    let stored: Option<Option<Vec<String>>> = sqlx::query_scalar(
        "SELECT files FROM planned_branches WHERE repo_root = $1 AND branch_name = $2",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        log_legacy!("[Plan] Failed to read planned_branches.files: {}", e);
        format!("reading the recorded bound failed: {}", e)
    })?;
    Ok(stored.flatten().unwrap_or_default())
}

/// INVARIANT: Approve a proposed plan for its recorded kind. A design always stays plan-only.
/// Requires the user's own approval on the owning thread. Missing or settled
/// markers return `None`; only a proposed marker transitions.
pub(crate) async fn approve_plan(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    caller_thread: Option<&str>,
) -> Result<Option<ApprovedPlan>, PlanApprovalError> {
    approve_plan_for(pool, repo_root, branch_name, caller_thread, false).await
}

pub(crate) async fn approve_plan_for(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    caller_thread: Option<&str>,
    plan_only: bool,
) -> Result<Option<ApprovedPlan>, PlanApprovalError> {
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT kind FROM planned_branches WHERE repo_root = $1 AND branch_name = $2 AND state = 'proposed'",
    ).bind(canonical_repo_root(repo_root)).bind(branch_name).fetch_optional(pool).await
        .map_err(PlanApprovalError::Unreadable)?;
    let Some(kind) = kind else {
        return Ok(None);
    };
    let target = match (kind.as_str(), plan_only) {
        (_, true) => ApprovalTarget::PlanForLater,
        ("design", false) => ApprovalTarget::Design,
        (_, false) => ApprovalTarget::Implementation,
    };
    if target == ApprovalTarget::Implementation {
        user_approved_plan(pool, repo_root, branch_name, caller_thread).await?;
    } else {
        user_approved_plan_for(pool, repo_root, branch_name, caller_thread, target).await?;
    }
    let plan_only = target != ApprovalTarget::Implementation;
    let marker = if plan_only {
        PlanMarkerKind::PlanOnly
    } else {
        PlanMarkerKind::Planned
    };
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "UPDATE planned_branches SET state = $3, planned_at = NOW() \
         WHERE repo_root = $1 AND branch_name = $2 AND state = 'proposed' AND kind = $4 \
         RETURNING plan_path",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .bind(marker.as_db())
    .bind(kind)
    .fetch_optional(pool)
    .await
    .map_err(PlanApprovalError::Unreadable)?;
    Ok(row.map(|(plan_path,)| ApprovedPlan {
        plan_path,
        plan_only,
    }))
}

#[derive(Debug, PartialEq, Eq)]
pub struct ApprovedPlan {
    pub plan_path: Option<String>,
    pub plan_only: bool,
}

/// INVARIANT: Delete the plan record once its change applied, alongside
/// `consume_harden_marker`. A `proposed` marker stays: a plan recorded while
/// the Apply ran belongs to the next round, and `approve_plan` still has to
/// find it.
pub(crate) async fn consume_plan_marker(pool: &sqlx::PgPool, repo_root: &Path, branch_name: &str) {
    if let Err(e) = sqlx::query(
        "DELETE FROM planned_branches WHERE repo_root = $1 AND branch_name = $2 \
         AND state <> $3",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .bind(PlanMarkerKind::Proposed.as_db())
    .execute(pool)
    .await
    {
        log_legacy!(
            "[Plan] Failed to delete planned_branches row for {}: {}",
            branch_name,
            e
        );
    }
}
