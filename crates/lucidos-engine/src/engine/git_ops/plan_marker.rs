//! Durable "Planned" marker for coding-agent branches — the enforcement floor
//! for the `implementation-plan` skill, modeled on the Hardened marker
//! (`harden_marker.rs`). A branch is "planned" once the agent either ran the
//! `implementation-plan` skill (which records a `docs/plans/` file) or
//! explicitly acknowledged a local fix. Both states pass every gate; only the
//! ABSENCE of a row blocks. Unlike hardening, the gate is binary
//! present/absent — there is no `Stale` re-check, because planning is a
//! pre-condition *decision* about the branch's work that a follow-up commit
//! does not invalidate (the stored `head_sha` is diagnostic only).

use std::path::Path;

/// The two valid marker states, both of which satisfy the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanMarkerKind {
    /// A `docs/plans/` file was written and recorded (the skill ran).
    Planned,
    /// The agent declared this a local fix needing no plan.
    AcknowledgedSimple,
}

impl PlanMarkerKind {
    /// Wire / DB string. Matches the `state` CHECK constraint in
    /// `planned_branches`.
    pub(crate) fn as_db(&self) -> &'static str {
        match self {
            PlanMarkerKind::Planned => "planned",
            PlanMarkerKind::AcknowledgedSimple => "acknowledged_simple",
        }
    }

    /// Parse a DB / wire `state` string. Unknown values are treated as
    /// `Planned` defensively — any stored row means the agent made a planning
    /// decision, so the gate should pass rather than nag on a value drift.
    pub(crate) fn parse(raw: &str) -> Self {
        match raw.trim() {
            "acknowledged_simple" => PlanMarkerKind::AcknowledgedSimple,
            _ => PlanMarkerKind::Planned,
        }
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

/// True iff a marker exists at all (either kind). Used by the gate and the
/// Apply floor — both treat any planning decision as "satisfied".
pub(crate) async fn is_plan_marker_present(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    plan_marker_state(pool, repo_root, branch_name)
        .await
        .is_present()
}

/// Record that `(repo_root, branch_name)` has a plan decision. Idempotent — a
/// second mark upserts the new state/path/reason/HEAD. Called by the HTTP
/// endpoint that the `lucidos planned mark` CLI POSTs to.
pub(crate) async fn record_planned(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    kind: PlanMarkerKind,
    plan_path: Option<&str>,
    reason: Option<&str>,
    head_sha: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO planned_branches (repo_root, branch_name, state, plan_path, reason, head_sha, planned_at) \
         VALUES ($1, $2, $3, $4, $5, $6, NOW()) \
         ON CONFLICT (repo_root, branch_name) DO UPDATE SET \
            state = EXCLUDED.state, plan_path = EXCLUDED.plan_path, \
            reason = EXCLUDED.reason, head_sha = EXCLUDED.head_sha, planned_at = NOW()",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .bind(kind.as_db())
    .bind(plan_path)
    .bind(reason)
    .bind(head_sha)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete the plan record. Call after successful merge, alongside
/// `consume_harden_marker`.
pub(crate) async fn consume_plan_marker(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) {
    if let Err(e) = sqlx::query(
        "DELETE FROM planned_branches WHERE repo_root = $1 AND branch_name = $2",
    )
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
