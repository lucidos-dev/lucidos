use super::*;
use std::path::Path;

/// `Fresh` means the stored SHA matches the branch's current HEAD; `Stale`
/// means CC has advanced HEAD since `/harden` ran; `Missing` means no DB row
/// exists for this (repo, branch). Apply-time hardening trusts both `Fresh`
/// and `Stale` (CC ran `/harden` at least once for this branch); only
/// `Missing` triggers Apply to run `/harden` itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardenMarkerState {
    Fresh,
    Stale,
    Missing,
}

/// Canonical absolute repo_root used as the DB key. Resolves symlinks so the
/// hook (which uses `git rev-parse --git-common-dir`) and the engine produce
/// the same row.
fn canonical_repo_root(repo_root: &Path) -> String {
    repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf())
        .to_string_lossy()
        .to_string()
}

pub(crate) async fn harden_marker_state(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> HardenMarkerState {
    let stored: Option<String> = match sqlx::query_scalar(
        "SELECT head_sha FROM hardened_branches WHERE repo_root = $1 AND branch_name = $2",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .fetch_optional(pool)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            log!("[Harden] Failed to read hardened_branches: {}", e);
            return HardenMarkerState::Missing;
        }
    };
    let stored = match stored {
        Some(s) => s,
        None => return HardenMarkerState::Missing,
    };
    if let Some(current) = branch_head_sha(repo_root, branch_name).await {
        if stored == current {
            log!("[Harden] Harden marker is fresh (HEAD SHA matches)");
            return HardenMarkerState::Fresh;
        }
    }
    log!(
        "[Harden] Harden marker is STALE (stored={}…)",
        &stored[..stored.floor_char_boundary(12)]
    );
    HardenMarkerState::Stale
}

/// Convenience wrapper: true iff the marker is `Fresh`.
pub(crate) async fn is_harden_marker_fresh(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    harden_marker_state(pool, repo_root, branch_name).await == HardenMarkerState::Fresh
}

/// True iff a marker exists at all (Fresh or Stale). Used by callers that
/// trust marker existence as a "CC ran /harden at least once" signal.
pub(crate) async fn is_harden_marker_present(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    harden_marker_state(pool, repo_root, branch_name).await != HardenMarkerState::Missing
}

/// Record that `(repo_root, branch_name)` has been hardened at `head_sha`.
/// Idempotent — a second `/harden` upserts the new HEAD SHA. Called by the
/// HTTP endpoint that the `lucidos hardened mark` CLI POSTs to from the
/// `lucidos hardened mark` call in `/harden` Phase 5.
pub(crate) async fn record_hardened(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    head_sha: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO hardened_branches (repo_root, branch_name, head_sha, hardened_at) \
         VALUES ($1, $2, $3, NOW()) \
         ON CONFLICT (repo_root, branch_name) DO UPDATE SET head_sha = EXCLUDED.head_sha, hardened_at = NOW()"
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .bind(head_sha)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete the hardening record. Call after successful merge.
pub(crate) async fn consume_harden_marker(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) {
    if let Err(e) =
        sqlx::query("DELETE FROM hardened_branches WHERE repo_root = $1 AND branch_name = $2")
            .bind(canonical_repo_root(repo_root))
            .bind(branch_name)
            .execute(pool)
            .await
    {
        log!(
            "[Harden] Failed to delete hardened_branches row for {}: {}",
            branch_name,
            e
        );
    }
}
