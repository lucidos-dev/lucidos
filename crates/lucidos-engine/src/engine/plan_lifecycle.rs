//! Durable planning stages. A start is an event, never permission to edit.
use super::event_bus::{BusEvent, EventBus};
use super::thread_events::{EventMeta, PlanKind, ThreadEvent};
use uuid::Uuid;

/// Serialize planning starts across engines. Retrying after an acknowledged or
/// interrupted request observes the committed event and does not start twice.
/// A kind switch or a completed approval/landing starts a new planning cycle.
pub(crate) async fn start(
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

async fn plan_blob(repo: &std::path::Path, branch: &str, path: &str) -> Result<String, Failure> {
    if path.is_empty()
        || std::path::Path::new(path)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || path.contains('\\')
    {
        return Err("plan_path must be a relative file path without traversal".into());
    }
    let spec = format!("{branch}:{path}");
    let output = super::git_ops::git_cmd(&["rev-parse", "--verify", &spec], repo).await?;
    if !output.status.success() {
        return Err("the plan must be committed on the marked branch".into());
    }
    let sha = String::from_utf8(output.stdout)?.trim().to_string();
    let object = super::git_ops::git_cmd(&["cat-file", "-t", &sha], repo).await?;
    if !object.status.success() || object.stdout != b"blob\n" {
        return Err("the plan must name a git blob".into());
    }
    Ok(sha)
}

/// A re-mark is a new proposal, even when the same blob is proposed again.
/// The marker and its fact either both commit or neither does.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn propose(
    bus: &EventBus,
    thread: Uuid,
    repo: &std::path::Path,
    branch: &str,
    path: &str,
    head: &str,
    reason: Option<&str>,
    kind: PlanKind,
) -> Result<(), Failure> {
    let root = super::git_ops::canonical_repo_root(repo);
    let claim = lock_marker(bus, &root, branch).await?;
    let sha = plan_blob(repo, branch, path).await?;
    let event = ThreadEvent::PlanProposed {
        kind,
        plan_path: path.into(),
        plan_sha: sha,
        branch_name: branch.into(),
    };
    let (branch, path, head, reason) = (
        branch.to_string(),
        path.to_string(),
        head.to_string(),
        reason.map(str::to_string),
    );
    // The append has already persisted this thread's proposal in the same
    // transaction. Its timestamp binds this marker to that exact event without
    // making replay responsible for restoring marker permission.
    if !bus.emit_with_mutation(thread, event, move |conn| Box::pin(async move {
        sqlx::query("INSERT INTO planned_branches (repo_root, branch_name, state, plan_path, reason, files, head_sha, kind, planned_at) VALUES ($1, $2, 'proposed', $3, $4, NULL, $5, $6, (SELECT created FROM events WHERE thread_id = $7 AND event_type = 'PlanProposed' ORDER BY sequence DESC LIMIT 1)) ON CONFLICT (repo_root, branch_name) DO UPDATE SET state = 'proposed', plan_path = EXCLUDED.plan_path, reason = EXCLUDED.reason, files = NULL, head_sha = EXCLUDED.head_sha, kind = EXCLUDED.kind, planned_at = EXCLUDED.planned_at")
            .bind(root).bind(branch).bind(path).bind(reason).bind(head).bind(kind.as_str()).bind(thread).execute(conn).await?;
        Ok(())
    })).await? { return Err("plan proposal was refused".into()); }
    claim.commit().await?;
    Ok(())
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
    use super::thread_events::PlanScope;
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
    let event = ThreadEvent::PlanApproved {
        kind,
        plan_path: path.clone(),
        plan_sha: sha,
        scope: if plan_only {
            PlanScope::PlanOnly
        } else {
            PlanScope::Implement
        },
    };
    let branch = branch.to_string();
    let expected_path = path.clone();
    if !bus.emit_with_mutation(thread, event, move |conn| Box::pin(async move {
        let changed = sqlx::query("UPDATE planned_branches SET state = $3, planned_at = NOW() WHERE repo_root = $1 AND branch_name = $2 AND state = 'proposed' AND planned_at = $4 AND plan_path = $5 AND kind = $6")
            .bind(root).bind(branch).bind(if plan_only { "plan_only" } else { "planned" }).bind(marked_at).bind(expected_path).bind(kind.as_str()).execute(conn).await?;
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
            crate::engine::agent_session::parse_ask_user_question_inputs(&input).remove(0);
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
                unattested_act: None,
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
            sqlx::query("DELETE FROM planned_branches")
                .execute(&pool)
                .await
                .unwrap();
            let historical = ThreadEvent::PlanApproved {
                kind,
                plan_path: "docs/plans/plan.md".into(),
                plan_sha: plan_blob(repo.path(), "work", "docs/plans/plan.md")
                    .await
                    .unwrap(),
                scope: if scope == "implement" {
                    super::super::thread_events::PlanScope::Implement
                } else {
                    super::super::thread_events::PlanScope::PlanOnly
                },
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
