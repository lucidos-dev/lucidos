//! Durable "Planned" marker for coding-agent branches — the enforcement floor
//! for the `implementation-plan` skill, modeled on the Hardened marker
//! (`harden_marker.rs`). A plan is recorded by the skill in the `Proposed`
//! (awaiting-approval) state, which does NOT satisfy the gate; the coding agent
//! presents the plan and, once the human approves in chat, flips it to
//! `Planned` via `lucidos planned approve`. A local fix is acknowledged
//! directly as `AcknowledgedSimple` (no approval needed). `Planned` and
//! `AcknowledgedSimple` satisfy every gate; `Proposed` and the ABSENCE of a row
//! both block. Unlike hardening, the gate is binary satisfying/not — there is
//! no `Stale` re-check, because planning is a pre-condition *decision* about
//! the branch's work that a follow-up commit does not invalidate (the stored
//! `head_sha` is diagnostic only).

use std::path::Path;

/// The valid marker states. `Planned` and `AcknowledgedSimple` satisfy the
/// gate; `Proposed` is recorded but awaits human approval and does NOT satisfy
/// it (see [`PlanMarkerKind::satisfies_gate`]).
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
}

impl PlanMarkerKind {
    /// Wire / DB string. Matches the `state` CHECK constraint in
    /// `planned_branches`.
    pub(crate) fn as_db(&self) -> &'static str {
        match self {
            PlanMarkerKind::Proposed => "proposed",
            PlanMarkerKind::Planned => "planned",
            PlanMarkerKind::AcknowledgedSimple => "acknowledged_simple",
        }
    }

    /// Parse a DB / wire `state` string. Unknown values are treated as
    /// `Planned` defensively — a row whose state isn't the awaiting-approval
    /// sentinel means a settled planning decision, so the gate should pass
    /// rather than nag on a value drift. (`"proposed"` is matched explicitly,
    /// so a drift never silently masquerades as awaiting-approval.)
    pub(crate) fn parse(raw: &str) -> Self {
        match raw.trim() {
            "proposed" => PlanMarkerKind::Proposed,
            "acknowledged_simple" => PlanMarkerKind::AcknowledgedSimple,
            _ => PlanMarkerKind::Planned,
        }
    }

    /// Whether this marker kind satisfies the cc-plan-gate hook and the Apply
    /// floor. `Proposed` does not — implementation stays blocked until the human
    /// approves and the marker flips to `Planned`.
    pub(crate) fn satisfies_gate(&self) -> bool {
        match self {
            PlanMarkerKind::Planned | PlanMarkerKind::AcknowledgedSimple => true,
            PlanMarkerKind::Proposed => false,
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
