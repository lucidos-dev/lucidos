//! Durable "Planned" marker for coding-agent branches — the enforcement floor
//! for the `implementation-plan` skill, modeled on the Hardened marker
//! (`harden_marker.rs`). A plan is recorded by the skill in the `Proposed`
//! (awaiting-approval) state, which does NOT satisfy the gate; the coding agent
//! presents the plan and, once the human approves in chat, flips it to
//! `Planned` via `lucidos planned approve`. A local fix is acknowledged
//! directly as `AcknowledgedSimple` (no approval needed). An unattended run
//! commits a scoped security fix as `BoundedSecurityFix`, which names the files
//! it is bounded to. Those three satisfy every gate; `Proposed` and the ABSENCE
//! of a row both block.
//!
//! Unlike hardening, the gate is binary satisfying/not, with no `Stale`
//! re-check. Planning is a pre-condition *decision* about the branch's work
//! that a follow-up commit does not invalidate, so the stored `head_sha` is
//! diagnostic only.

use std::path::Path;

/// How many files a `BoundedSecurityFix` may name. The lane's whole claim is
/// that the fix is small, so the cap is what turns "bounded" from an adjective
/// into a checkable fact. Ten covers the usual shape (a module, its inline
/// test, a knowhow file, a glossary entry, a migration) without admitting a
/// refactor.
pub(crate) const MAX_BOUNDED_SECURITY_FIX_FILES: usize = 10;

/// The valid marker states. `Planned`, `AcknowledgedSimple` and
/// `BoundedSecurityFix` satisfy the gate; `Proposed` is recorded but awaits
/// human approval and does NOT satisfy it (see
/// [`PlanMarkerKind::satisfies_gate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanMarkerKind {
    /// A `docs/plans/` file was written and recorded by the skill, but the
    /// human has not approved it yet. Does NOT satisfy the gate.
    Proposed,
    /// The human approved the proposed plan (the agent ran `planned approve`),
    /// or a legacy row recorded before the approval step existed. Satisfies.
    Planned,
    /// The agent declared this a local fix needing no plan. Satisfies.
    AcknowledgedSimple,
    /// An unattended run (the nightly security pass) is committing a security
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
    /// Wire / DB string. Matches the `state` CHECK constraint in
    /// `planned_branches`.
    pub(crate) fn as_db(&self) -> &'static str {
        match self {
            PlanMarkerKind::Proposed => "proposed",
            PlanMarkerKind::Planned => "planned",
            PlanMarkerKind::AcknowledgedSimple => "acknowledged_simple",
            PlanMarkerKind::BoundedSecurityFix => "bounded_security_fix",
        }
    }

    /// Parse a DB / wire `state` string. Unknown values are treated as
    /// `Planned` defensively — a row whose state isn't the awaiting-approval
    /// sentinel means a settled planning decision, so the gate should pass
    /// rather than nag on a value drift. (`"proposed"` is matched explicitly,
    /// so a drift never silently masquerades as awaiting-approval.)
    pub(crate) fn parse(raw: &str) -> Self {
        Self::parse_strict(raw).unwrap_or(PlanMarkerKind::Planned)
    }

    /// Parse for a WRITE, where an unrecognized value is a caller mistake
    /// rather than drift to tolerate.
    ///
    /// [`parse`](Self::parse)'s lenient default is right when reading a stored
    /// row and wrong when recording one. A typo would be written as `planned`,
    /// which claims a human approved the plan. `bounded-security-fix` is the
    /// live example. `CLAUDE.md` makes kebab-case the convention for public API
    /// values, so it is the spelling an agent reaches for. It would record a
    /// gate-satisfying marker with no bound to enforce.
    pub(crate) fn parse_strict(raw: &str) -> Option<Self> {
        match raw.trim() {
            "proposed" => Some(PlanMarkerKind::Proposed),
            "planned" => Some(PlanMarkerKind::Planned),
            "acknowledged_simple" => Some(PlanMarkerKind::AcknowledgedSimple),
            "bounded_security_fix" => Some(PlanMarkerKind::BoundedSecurityFix),
            _ => None,
        }
    }

    /// Whether this marker kind satisfies the cc-plan-gate hook and the Apply
    /// floor. `Proposed` does not — implementation stays blocked until the human
    /// approves and the marker flips to `Planned`.
    pub(crate) fn satisfies_gate(&self) -> bool {
        match self {
            PlanMarkerKind::Planned
            | PlanMarkerKind::AcknowledgedSimple
            | PlanMarkerKind::BoundedSecurityFix => true,
            PlanMarkerKind::Proposed => false,
        }
    }

    /// Whether this kind carries a file bound the Apply floor must enforce.
    pub(crate) fn is_file_bounded(&self) -> bool {
        matches!(self, PlanMarkerKind::BoundedSecurityFix)
    }
}

/// `Present` carries which kind of marker exists (for diagnostics / wire);
/// `Missing` means no row for this `(repo, branch)` — the only state that
/// blocks the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanMarkerState {
    Present(PlanMarkerKind),
    Missing,
}

impl PlanMarkerState {
    pub(crate) fn is_present(&self) -> bool {
        matches!(self, PlanMarkerState::Present(_))
    }

    /// Whether this state satisfies the gate (a present marker whose kind
    /// satisfies it). `Proposed` and `Missing` both block.
    pub(crate) fn satisfies_gate(&self) -> bool {
        matches!(self, PlanMarkerState::Present(k) if k.satisfies_gate())
    }
}

/// Canonical absolute repo_root used as the DB key. Resolves symlinks so the
/// hook (which uses `git rev-parse --git-common-dir`) and the engine produce
/// the same row — identical to `harden_marker::canonical_repo_root`.
fn canonical_repo_root(repo_root: &Path) -> String {
    repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf())
        .to_string_lossy()
        .to_string()
}

pub(crate) async fn plan_marker_state(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> PlanMarkerState {
    let stored: Option<String> = match sqlx::query_scalar(
        "SELECT state FROM planned_branches WHERE repo_root = $1 AND branch_name = $2",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .fetch_optional(pool)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            log!("[Plan] Failed to read planned_branches: {}", e);
            return PlanMarkerState::Missing;
        }
    };
    match stored {
        Some(s) => PlanMarkerState::Present(PlanMarkerKind::parse(&s)),
        None => PlanMarkerState::Missing,
    }
}

/// Why a `record_planned` call was refused before it reached the database.
/// Surfaced to the agent as the HTTP 400 body, so each variant's text is what
/// the agent reads and acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanMarkerRejection {
    /// A bounded security fix named no files, so there is no bound to enforce.
    BoundedFixNeedsFiles,
    /// A bounded security fix named more files than the lane allows.
    BoundedFixTooManyFiles(usize),
    /// A file list arrived for a kind that has no bound to record.
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

/// Refuse a mark whose file list and kind disagree, before it reaches the
/// database.
///
/// The schema carries the same pairing as a CHECK constraint, and both are
/// wanted: the constraint is the floor no write path can dodge, this is the arm
/// that tells the agent what to do instead. A raw constraint violation reads as
/// an internal error and teaches nothing.
/// Returns the CLEANED list to store, so no caller can record the raw one.
pub(crate) fn validate_plan_files(
    kind: PlanMarkerKind,
    files: &[String],
) -> Result<Vec<String>, PlanMarkerRejection> {
    // Trim first. `--files a.rs, b.rs` splits on the comma and keeps the
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

/// Record that `(repo_root, branch_name)` has a plan decision. Idempotent — a
/// second mark upserts the new state/path/reason/files/HEAD. Called by the HTTP
/// endpoint that the `lucidos planned mark` CLI POSTs to.
///
/// `files` is the bound a `BoundedSecurityFix` is confined to and must be empty
/// for every other kind; [`validate_plan_files`] is the arm that says so.
#[allow(clippy::too_many_arguments)] // one arg per planned_branches column, plus the pool
pub(crate) async fn record_planned(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    kind: PlanMarkerKind,
    plan_path: Option<&str>,
    reason: Option<&str>,
    files: &[String],
    head_sha: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let files = validate_plan_files(kind, files)?;
    // NULL rather than an empty array for the unbounded kinds, so the schema's
    // pairing constraint can tell "no bound" from "an empty bound".
    let files: Option<Vec<String>> = kind.is_file_bounded().then_some(files);
    sqlx::query(
        "INSERT INTO planned_branches (repo_root, branch_name, state, plan_path, reason, files, head_sha, planned_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, NOW()) \
         ON CONFLICT (repo_root, branch_name) DO UPDATE SET \
            state = EXCLUDED.state, plan_path = EXCLUDED.plan_path, \
            reason = EXCLUDED.reason, files = EXCLUDED.files, \
            head_sha = EXCLUDED.head_sha, planned_at = NOW()",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .bind(kind.as_db())
    .bind(plan_path)
    .bind(reason)
    .bind(files)
    .bind(head_sha)
    .execute(pool)
    .await?;
    Ok(())
}

/// Which of `changed` fall outside the `bound` a `BoundedSecurityFix` named.
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
        .filter(|f| !f.starts_with("docs/plans/"))
        .filter(|f| !bound.iter().any(|b| b == *f))
        .cloned()
        .collect()
}

/// The three reads the bound check needs, each already resolved, so the
/// decision itself is pure and testable without an engine.
pub(crate) struct BoundedFixInputs {
    /// The files the marker named.
    pub bound: Result<Vec<String>, String>,
    /// What the branch has committed against its base.
    pub committed: Result<Vec<String>, String>,
    /// What is dirty in the branch's worktree. `Ok(vec![])` when there is no
    /// worktree, since then there is nothing for Apply to auto-commit.
    pub dirty: Result<Vec<String>, String>,
}

/// Why a `BoundedSecurityFix` branch may not apply, or `None` when everything
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

/// The files a `BoundedSecurityFix` marker is confined to. `Ok` of an empty
/// list for every other state and for a branch with no marker.
///
/// `Err` when the read itself failed, never an empty list. The Apply floor
/// refuses either way, but the two need different words. An unreadable bound is
/// the engine's problem. Told instead that it broke a bound it kept, an
/// unattended agent would widen the bound for no reason.
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
        log!("[Plan] Failed to read planned_branches.files: {}", e);
        format!("reading the recorded bound failed: {}", e)
    })?;
    Ok(stored.flatten().unwrap_or_default())
}

/// Approve a `Proposed` plan: flip its state to `Planned` so the gate passes.
/// Targeted update — only a `proposed` row is flipped, so it never fabricates a
/// marker on an unplanned branch and never clobbers an `acknowledged_simple`
/// ack. `plan_path` and `reason` are preserved. Returns whether a row was
/// flipped (`false` = no proposed row: missing, already planned, or simple).
/// Called by the `/api/v1/internal/approve-plan` endpoint that the
/// `lucidos planned approve` CLI POSTs to.
pub(crate) async fn approve_plan(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE planned_branches SET state = 'planned', planned_at = NOW() \
         WHERE repo_root = $1 AND branch_name = $2 AND state = 'proposed'",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete the plan record. Call after successful merge, alongside
/// `consume_harden_marker`.
pub(crate) async fn consume_plan_marker(pool: &sqlx::PgPool, repo_root: &Path, branch_name: &str) {
    if let Err(e) =
        sqlx::query("DELETE FROM planned_branches WHERE repo_root = $1 AND branch_name = $2")
            .bind(canonical_repo_root(repo_root))
            .bind(branch_name)
            .execute(pool)
            .await
    {
        log!(
            "[Plan] Failed to delete planned_branches row for {}: {}",
            branch_name,
            e
        );
    }
}
