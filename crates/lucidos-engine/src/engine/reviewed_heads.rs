//! INVARIANT: *Reviewed heads*: commits another `/harden` run certified, which
//! `/harden` Phase 0.4 hands to `scripts/harden-scope.sh` as already reviewed.
//! A head missing from the list only widens the review, so every unread or
//! unmatched source drops out rather than standing in (ADR 0315).

use crate::engine::git_ops::{harden_marker, lane_branch_batch, HardenMarker, HardenTier};
use crate::engine::landing_lane::LaneMember;
use std::path::Path;
use uuid::Uuid;

/// The heads the branch's integrated children hardened, plus, on a lane
/// branch, each member snapshot its member's full marker certifies.
pub async fn reviewed_heads(
    pool: &sqlx::PgPool,
    branch_name: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let mut heads = crate::engine::integrated::integrated_child_heads(pool, branch_name).await?;
    if let Some(batch_id) = lane_branch_batch(branch_name) {
        heads.extend(certified_member_heads(pool, batch_id).await?);
    }
    heads.sort();
    heads.dedup();
    Ok(heads)
}

/// INVARIANT: A member counts only when its branch's marker is full tier at
/// exactly the snapshot the lane merged. A remote member has no local marker.
async fn certified_member_heads(
    pool: &sqlx::PgPool,
    batch_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    let members: Option<sqlx::types::Json<Vec<LaneMember>>> = sqlx::query_scalar(
        "SELECT payload->'data'->'members' FROM events \
         WHERE event_type = 'LaneBatchStarted' AND aggregate_id = $1 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(batch_id.to_string())
    .fetch_optional(pool)
    .await?;
    let changes = crate::core::changes_projection::ChangesProjection::new(pool.clone());
    let mut heads = Vec::new();
    for member in members.map(|m| m.0).unwrap_or_default() {
        if member.remote.is_some() {
            continue;
        }
        let Some(change) = changes.get_by_id(member.change_id).await? else {
            continue;
        };
        let marker = harden_marker(pool, Path::new(&change.repo_root), &change.branch_name).await;
        if let HardenMarker::Recorded {
            head_sha,
            tier: HardenTier::Full,
            ..
        } = marker
        {
            if head_sha == member.snapshot_sha {
                heads.push(head_sha);
            }
        }
    }
    Ok(heads)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
    use crate::engine::git_ops::record_hardened;
    use crate::engine::thread_events::landing_lane::RemoteLaneSource;
    use crate::test_support::{seed_pending_change, setup_test_db, teardown_test_db};

    #[tokio::test]
    async fn a_lane_lists_only_members_fully_hardened_at_their_snapshot() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let mut members = Vec::new();
        for (branch, snapshot, marker) in [
            (
                "claude-code/certified",
                "aaa1",
                Some(("aaa1", HardenTier::Full)),
            ),
            ("claude-code/fast", "bbb2", Some(("bbb2", HardenTier::Fast))),
            (
                "claude-code/moved",
                "ccc3",
                Some(("ccc9", HardenTier::Full)),
            ),
            ("claude-code/unmarked", "ddd4", None),
        ] {
            let change_id = seed_pending_change(&bus, Uuid::new_v4(), branch).await;
            if let Some((sha, tier)) = marker {
                record_hardened(&pool, Path::new("/tmp"), branch, sha, tier)
                    .await
                    .expect("record a marker");
            }
            members.push(LaneMember {
                change_id,
                snapshot_sha: snapshot.into(),
                remote: None,
            });
        }
        let remote_id = seed_pending_change(&bus, Uuid::new_v4(), "claude-code/remote").await;
        record_hardened(
            &pool,
            Path::new("/tmp"),
            "claude-code/remote",
            "eee5",
            HardenTier::Full,
        )
        .await
        .expect("record a marker");
        members.push(LaneMember {
            change_id: remote_id,
            snapshot_sha: "eee5".into(),
            remote: Some(RemoteLaneSource {
                oid: "eee5".into(),
                title: "Remote".into(),
                summary: String::new(),
                branch: "claude-code/remote".into(),
                gateway: None,
            }),
        });
        let batch_id = Uuid::new_v4();
        bus.emit(BusEvent::System(SystemEvent::LaneBatchStarted {
            batch_id,
            members,
            base_sha: "base".into(),
            lane_thread_id: Uuid::new_v4(),
        }))
        .await
        .expect("batch started");

        let lane = crate::core::git::lane_branch(batch_id);
        assert_eq!(
            reviewed_heads(&pool, &lane).await.expect("heads"),
            vec!["aaa1".to_string()]
        );
        let other_lane = crate::core::git::lane_branch(Uuid::new_v4());
        assert!(reviewed_heads(&pool, &other_lane)
            .await
            .expect("heads")
            .is_empty());
        assert!(reviewed_heads(&pool, "claude-code/certified")
            .await
            .expect("heads")
            .is_empty());

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
