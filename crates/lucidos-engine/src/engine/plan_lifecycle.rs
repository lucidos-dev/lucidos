//! Durable planning stages. A start is an event, never permission to edit.
use super::event_bus::{BusEvent, EventBus};
use super::thread_events::{EventMeta, PlanKind, PlanScope, ThreadEvent};
use sha2::{Digest, Sha256};
use std::io::{Error as PlanReadError, ErrorKind};
use uuid::Uuid;

/// Serialize planning starts across engines. Retrying after an acknowledged or
/// interrupted request observes the committed event and does not start twice.
/// A kind switch or a completed approval/landing starts a new planning cycle.
pub async fn start(
    bus: &EventBus,
    thread_id: Uuid,
    kind: PlanKind,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut claim = bus.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("plan-lifecycle:{thread_id}"))
        .execute(&mut *claim)
        .await?;
    let last: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT event_type, payload->>'kind' FROM events WHERE aggregate_id = $1 \
         AND event_type IN ('PlanStarted', 'PlanProposed', 'PlanApproved', 'ChangeApplied') \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .fetch_optional(&mut *claim)
    .await?;
    if last.as_ref().is_some_and(|(event, prior_kind)| {
        matches!(event.as_str(), "PlanStarted" | "PlanProposed")
            && prior_kind.as_deref() == Some(kind.as_str())
    }) {
        return Ok(());
    }
    let recorded = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::PlanStarted { kind },
            meta: EventMeta::NONE,
        })
        .await?;
    if recorded.is_none() {
        return Err("planning start was refused".into());
    }
    claim.commit().await?;
    Ok(())
}

type Failure = Box<dyn std::error::Error + Send + Sync>;

async fn lock_marker(
    bus: &EventBus,
    repo: &str,
    branch: &str,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
    let mut claim = bus.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("plan-marker:{repo}:{branch}"))
        .execute(&mut *claim)
        .await?;
    Ok(claim)
}

pub(super) async fn committed_plan_version(
    repo: &std::path::Path,
    branch: &str,
    path: &str,
) -> Result<Option<String>, PlanReadError> {
    let Some(sha) = plan_blob_if_present(repo, branch, path).await? else {
        return Ok(None);
    };
    plan_version(repo, &sha).await.map(Some)
}

async fn plan_blob(repo: &std::path::Path, branch: &str, path: &str) -> Result<String, Failure> {
    plan_blob_if_present(repo, branch, path)
        .await?
        .ok_or_else(|| "the plan must be committed on the marked branch".into())
}

async fn plan_blob_if_present(
    repo: &std::path::Path,
    branch: &str,
    path: &str,
) -> Result<Option<String>, PlanReadError> {
    if path.is_empty()
        || std::path::Path::new(path)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || path.contains('\\')
    {
        return Err(PlanReadError::new(
            ErrorKind::InvalidData,
            "plan_path must be a relative file path without traversal",
        ));
    }
    let branch_ref = format!("refs/heads/{branch}");
    let output = super::git_ops::git_cmd(
        &[
            "--literal-pathspecs",
            "ls-tree",
            "-z",
            &branch_ref,
            "--",
            path,
        ],
        repo,
    )
    .await
    .map_err(PlanReadError::other)?;
    if !output.status.success() {
        return Err(PlanReadError::other(
            "could not read the branch's committed plan",
        ));
    }
    if output.stdout.is_empty() {
        return Ok(None);
    }
    let entry = std::str::from_utf8(&output.stdout).map_err(PlanReadError::other)?;
    let Some((metadata, _)) = entry.split_once('\t') else {
        return Err(PlanReadError::other("could not read the plan's tree entry"));
    };
    // ls-tree prints mode, type and object ID before the tab.
    match metadata
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["100644" | "100755", "blob", sha] => Ok(Some((*sha).into())),
        _ => Err(PlanReadError::new(
            ErrorKind::InvalidData,
            "the plan must name a regular file",
        )),
    }
}

async fn plan_version(repo: &std::path::Path, sha: &str) -> Result<String, PlanReadError> {
    let blob = super::git_ops::git_cmd(&["cat-file", "blob", sha], repo)
        .await
        .map_err(PlanReadError::other)?;
    if !blob.status.success() {
        return Err(PlanReadError::other("could not read the committed plan"));
    }
    let text = std::str::from_utf8(&blob.stdout)
        .map_err(|error| PlanReadError::new(ErrorKind::InvalidData, error))?;
    Ok(hash_plan_text(text))
}

fn hash_plan_text(text: &str) -> String {
    use crate::core::plan_status::{is_plan_status_line, APPROVED_STATUS};

    let mut digest = Sha256::new();
    let mut found = false;
    for line in text.split_inclusive('\n') {
        if !found && is_plan_status_line(line) {
            found = true;
            digest.update(APPROVED_STATUS);
            if line.ends_with('\n') {
                digest.update(b"\n");
            }
        } else {
            digest.update(line);
        }
    }
    format!("{:x}", digest.finalize())
}

async fn prior_implementation_approval(
    pool: &sqlx::PgPool,
    thread: Uuid,
    root: &str,
    path: &str,
    version: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    // Only engine-checked approvals are sources. Every inheritance points
    // straight to the human approval, even across several generations.
    sqlx::query_scalar(
        "WITH RECURSIVE ancestors AS ( \
             SELECT thread_id, parent_thread_id FROM thread_summaries WHERE thread_id = $1 \
             UNION SELECT t.thread_id, t.parent_thread_id FROM thread_summaries t \
                    JOIN ancestors a ON a.parent_thread_id = t.thread_id \
         ), family AS ( \
             SELECT thread_id FROM ancestors WHERE parent_thread_id IS NULL \
             UNION SELECT t.thread_id FROM thread_summaries t \
                    JOIN family f ON t.parent_thread_id = f.thread_id \
         ) SELECT e.id FROM family f JOIN events e USING (thread_id) \
           WHERE e.event_type = 'PlanApproved' AND e.payload->>'kind' = 'implementation' \
             AND e.payload->>'scope' = 'implement' AND e.payload->>'repo_root' = $2 \
             AND e.payload->>'plan_path' = $3 AND e.payload->>'plan_version' = $4 \
             AND e.payload->>'inherited_from_event_id' IS NULL \
           ORDER BY e.created DESC, e.id DESC LIMIT 1",
    )
    .bind(thread)
    .bind(root)
    .bind(path)
    .bind(version)
    .fetch_optional(pool)
    .await
}

/// Mark the committed revision. Returns whether it inherited approval.
/// The marker and its fact either both commit or neither does.
#[allow(clippy::too_many_arguments)]
pub async fn propose(
    bus: &EventBus,
    thread: Uuid,
    repo: &std::path::Path,
    branch: &str,
    path: &str,
    head: &str,
    reason: Option<&str>,
    kind: PlanKind,
) -> Result<bool, Failure> {
    let root = super::git_ops::canonical_repo_root(repo);
    let claim = lock_marker(bus, &root, branch).await?;
    let sha = plan_blob(repo, branch, path).await?;
    let (version, inherited) = if kind == PlanKind::Implementation {
        let version = plan_version(repo, &sha).await?;
        let inherited =
            prior_implementation_approval(bus.pool(), thread, &root, path, &version).await?;
        (Some(version), inherited)
    } else {
        (None, None)
    };
    let (event, state) = if let Some(origin) = inherited {
        (
            ThreadEvent::PlanApproved {
                kind: kind.into(),
                branch_name: Some(branch.into()),
                plan_path: path.into(),
                plan_sha: sha,
                scope: PlanScope::Implement,
                repo_root: Some(root.clone()),
                plan_version: version.clone(),
                inherited_from_event_id: Some(origin),
            },
            "planned",
        )
    } else {
        (
            ThreadEvent::PlanProposed {
                kind,
                plan_path: path.into(),
                plan_sha: sha,
                branch_name: branch.into(),
            },
            "proposed",
        )
    };
    let (branch, path, head, reason) = (
        branch.to_string(),
        path.to_string(),
        head.to_string(),
        reason.map(str::to_string),
    );
    // The event timestamp binds the marker to the fact in this transaction.
    let event_type = event.event_type();
    if !bus.emit_with_mutation(thread, event, move |conn| Box::pin(async move {
        sqlx::query("INSERT INTO planned_branches (repo_root, branch_name, state, plan_path, reason, files, head_sha, kind, plan_version, planned_at) VALUES ($1, $2, $8, $3, $4, NULL, $5, $6, $10, (SELECT created FROM events WHERE thread_id = $7 AND event_type = $9 ORDER BY sequence DESC LIMIT 1)) ON CONFLICT (repo_root, branch_name) DO UPDATE SET state = EXCLUDED.state, plan_path = EXCLUDED.plan_path, reason = EXCLUDED.reason, files = NULL, head_sha = EXCLUDED.head_sha, kind = EXCLUDED.kind, planned_at = EXCLUDED.planned_at, plan_version = EXCLUDED.plan_version, carried_without_plan = FALSE")
            .bind(root).bind(branch).bind(path).bind(reason).bind(head).bind(kind.as_str()).bind(thread).bind(state).bind(event_type).bind(version.filter(|_| inherited.is_some())).execute(conn).await?;
        Ok(())
    })).await? { return Err("plan marking event was refused".into()); }
    claim.commit().await?;
    Ok(inherited.is_some())
}

/// Check approval while holding the marker command lock; compare-and-swap
/// also refuses a concurrent legacy writer that does not hold that lock.
pub(crate) async fn approve(
    bus: &EventBus,
    thread: Uuid,
    repo: &std::path::Path,
    branch: &str,
    plan_only: bool,
) -> Result<Option<super::git_ops::ApprovedPlan>, super::git_ops::PlanApprovalError> {
    use super::git_ops::{ApprovalTarget, ApprovedPlan, PlanApprovalError};
    let unreadable =
        |e: Failure| PlanApprovalError::Unreadable(sqlx::Error::Protocol(e.to_string()));
    let root = super::git_ops::canonical_repo_root(repo);
    let claim = lock_marker(bus, &root, branch)
        .await
        .map_err(PlanApprovalError::Unreadable)?;
    let marker: Option<(String, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as("SELECT kind, plan_path, planned_at FROM planned_branches WHERE repo_root = $1 AND branch_name = $2 AND state = 'proposed'")
        .bind(&root).bind(branch).fetch_optional(bus.pool()).await.map_err(PlanApprovalError::Unreadable)?;
    let Some((kind, Some(path), marked_at)) = marker else {
        return Ok(None);
    };
    let kind = match kind.as_str() {
        "design" => PlanKind::Design,
        "implementation" => PlanKind::Implementation,
        _ => return Err(PlanApprovalError::PlanChanged),
    };
    let target = if plan_only {
        ApprovalTarget::PlanForLater
    } else if kind == PlanKind::Design {
        ApprovalTarget::Design
    } else {
        ApprovalTarget::Implementation
    };
    super::git_ops::user_approved_plan_for(
        bus.pool(),
        repo,
        branch,
        Some(&thread.to_string()),
        target,
    )
    .await?;
    let sha = plan_blob(repo, branch, &path).await.map_err(unreadable)?;
    // Resolve the marker's proposal before checking its owner. Filtering by
    // caller first would mistake another thread's modern proposal for legacy.
    let prior: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT thread_id, payload->>'plan_sha', payload->>'kind' FROM events \
         WHERE event_type = 'PlanProposed' AND payload->>'plan_path' = $1 \
         AND payload->>'branch_name' = $2 AND created = $3 ORDER BY sequence DESC LIMIT 1",
    )
    .bind(&path)
    .bind(branch)
    .bind(marked_at)
    .fetch_optional(bus.pool())
    .await
    .map_err(PlanApprovalError::Unreadable)?;
    if let Some((owner, revision, prior_kind)) = prior {
        if owner != thread || prior_kind != kind.as_str() {
            return Err(PlanApprovalError::PlanChanged);
        }
        if revision != sha
            && !super::git_ops::user_picked_plan_variant(
                bus.pool(),
                &thread.to_string(),
                repo,
                branch,
            )
            .await?
        {
            return Err(PlanApprovalError::PlanChanged);
        }
    }
    let plan_only = target != ApprovalTarget::Implementation;
    let version = if plan_only {
        None
    } else {
        Some(
            plan_version(repo, &sha)
                .await
                .map_err(|error| unreadable(error.into()))?,
        )
    };
    let event = ThreadEvent::PlanApproved {
        kind: kind.into(),
        branch_name: Some(branch.into()),
        plan_path: path.clone(),
        plan_sha: sha,
        scope: if plan_only {
            PlanScope::PlanOnly
        } else {
            PlanScope::Implement
        },
        repo_root: version.as_ref().map(|_| root.clone()),
        plan_version: version.clone(),
        inherited_from_event_id: None,
    };
    let branch = branch.to_string();
    let expected_path = path.clone();
    if !bus.emit_with_mutation(thread, event, move |conn| Box::pin(async move {
        let changed = sqlx::query("UPDATE planned_branches SET state = $3, planned_at = NOW(), plan_version = $7, carried_without_plan = FALSE WHERE repo_root = $1 AND branch_name = $2 AND state = 'proposed' AND planned_at = $4 AND plan_path = $5 AND kind = $6")
            .bind(root).bind(branch).bind(if plan_only { "plan_only" } else { "planned" }).bind(marked_at).bind(expected_path).bind(kind.as_str()).bind(version).execute(conn).await?;
        if changed.rows_affected() != 1 { return Err(sqlx::Error::Protocol("the plan marker changed during approval".into())); }
        Ok(())
    })).await.map_err(unreadable)? { return Err(PlanApprovalError::Unreadable(sqlx::Error::Protocol("plan approval event was refused".into()))); }
    claim
        .commit()
        .await
        .map_err(PlanApprovalError::Unreadable)?;
    Ok(Some(ApprovedPlan {
        plan_path: Some(path),
        plan_only,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};

    async fn git(repo: &std::path::Path, args: &[&str]) {
        let out = super::super::git_ops::git_cmd(args, repo).await.unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    async fn repository() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-b", "work"]).await;
        git(repo.path(), &["config", "user.name", "Test User"]).await;
        git(repo.path(), &["config", "user.email", "test@example.com"]).await;
        tokio::fs::create_dir_all(repo.path().join("docs/plans"))
            .await
            .unwrap();
        tokio::fs::write(repo.path().join("docs/plans/plan.md"), "The plan")
            .await
            .unwrap();
        git(repo.path(), &["add", "."]).await;
        git(repo.path(), &["commit", "-m", "docs: add plan"]).await;
        repo
    }

    async fn propose_test_plan(
        bus: &EventBus,
        thread: Uuid,
        repo: &std::path::Path,
        kind: PlanKind,
    ) {
        propose(
            bus,
            thread,
            repo,
            "work",
            "docs/plans/plan.md",
            "head",
            None,
            kind,
        )
        .await
        .unwrap();
    }

    async fn say(bus: &EventBus, thread: Uuid, text: &str) {
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: crate::test_support::message_received(text),
            meta: EventMeta::with_actor(Some(super::super::thread_events::MessageOrigin::Device {
                device_id: "test-device".into(),
            })),
        })
        .await
        .unwrap()
        .expect("user message");
    }

    #[tokio::test]
    async fn implementation_approval_is_reused_only_for_its_tree_repo_and_version() {
        use super::super::git_ops::{
            consume_plan_marker, plan_marker_state, PlanMarkerKind, PlanMarkerState,
        };
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let owner = seed_coding_thread(&bus, None).await;
        let child = seed_coding_thread(&bus, Some(owner)).await;
        let sibling = seed_coding_thread(&bus, Some(owner)).await;
        let unrelated = seed_coding_thread(&bus, None).await;
        let repo = repository().await;
        let other_repo = repository().await;
        let path = "docs/plans/plan.md";
        tokio::fs::write(
            repo.path().join(path),
            "- **Status**: Proposed  \r\nPlan text\n- **Status**: Proposed\n",
        )
        .await
        .unwrap();
        git(repo.path(), &["commit", "-am", "docs: add plan status"]).await;
        propose_test_plan(&bus, owner, repo.path(), PlanKind::Implementation).await;
        say(&bus, owner, "approve").await;
        approve(&bus, owner, repo.path(), "work", false)
            .await
            .unwrap()
            .unwrap();
        git(repo.path(), &["branch", "-m", "work", "moved"]).await;
        assert_eq!(
            plan_marker_state(&pool, repo.path(), "work").await,
            PlanMarkerState::Unreadable
        );
        git(repo.path(), &["branch", "-m", "moved", "work"]).await;
        let origin: Uuid = sqlx::query_scalar(
            "SELECT id FROM events WHERE thread_id = $1 AND event_type = 'PlanApproved'",
        )
        .bind(owner)
        .fetch_one(&pool)
        .await
        .unwrap();

        tokio::fs::write(
            repo.path().join(path),
            "- **Status**: Approved  \r\nPlan text\n- **Status**: Proposed\n",
        )
        .await
        .unwrap();
        git(repo.path(), &["commit", "-am", "docs: flip plan status"]).await;
        assert_eq!(
            plan_marker_state(&pool, repo.path(), "work").await,
            PlanMarkerState::Present(PlanMarkerKind::Planned)
        );
        git(repo.path(), &["branch", "child"]).await;
        consume_plan_marker(&pool, repo.path(), "work").await;
        assert_eq!(
            plan_marker_state(&pool, repo.path(), "work").await,
            PlanMarkerState::Missing
        );
        for (thread, branch) in [(child, "child"), (sibling, "work")] {
            assert!(propose(
                &bus,
                thread,
                repo.path(),
                branch,
                path,
                "head",
                None,
                PlanKind::Implementation
            )
            .await
            .unwrap());
            assert_eq!(
                plan_marker_state(&pool, repo.path(), branch).await,
                PlanMarkerState::Present(PlanMarkerKind::Planned)
            );
            let inherited: String = sqlx::query_scalar(
                "SELECT payload->>'inherited_from_event_id' FROM events WHERE thread_id = $1 AND event_type = 'PlanApproved'",
            ).bind(thread).fetch_one(&pool).await.unwrap();
            assert_eq!(inherited, origin.to_string());
        }

        tokio::fs::copy(repo.path().join(path), other_repo.path().join(path))
            .await
            .unwrap();
        git(other_repo.path(), &["commit", "-am", "docs: copy plan"]).await;
        assert!(!propose(
            &bus,
            child,
            other_repo.path(),
            "work",
            path,
            "head",
            None,
            PlanKind::Implementation
        )
        .await
        .unwrap());
        tokio::fs::write(
            repo.path().join(path),
            "- **Status**: Approved\nChanged text\n",
        )
        .await
        .unwrap();
        git(repo.path(), &["commit", "-am", "docs: change plan body"]).await;
        assert_eq!(
            plan_marker_state(&pool, repo.path(), "work").await,
            PlanMarkerState::Changed
        );
        git(repo.path(), &["restore", "--source", "HEAD^", "--", path]).await;
        git(
            repo.path(),
            &["commit", "-am", "docs: restore approved plan"],
        )
        .await;
        assert_eq!(
            plan_marker_state(&pool, repo.path(), "work").await,
            PlanMarkerState::Present(PlanMarkerKind::Planned)
        );
        assert!(!propose(
            &bus,
            unrelated,
            repo.path(),
            "work",
            path,
            "head",
            None,
            PlanKind::Implementation
        )
        .await
        .unwrap());
        git(repo.path(), &["revert", "--no-edit", "HEAD"]).await;
        assert!(!propose(
            &bus,
            child,
            repo.path(),
            "work",
            path,
            "head",
            None,
            PlanKind::Implementation
        )
        .await
        .unwrap());
        say(&bus, child, "for later").await;
        approve(&bus, child, repo.path(), "work", true)
            .await
            .unwrap()
            .unwrap();
        assert!(!propose(
            &bus,
            sibling,
            repo.path(),
            "work",
            path,
            "head",
            None,
            PlanKind::Implementation
        )
        .await
        .unwrap());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_child_inherits_an_approved_marker_before_its_first_session() {
        use super::super::git_ops::{
            carry_child_planned, plan_marker_state, PlanMarkerKind, PlanMarkerState,
        };
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let parent = seed_coding_thread(&bus, None).await;
        let child = seed_coding_thread(&bus, Some(parent)).await;
        let repo = repository().await;
        let path = "docs/plans/plan.md";
        git(repo.path(), &["switch", "-c", "child"]).await;
        git(repo.path(), &["rm", path]).await;
        git(
            repo.path(),
            &["commit", "-m", "chore: start from base without plan"],
        )
        .await;
        git(repo.path(), &["switch", "work"]).await;
        let start_parent = |folder: &std::path::Path| BusEvent::Thread {
            thread_id: parent,
            event: ThreadEvent::SessionStarted {
                sandbox: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: Uuid::new_v4().to_string(),
                branch: "work".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: folder.to_string_lossy().into_owned(),
                app_id: None,
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
            meta: EventMeta {
                channel: Some(super::super::thread_events::EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        };
        bus.emit(start_parent(repo.path())).await.unwrap();
        propose_test_plan(&bus, parent, repo.path(), PlanKind::Implementation).await;
        carry_child_planned(&pool, repo.path(), child, "child")
            .await
            .unwrap();
        assert_eq!(
            plan_marker_state(&pool, repo.path(), "child").await,
            PlanMarkerState::Missing
        );
        say(&bus, parent, "approve").await;
        approve(&bus, parent, repo.path(), "work", false)
            .await
            .unwrap()
            .unwrap();
        let other_repo = repository().await;
        bus.emit(start_parent(other_repo.path())).await.unwrap();
        carry_child_planned(&pool, repo.path(), child, "child")
            .await
            .unwrap();
        assert_eq!(
            plan_marker_state(&pool, repo.path(), "child").await,
            PlanMarkerState::Missing
        );
        bus.emit(start_parent(repo.path())).await.unwrap();
        carry_child_planned(&pool, repo.path(), child, "child")
            .await
            .unwrap();
        assert_eq!(
            plan_marker_state(&pool, repo.path(), "child").await,
            PlanMarkerState::Present(PlanMarkerKind::Planned)
        );
        git(repo.path(), &["switch", "child"]).await;
        tokio::fs::create_dir_all(repo.path().join("docs/plans"))
            .await
            .unwrap();
        tokio::fs::write(repo.path().join(path), "Edited plan on child branch")
            .await
            .unwrap();
        git(repo.path(), &["add", path]).await;
        git(repo.path(), &["commit", "-m", "docs: revise carried plan"]).await;
        assert_eq!(
            plan_marker_state(&pool, repo.path(), "child").await,
            PlanMarkerState::Changed
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn approval_checks_the_markers_own_proposal_revision() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let first = repository().await;
        let second = repository().await;
        tokio::fs::write(second.path().join("docs/plans/plan.md"), "Another plan")
            .await
            .unwrap();
        git(second.path(), &["commit", "-am", "docs: revise plan"]).await;
        propose_test_plan(&bus, thread, first.path(), PlanKind::Implementation).await;
        propose_test_plan(&bus, thread, second.path(), PlanKind::Implementation).await;
        say(&bus, thread, "approve").await;
        assert!(approve(&bus, thread, first.path(), "work", false)
            .await
            .unwrap()
            .is_some());
        // The other marker still checks its original blob after the first approval.
        tokio::fs::write(second.path().join("docs/plans/plan.md"), "Changed plan")
            .await
            .unwrap();
        git(second.path(), &["commit", "-am", "docs: change plan"]).await;
        assert!(matches!(
            approve(&bus, thread, second.path(), "work", false).await,
            Err(crate::engine::git_ops::PlanApprovalError::PlanChanged)
        ));
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_direct_variant_pick_approves_its_revised_committed_blob() {
        use super::super::thread_events::{AnswerKind, MessageOrigin};
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let repo = repository().await;
        propose_test_plan(&bus, thread, repo.path(), PlanKind::Implementation).await;
        let tool = "plan-variant";
        let input = serde_json::json!({"questions":[{"question":"Approve?", "options":[{"label":"Approve"},{"label":"Frontend only"}]}]});
        let question =
            crate::engine::question_parsing::parse_ask_user_question_inputs(&input).remove(0);
        let option_id = question.options[1].id.clone();
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::UserQuestionAsked {
                tool_use_id: tool.into(),
                cc_session_id: String::new(),
                options: question.options,
                question: question.question,
                worktree_path: None,
                multi_select: false,
                owner_approval: None,
                permission_request: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::UserQuestionAnswered {
                compose: None,
                tool_use_id: tool.into(),
                quiet: false,
                answer: AnswerKind::Selected { option_id },
            },
            meta: EventMeta::with_actor(Some(MessageOrigin::Device {
                device_id: "test-device".into(),
            })),
        })
        .await
        .unwrap();
        tokio::fs::write(repo.path().join("docs/plans/plan.md"), "Frontend only")
            .await
            .unwrap();
        git(
            repo.path(),
            &["commit", "-am", "docs: select frontend variant"],
        )
        .await;
        let revision = plan_blob(repo.path(), "work", "docs/plans/plan.md")
            .await
            .unwrap();
        assert!(approve(&bus, thread, repo.path(), "work", false)
            .await
            .unwrap()
            .is_some());
        let approved: String = sqlx::query_scalar(
            "SELECT payload->>'plan_sha' FROM events WHERE thread_id = $1 AND event_type = 'PlanApproved'",
        ).bind(thread).fetch_one(&pool).await.unwrap();
        assert_eq!(approved, revision);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn approval_refuses_another_threads_modern_proposal() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let owner = seed_coding_thread(&bus, None).await;
        let other = seed_coding_thread(&bus, None).await;
        let repo = repository().await;
        propose_test_plan(&bus, owner, repo.path(), PlanKind::Implementation).await;
        say(&bus, other, "approve").await;
        assert!(matches!(
            approve(&bus, other, repo.path(), "work", false).await,
            Err(crate::engine::git_ops::PlanApprovalError::PlanChanged)
        ));
        tokio::fs::write(repo.path().join("docs/plans/plan.md"), "Changed plan")
            .await
            .unwrap();
        git(repo.path(), &["commit", "-am", "docs: change plan"]).await;
        assert!(matches!(
            approve(&bus, other, repo.path(), "work", false).await,
            Err(crate::engine::git_ops::PlanApprovalError::PlanChanged)
        ));
        let state: String =
            sqlx::query_scalar("SELECT state FROM planned_branches WHERE branch_name = 'work'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "proposed");
        let approvals: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'PlanApproved'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(approvals, 0, "a foreign caller cannot attest any revision");
        tokio::fs::write(repo.path().join("docs/plans/plan.md"), "The plan")
            .await
            .unwrap();
        git(
            repo.path(),
            &["commit", "-am", "docs: restore proposed plan"],
        )
        .await;
        say(&bus, owner, "approve").await;
        assert!(approve(&bus, owner, repo.path(), "work", false)
            .await
            .unwrap()
            .is_some());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn approval_accepts_a_threadless_legacy_marker() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let repo = repository().await;
        crate::engine::git_ops::record_planned(
            &pool,
            repo.path(),
            "work",
            crate::engine::git_ops::PlanMarkerKind::Proposed,
            Some("docs/plans/plan.md"),
            None,
            &[],
            "head",
        )
        .await
        .unwrap();
        say(&bus, thread, "approve").await;
        assert!(approve(&bus, thread, repo.path(), "work", false)
            .await
            .unwrap()
            .is_some());
        let proposals: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'PlanProposed'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(proposals, 0, "legacy approval needs no fabricated proposal");
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn batch_binding_scopes_same_branch_to_repository_and_refuses_legacy_ambiguity() {
        use crate::engine::git_ops::{canonical_repo_root, proposed_plan_binding};
        use crate::runtime::CodingAgent;
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let first = repository().await;
        let second = repository().await;
        let thread = seed_coding_thread(&bus, None).await;
        let other = seed_coding_thread(&bus, None).await;
        propose_test_plan(&bus, thread, first.path(), PlanKind::Implementation).await;
        propose_test_plan(&bus, other, second.path(), PlanKind::Implementation).await;
        for (folder, expected) in [
            (
                first.path().to_string_lossy().to_string(),
                Some(first.path()),
            ),
            (
                second.path().to_string_lossy().to_string(),
                Some(second.path()),
            ),
            (String::new(), None),
        ] {
            bus.emit(BusEvent::Thread {
                thread_id: thread,
                event: ThreadEvent::SessionStarted {
                    sandbox: None,
                    coding_agent: CodingAgent::Codex,
                    session_id: Uuid::new_v4().to_string(),
                    branch: "work".into(),
                    repo_id: None,
                    coding_agent_kind: Default::default(),
                    coding_agent_folder: folder,
                    app_id: None,
                    integrated: false,
                    auto_compact_window: None,
                    auto_compact_arm: None,
                    continuity: None,
                },
                meta: EventMeta {
                    channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap();
            let binding = proposed_plan_binding(&pool, thread).await;
            if let Some(repo) = expected {
                assert_eq!(
                    binding.unwrap().unwrap().repo_root,
                    canonical_repo_root(repo)
                );
            } else {
                assert!(binding.unwrap_err().contains("multiple plan markers"));
            }
        }
        sqlx::query("DELETE FROM planned_branches WHERE repo_root = $1")
            .bind(canonical_repo_root(second.path()))
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            proposed_plan_binding(&pool, thread)
                .await
                .unwrap()
                .unwrap()
                .repo_root,
            canonical_repo_root(first.path())
        );
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn proposals_bind_git_blobs_and_remarking_records_a_new_proposal() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let repo = repository().await;
        propose_test_plan(&bus, thread, repo.path(), PlanKind::Design).await;
        propose_test_plan(&bus, thread, repo.path(), PlanKind::Design).await;
        let shas: Vec<String> = sqlx::query_scalar("SELECT payload->>'plan_sha' FROM events WHERE thread_id = $1 AND event_type = 'PlanProposed'")
            .bind(thread).fetch_all(&pool).await.unwrap();
        let expected = plan_blob(repo.path(), "work", "docs/plans/plan.md")
            .await
            .unwrap();
        assert_eq!(shas, [expected.clone(), expected]);
        let marker: (String, String) = sqlx::query_as("SELECT state, kind FROM planned_branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(marker, ("proposed".into(), "design".into()));
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn failed_marker_mutation_rolls_back_the_event() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let error = bus.emit_with_mutation(thread, ThreadEvent::PlanStarted { kind: PlanKind::Design }, |conn| Box::pin(async move {
            sqlx::query("INSERT INTO planned_branches (repo_root, branch_name, state, head_sha, planned_at) VALUES ('/test', 'work', 'proposed', 'head', NOW())").execute(&mut *conn).await?;
            Err(sqlx::Error::Protocol("injected failure after marker write".into()))
        })).await.unwrap_err();
        assert!(error.to_string().contains("injected failure"));
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'PlanStarted'",
        )
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
        let markers: i64 = sqlx::query_scalar("SELECT count(*) FROM planned_branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((events, markers), (0, 0));
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn refused_proposal_never_writes_a_marker() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let repo = repository().await;
        crate::test_support::refuse_event_type(&pool, "PlanProposed").await;
        assert!(propose(
            &bus,
            thread,
            repo.path(),
            "work",
            "docs/plans/plan.md",
            "head",
            None,
            PlanKind::Design
        )
        .await
        .is_err());
        let markers: i64 = sqlx::query_scalar("SELECT count(*) FROM planned_branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(markers, 0);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn refused_approval_leaves_the_marker_proposed() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let repo = repository().await;
        propose_test_plan(&bus, thread, repo.path(), PlanKind::Implementation).await;
        say(&bus, thread, "approve").await;
        crate::test_support::refuse_event_type(&pool, "PlanApproved").await;
        assert!(approve(&bus, thread, repo.path(), "work", false)
            .await
            .is_err());
        let state: String = sqlx::query_scalar("SELECT state FROM planned_branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        let facts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'PlanApproved'",
        )
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, "proposed");
        assert_eq!(facts, 0);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn concurrent_approval_records_one_scoped_fact_and_rejects_changed_blobs() {
        for (kind, later, reply, scope) in [
            (PlanKind::Design, false, "approve design", "plan-only"),
            (PlanKind::Implementation, true, "for later", "plan-only"),
            (PlanKind::Implementation, false, "approve", "implement"),
        ] {
            let (pool, db) = setup_test_db().await;
            let (bus, _callbacks) = EventBus::new(pool.clone());
            let thread = seed_coding_thread(&bus, None).await;
            let repo = repository().await;
            propose_test_plan(&bus, thread, repo.path(), kind).await;
            say(&bus, thread, reply).await;
            let (a, b) = tokio::join!(
                approve(&bus, thread, repo.path(), "work", later),
                approve(&bus, thread, repo.path(), "work", later)
            );
            assert_eq!(
                usize::from(a.unwrap().is_some()) + usize::from(b.unwrap().is_some()),
                1
            );
            let scopes: Vec<String> = sqlx::query_scalar("SELECT payload->>'scope' FROM events WHERE thread_id = $1 AND event_type = 'PlanApproved'").bind(thread).fetch_all(&pool).await.unwrap();
            assert_eq!(scopes, [scope]);
            let branch: String = sqlx::query_scalar("SELECT payload->>'branch_name' FROM events WHERE thread_id = $1 AND event_type = 'PlanApproved'")
                .bind(thread).fetch_one(&pool).await.unwrap();
            assert_eq!(branch, "work");
            sqlx::query("DELETE FROM planned_branches")
                .execute(&pool)
                .await
                .unwrap();
            let historical = ThreadEvent::PlanApproved {
                kind: kind.into(),
                branch_name: None,
                plan_path: "docs/plans/plan.md".into(),
                plan_sha: plan_blob(repo.path(), "work", "docs/plans/plan.md")
                    .await
                    .unwrap(),
                scope: if scope == "implement" {
                    super::super::thread_events::PlanScope::Implement
                } else {
                    super::super::thread_events::PlanScope::PlanOnly
                },
                repo_root: None,
                plan_version: None,
                inherited_from_event_id: None,
            };
            let mut replay = pool.begin().await.unwrap();
            bus.update_thread_projection(&mut replay, thread, &historical, &EventMeta::NONE)
                .await
                .unwrap();
            replay.commit().await.unwrap();
            let markers: i64 = sqlx::query_scalar("SELECT count(*) FROM planned_branches")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(
                markers, 0,
                "replaying an approval never restores consumed permission"
            );
            tokio::fs::write(repo.path().join("docs/plans/plan.md"), "New plan")
                .await
                .unwrap();
            git(
                repo.path(),
                &["commit", "-am", "docs: start a new plan version"],
            )
            .await;
            propose_test_plan(&bus, thread, repo.path(), kind).await;
            say(&bus, thread, reply).await;
            tokio::fs::write(repo.path().join("docs/plans/plan.md"), "Changed plan")
                .await
                .unwrap();
            git(repo.path(), &["commit", "-am", "docs: change plan"]).await;
            assert!(matches!(
                approve(&bus, thread, repo.path(), "work", later).await,
                Err(super::super::git_ops::PlanApprovalError::PlanChanged)
            ));
            teardown_test_db(&db).await;
        }
    }

    #[tokio::test]
    async fn failed_append_is_reported_without_a_stage_fact() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        crate::test_support::refuse_event_type(&pool, "PlanStarted").await;
        let thread = seed_coding_thread(&bus, None).await;
        let error = start(&bus, thread, PlanKind::Design).await.unwrap_err();
        assert!(error.to_string().contains("refused"), "{error}");
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE aggregate_id = $1 AND event_type = 'PlanStarted'",
        )
        .bind(thread.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn starts_follow_the_latest_proposals_kind() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let repo = repository().await;
        start(&bus, thread, PlanKind::Implementation).await.unwrap();
        propose_test_plan(&bus, thread, repo.path(), PlanKind::Design).await;
        start(&bus, thread, PlanKind::Design).await.unwrap();
        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT payload->>'kind' FROM events WHERE thread_id = $1 \
             AND event_type = 'PlanStarted' ORDER BY sequence",
        )
        .bind(thread)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            kinds,
            ["implementation"],
            "a proposal already began this kind"
        );
        start(&bus, thread, PlanKind::Implementation).await.unwrap();
        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT payload->>'kind' FROM events WHERE thread_id = $1 \
             AND event_type = 'PlanStarted' ORDER BY sequence",
        )
        .bind(thread)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(kinds, ["implementation", "implementation"]);
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn retries_and_concurrent_starts_record_one_stage_and_no_marker() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callbacks) = EventBus::new(pool.clone());
        let thread = seed_coding_thread(&bus, None).await;
        let (first, second) = tokio::join!(
            start(&bus, thread, PlanKind::Implementation),
            start(&bus, thread, PlanKind::Implementation)
        );
        first.unwrap();
        second.unwrap();
        start(&bus, thread, PlanKind::Implementation).await.unwrap();
        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT payload->>'kind' FROM events WHERE aggregate_id = $1 AND event_type = 'PlanStarted' ORDER BY sequence",
        ).bind(thread.to_string()).fetch_all(&pool).await.unwrap();
        assert_eq!(kinds, ["implementation"]);
        start(&bus, thread, PlanKind::Design).await.unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE aggregate_id = $1 AND event_type = 'PlanStarted'",
        )
        .bind(thread.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 2, "a different kind starts a real stage");
        let markers: i64 = sqlx::query_scalar("SELECT count(*) FROM planned_branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(markers, 0, "a start never opens the edit gate");
        teardown_test_db(&db).await;
    }
}
