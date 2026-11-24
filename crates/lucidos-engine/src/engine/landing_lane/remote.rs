//! The landing hand-off, carried by annotated tags on the repository's origin.

use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use uuid::Uuid;

use super::LaneMember;
use crate::engine::thread_events::landing_lane::RemoteLaneSource;

use crate::engine::git_ops::{fetch_origin_object, list_origin_refs, push_origin_refs};

pub(super) const QUEUE: &str = "refs/lucidos/queue";
pub(super) const RESULT: &str = "refs/lucidos/result";

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct QueueNote {
    pub title: String,
    pub summary: String,
    pub branch: String,
    pub gateway: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ResultNote {
    pub reason: String,
    pub removed: bool,
}

pub(super) async fn push_queue(
    repo: &Path,
    id: Uuid,
    snapshot: &str,
    note: QueueNote,
) -> Result<(), String> {
    let name = format!("{QUEUE}/{id}");
    if let Some((oid, target, _)) = read_ref::<QueueNote>(repo, &name).await? {
        return if target == snapshot {
            Ok(())
        } else {
            Err(format!(
                "Origin already has a different snapshot at {name} ({oid})"
            ))
        };
    }
    let oid = write_tag(repo.to_path_buf(), id, snapshot.to_string(), note).await?;
    let update = format!("{oid}:{name}");
    let lease = format!("--force-with-lease={name}:");
    push_origin_refs(repo, &[&lease, &update]).await?;
    crate::landing_gateway::report_main_landed();
    Ok(())
}

/// Read each advertised queue entry by its exact object ID. A bad entry
/// stays on origin and does not stop unrelated entries from being read.
pub(super) async fn read_queue(repo: &Path) -> Result<Vec<Result<LaneMember, String>>, String> {
    let refs = list_origin_refs(repo, &format!("{QUEUE}/*")).await?;
    let mut members = Vec::new();
    for (name, oid) in refs {
        let Some(id) = name
            .strip_prefix(&format!("{QUEUE}/"))
            .and_then(|suffix| Uuid::parse_str(suffix).ok())
            .filter(|id| name == format!("{QUEUE}/{id}"))
        else {
            continue;
        };
        members.push(
            read_tag::<QueueNote>(repo, oid)
                .await
                .map(|(oid, snapshot_sha, note)| LaneMember {
                    change_id: id,
                    snapshot_sha,
                    remote: Some(RemoteLaneSource {
                        oid,
                        title: note.title,
                        summary: note.summary,
                        branch: note.branch,
                        gateway: note.gateway,
                    }),
                })
                .map_err(|error| format!("Could not read {name}: {error}")),
        );
    }
    Ok(members)
}

/// Move main and consume exactly the queue advertisements used to build it,
/// or hand back the members with result tags in the same transaction.
async fn finish(
    repo: &Path,
    head: Option<&str>,
    members: &[&LaneMember],
    result: Option<(&str, bool)>,
) -> Result<(), String> {
    let mut updates = Vec::new();
    if let Some(head) = head {
        updates.push(format!("{head}:refs/heads/main"));
    }
    for member in members {
        let Some(source) = &member.remote else {
            continue;
        };
        let id = member.change_id;
        let queue = format!("{QUEUE}/{id}");
        match queue_oid(repo, id).await? {
            Some(oid) if oid == source.oid => {}
            // The old advertisement already left the queue. Never publish
            // its failure against a replacement, even for the same snapshot.
            _ if result.is_some() => continue,
            _ => {
                return Err(format!(
                    "Origin no longer advertises the expected queue ref for {id}"
                ))
            }
        }
        if result.is_none() && read_result(repo, id).await?.is_some() {
            return Err(format!("Origin already has a result for {id}"));
        }
        if let Some((reason, removed)) = result {
            let name = format!("{RESULT}/{id}");
            let oid = write_tag(
                repo.to_path_buf(),
                id,
                member.snapshot_sha.clone(),
                ResultNote {
                    reason: reason.into(),
                    removed,
                },
            )
            .await?;
            updates.push(format!("--force-with-lease={name}:"));
            updates.push(format!("{oid}:{name}"));
        }
        updates.push(format!("--force-with-lease={queue}:{}", source.oid));
        updates.push(format!(":{queue}"));
    }
    if !updates.is_empty() {
        push_origin_refs(
            repo,
            &updates.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .await?;
        crate::landing_gateway::report_main_landed();
    }
    Ok(())
}

pub(super) async fn queue_oid(repo: &Path, id: Uuid) -> Result<Option<String>, String> {
    let name = format!("{QUEUE}/{id}");
    Ok(list_origin_refs(repo, &name)
        .await?
        .into_iter()
        .find_map(|(found, oid)| (found == name).then_some(oid)))
}

pub(super) async fn land(repo: &Path, head: &str, members: &[&LaneMember]) -> Result<(), String> {
    finish(repo, Some(head), members, None).await
}

pub(super) async fn reject(
    repo: &Path,
    members: &[&LaneMember],
    reason: &str,
    removed: bool,
) -> Result<(), String> {
    finish(repo, None, members, Some((reason, removed))).await
}

pub(super) async fn read_result(
    repo: &Path,
    id: Uuid,
) -> Result<Option<(String, String, ResultNote)>, String> {
    read_ref(repo, &format!("{RESULT}/{id}")).await
}

pub(super) async fn clear_result(repo: &Path, id: Uuid, oid: &str) -> Result<(), String> {
    let name = format!("{RESULT}/{id}");
    // Result publication already consumed the old queue tag atomically.
    // A queue tag here belongs to a different advertisement.
    push_origin_refs(
        repo,
        &[
            &format!("--force-with-lease={name}:{oid}"),
            &format!(":{name}"),
        ],
    )
    .await
}

async fn read_ref<T: DeserializeOwned + Send + 'static>(
    repo: &Path,
    name: &str,
) -> Result<Option<(String, String, T)>, String> {
    let refs = list_origin_refs(repo, name).await?;
    let Some((_, oid)) = refs.into_iter().find(|(found, _)| found == name) else {
        return Ok(None);
    };
    read_tag(repo, oid).await.map(Some)
}

async fn read_tag<T: DeserializeOwned + Send + 'static>(
    repo: &Path,
    oid: String,
) -> Result<(String, String, T), String> {
    let parsed = git2::Oid::from_str(&oid).map_err(|e| e.to_string())?;
    fetch_origin_object(repo, &oid).await?;
    let repo = repo.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let repo = git2::Repository::open(repo).map_err(|e| e.to_string())?;
        let tag = repo.find_tag(parsed).map_err(|e| e.to_string())?;
        if tag.target_type() != Some(git2::ObjectType::Commit) {
            return Err("The landing tag does not target a commit.".into());
        }
        let message = tag
            .message()
            .ok_or("The landing tag has no UTF-8 message.")?;
        let note = serde_json::from_str(message).map_err(|e| e.to_string())?;
        Ok((oid, tag.target_id().to_string(), note))
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn write_tag(
    repo: PathBuf,
    id: Uuid,
    snapshot: String,
    note: impl Serialize + Send + 'static,
) -> Result<String, String> {
    let message = serde_json::to_string(&note).map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let repo = git2::Repository::open(repo)?;
        let target = repo.find_object(
            git2::Oid::from_str(&snapshot)?,
            Some(git2::ObjectType::Commit),
        )?;
        let tagger = git2::Signature::now("Lucidos", "lucidos@localhost")?;
        // Distinguish a newly advertised tag from a consumed one, even
        // when the same snapshot is requeued within the same second.
        let name = format!("{id}-{}", Uuid::new_v4());
        repo.tag_annotation_create(&name, &target, &tagger, &message)
    })
    .await
    .map_err(|e| e.to_string())?
    .map(|oid| oid.to_string())
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::git_ops::common::{git_ok, OriginFixture};

    fn note() -> QueueNote {
        QueueNote {
            title: "Change".into(),
            summary: "A test change".into(),
            branch: "test-change".into(),
            gateway: None,
        }
    }

    #[tokio::test]
    async fn a_queue_ref_holds_the_snapshot_and_survives_retries() {
        let fixture = OriginFixture::new().await;
        let id = Uuid::new_v4();
        let head = fixture.local_main().await;
        push_queue(&fixture.repo, id, &head, note()).await.unwrap();
        let name = format!("{QUEUE}/{id}");
        let (first, snapshot, payload) = read_ref::<QueueNote>(&fixture.other, &name)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot, head);
        assert_eq!(payload.title, "Change");
        push_queue(&fixture.repo, id, &head, note()).await.unwrap();
        let refs = list_origin_refs(&fixture.other, &name).await.unwrap();
        assert_eq!(refs, vec![(name, first)]);
    }

    #[tokio::test]
    async fn landing_consumes_the_queue_only_when_main_moves_on_origin() {
        let fixture = OriginFixture::new().await;
        let base = fixture.origin_main().await;
        let id = Uuid::new_v4();
        git_ok(
            &fixture.repo,
            &["commit", "--allow-empty", "-m", "snapshot"],
        )
        .await;
        let snapshot = fixture.local_main().await;
        push_queue(&fixture.repo, id, &snapshot, note())
            .await
            .unwrap();
        let member = read_queue(&fixture.other)
            .await
            .unwrap()
            .pop()
            .unwrap()
            .unwrap();
        fixture.refuse_pushes();
        assert!(land(&fixture.other, &snapshot, &[&member]).await.is_err());
        assert_eq!(fixture.origin_main().await, base);
        assert!(queue_oid(&fixture.repo, id).await.unwrap().is_some());
        std::fs::remove_file(fixture.origin.join("hooks/pre-receive")).unwrap();
        land(&fixture.other, &snapshot, &[&member]).await.unwrap();
        assert_eq!(fixture.origin_main().await, snapshot);
        assert!(queue_oid(&fixture.repo, id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_rejected_atomic_result_keeps_the_source_queued() {
        let fixture = OriginFixture::new().await;
        let id = Uuid::new_v4();
        let snapshot = fixture.local_main().await;
        push_queue(&fixture.repo, id, &snapshot, note())
            .await
            .unwrap();
        let member = read_queue(&fixture.other)
            .await
            .unwrap()
            .pop()
            .unwrap()
            .unwrap();
        fixture.refuse_pushes();
        assert!(reject(&fixture.other, &[&member], "Removed", true)
            .await
            .is_err());
        assert!(read_result(&fixture.repo, id).await.unwrap().is_none());
        assert!(queue_oid(&fixture.repo, id).await.unwrap().is_some());
        std::fs::remove_file(fixture.origin.join("hooks/pre-receive")).unwrap();
        reject(&fixture.other, &[&member], "Removed", true)
            .await
            .unwrap();
        let (_, target, result) = read_result(&fixture.repo, id).await.unwrap().unwrap();
        assert!(result.removed);
        assert_eq!(target, snapshot);
        assert!(queue_oid(&fixture.repo, id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn an_old_batch_cannot_return_a_result_for_a_requeued_snapshot() {
        let fixture = OriginFixture::new().await;
        let id = Uuid::new_v4();
        let snapshot = fixture.local_main().await;
        push_queue(&fixture.repo, id, &snapshot, note())
            .await
            .unwrap();
        let old = read_queue(&fixture.other)
            .await
            .unwrap()
            .pop()
            .unwrap()
            .unwrap();
        land(&fixture.other, &snapshot, &[&old]).await.unwrap();
        push_queue(&fixture.repo, id, &snapshot, note())
            .await
            .unwrap();
        let replacement = queue_oid(&fixture.repo, id).await.unwrap().unwrap();
        assert_ne!(old.remote.as_ref().unwrap().oid, replacement);
        assert!(land(&fixture.other, &snapshot, &[&old]).await.is_err());
        reject(&fixture.other, &[&old], "Old batch canceled", false)
            .await
            .unwrap();
        assert!(read_result(&fixture.repo, id).await.unwrap().is_none());
        assert_eq!(
            queue_oid(&fixture.repo, id).await.unwrap(),
            Some(replacement)
        );
    }

    #[tokio::test]
    async fn clearing_a_result_leaves_a_new_queue_tag_alone() {
        let fixture = OriginFixture::new().await;
        let id = Uuid::new_v4();
        let snapshot = fixture.local_main().await;
        push_queue(&fixture.repo, id, &snapshot, note())
            .await
            .unwrap();
        let old = read_queue(&fixture.other)
            .await
            .unwrap()
            .pop()
            .unwrap()
            .unwrap();
        reject(&fixture.other, &[&old], "Removed", true)
            .await
            .unwrap();
        let (result, _, _) = read_result(&fixture.repo, id).await.unwrap().unwrap();
        push_queue(&fixture.repo, id, &snapshot, note())
            .await
            .unwrap();
        let replacement = queue_oid(&fixture.repo, id).await.unwrap().unwrap();
        clear_result(&fixture.repo, id, &result).await.unwrap();
        assert!(read_result(&fixture.repo, id).await.unwrap().is_none());
        assert_eq!(
            queue_oid(&fixture.repo, id).await.unwrap(),
            Some(replacement)
        );
    }

    #[tokio::test]
    async fn a_malformed_result_does_not_read_as_absent() {
        let fixture = OriginFixture::new().await;
        let id = Uuid::new_v4();
        let head = fixture.local_main().await;
        git_ok(
            &fixture.repo,
            &["push", "origin", &format!("{head}:{RESULT}/{id}")],
        )
        .await;
        assert!(read_result(&fixture.other, id).await.is_err());
        assert!(read_result(&fixture.other, Uuid::new_v4())
            .await
            .unwrap()
            .is_none());
    }
}
