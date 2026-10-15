use super::*;
use std::path::Path;

/// `Fresh` means the stored SHA matches the branch's current HEAD; `Stale`
/// means CC has advanced HEAD since `/harden` ran; `Missing` means no marker
/// that answers the question asked. Apply-time hardening trusts both `Fresh`
/// and `Stale` (CC ran `/harden` at least once for this branch); only
/// `Missing` triggers Apply to run `/harden` itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardenMarkerState {
    Fresh,
    Stale,
    Missing,
}

/// Which part of `/harden` a marker certifies. An *integrated sub-thread* runs
/// the *fast tier*: review, lint and the frontend checks, with no engine
/// suite. Every other branch runs the full one. A full marker covers a fast
/// need, never the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardenTier {
    Fast,
    Full,
}

impl HardenTier {
    pub(crate) const ALL: [Self; 2] = [Self::Fast, Self::Full];

    /// The `hardened_branches.tier` text, and the CLI's spelling.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Full => "full",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == value)
    }

    /// Whether a marker of this tier certifies work that needs `need`.
    pub(crate) fn covers(self, need: HardenTier) -> bool {
        self == Self::Full || need == Self::Fast
    }
}

/// What `hardened_branches` holds for a branch. A recorded marker always
/// carries the HEAD SHA the last `/harden` covered, and the tier it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HardenMarker {
    Missing,
    Recorded {
        head_sha: String,
        fresh: bool,
        tier: HardenTier,
    },
}

impl HardenMarker {
    /// The marker's state for work that needs `need`. A marker of a tier that
    /// does not cover the need reads as `Missing`.
    pub(crate) fn state_for(&self, need: HardenTier) -> HardenMarkerState {
        match self {
            HardenMarker::Recorded { tier, .. } if !tier.covers(need) => HardenMarkerState::Missing,
            HardenMarker::Missing => HardenMarkerState::Missing,
            HardenMarker::Recorded { fresh: true, .. } => HardenMarkerState::Fresh,
            HardenMarker::Recorded { fresh: false, .. } => HardenMarkerState::Stale,
        }
    }

    /// The recorded tier, or `None` without a marker.
    pub(crate) fn tier(&self) -> Option<HardenTier> {
        match self {
            HardenMarker::Recorded { tier, .. } => Some(*tier),
            HardenMarker::Missing => None,
        }
    }
}

/// The marker's state as Apply and a proposal of independent work see it:
/// only a full marker counts.
pub(crate) async fn full_harden_marker_state(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> HardenMarkerState {
    harden_marker(pool, repo_root, branch_name)
        .await
        .state_for(HardenTier::Full)
}

pub(crate) async fn harden_marker(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> HardenMarker {
    let stored: Option<(String, String)> = match sqlx::query_as(
        "SELECT head_sha, tier FROM hardened_branches WHERE repo_root = $1 AND branch_name = $2",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .fetch_optional(pool)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            log!("[Harden] Failed to read hardened_branches: {}", e);
            return HardenMarker::Missing;
        }
    };
    let Some((head_sha, tier)) = stored else {
        return HardenMarker::Missing;
    };
    // The CHECK admits only the two tiers. Anything else reads as the weaker
    // one, which never stands in for a full run.
    let tier = HardenTier::parse(&tier).unwrap_or(HardenTier::Fast);
    let fresh = branch_head_sha(repo_root, branch_name).await.as_deref() == Some(head_sha.as_str());
    if fresh {
        log!("[Harden] Harden marker is fresh (HEAD SHA matches)");
    } else {
        log!(
            "[Harden] Harden marker is STALE (stored={}…)",
            &head_sha[..head_sha.floor_char_boundary(12)]
        );
    }
    HardenMarker::Recorded {
        head_sha,
        fresh,
        tier,
    }
}

/// True iff a full marker exists (Fresh or Stale). Used by callers that trust
/// it as a "CC ran the whole `/harden` at least once" signal. A fast marker
/// alone never counts: its branch never ran the engine suite.
pub(crate) async fn has_full_harden_marker(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    full_harden_marker_state(pool, repo_root, branch_name).await != HardenMarkerState::Missing
}

/// Record that `(repo_root, branch_name)` has been hardened at `head_sha`, by
/// a run of `tier`. Idempotent: a second `/harden` upserts the new HEAD SHA
/// and tier. Called by the HTTP endpoint that `lucidos hardened mark` POSTs to
/// from `/harden` Phase 5.
pub(crate) async fn record_hardened(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
    head_sha: &str,
    tier: HardenTier,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO hardened_branches (repo_root, branch_name, head_sha, tier, hardened_at) \
         VALUES ($1, $2, $3, $4, NOW()) \
         ON CONFLICT (repo_root, branch_name) DO UPDATE \
         SET head_sha = EXCLUDED.head_sha, tier = EXCLUDED.tier, hardened_at = NOW()",
    )
    .bind(canonical_repo_root(repo_root))
    .bind(branch_name)
    .bind(head_sha)
    .bind(tier.as_str())
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
