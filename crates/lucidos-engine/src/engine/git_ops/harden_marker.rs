use super::*;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardenMarkerState {
    Fresh,
    Stale,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardenTier {
    Fast,
    Full,
}

impl HardenTier {
    pub(crate) const ALL: [Self; 2] = [Self::Fast, Self::Full];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Full => "full",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == value)
    }

    pub fn covers(self, need: HardenTier) -> bool {
        self == Self::Full || need == Self::Fast
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardenMarker {
    Missing,
    Recorded {
        head_sha: String,
        fresh: bool,
        tier: HardenTier,
    },
}

impl HardenMarker {
    pub fn state_for(&self, need: HardenTier) -> HardenMarkerState {
        match self {
            HardenMarker::Recorded { tier, .. } if !tier.covers(need) => HardenMarkerState::Missing,
            HardenMarker::Missing => HardenMarkerState::Missing,
            HardenMarker::Recorded { fresh: true, .. } => HardenMarkerState::Fresh,
            HardenMarker::Recorded { fresh: false, .. } => HardenMarkerState::Stale,
        }
    }

    pub fn tier(&self) -> Option<HardenTier> {
        match self {
            HardenMarker::Recorded { tier, .. } => Some(*tier),
            HardenMarker::Missing => None,
        }
    }
}

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
            log_legacy!("[Harden] Failed to read hardened_branches: {}", e);
            return HardenMarker::Missing;
        }
    };
    let Some((head_sha, tier)) = stored else {
        return HardenMarker::Missing;
    };
    // INVARIANT: The CHECK admits only the two tiers. Anything else reads as the weaker
    // one, which never stands in for a full run.
    let tier = HardenTier::parse(&tier).unwrap_or(HardenTier::Fast);
    let fresh = branch_head_sha(repo_root, branch_name).await.as_deref() == Some(head_sha.as_str());
    if fresh {
        log_legacy!("[Harden] Harden marker is fresh (HEAD SHA matches)");
    } else {
        log_legacy!(
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

pub async fn has_full_harden_marker(
    pool: &sqlx::PgPool,
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    full_harden_marker_state(pool, repo_root, branch_name).await != HardenMarkerState::Missing
}

pub async fn record_hardened(
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

pub(crate) async fn carry_harden_marker(
    bus: &crate::engine::event_bus::EventBus,
    repo_root: &Path,
    branch: &str,
    batch_id: uuid::Uuid,
    from: &str,
    to: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::engine::event_bus::SystemEvent;

    let repo = canonical_repo_root(repo_root);
    let branch = branch.to_owned();
    let from = from.to_owned();
    let to = to.to_owned();
    bus.emit_system_with_mutation(
        SystemEvent::HardenCertificateCarried {
            batch_id,
            from_sha: from.clone(),
            to_sha: to.clone(),
        },
        move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE hardened_branches SET head_sha = $1
                 WHERE repo_root = $2 AND branch_name = $3 AND head_sha = $4 AND tier = 'full'
                 RETURNING head_sha",
                )
                .bind(to)
                .bind(repo)
                .bind(branch)
                .bind(from)
                .fetch_one(conn)
                .await?;
                Ok(())
            })
        },
    )
    .await
}

pub async fn consume_harden_marker(pool: &sqlx::PgPool, repo_root: &Path, branch_name: &str) {
    if let Err(e) =
        sqlx::query("DELETE FROM hardened_branches WHERE repo_root = $1 AND branch_name = $2")
            .bind(canonical_repo_root(repo_root))
            .bind(branch_name)
            .execute(pool)
            .await
    {
        log_legacy!(
            "[Harden] Failed to delete hardened_branches row for {}: {}",
            branch_name,
            e
        );
    }
}
