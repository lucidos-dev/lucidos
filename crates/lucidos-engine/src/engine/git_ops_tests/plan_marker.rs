use super::common::make_test_repo;
use super::*;

#[test]
fn plan_marker_kind_round_trips_known_states() {
    assert_eq!(
        PlanMarkerKind::parse("planned").as_db(),
        PlanMarkerKind::Planned.as_db()
    );
    assert_eq!(
        PlanMarkerKind::parse("acknowledged_simple"),
        PlanMarkerKind::AcknowledgedSimple
    );
    assert_eq!(PlanMarkerKind::parse("proposed"), PlanMarkerKind::Proposed);
    assert_eq!(PlanMarkerKind::Proposed.as_db(), "proposed");
}

#[test]
fn plan_marker_kind_unknown_defaults_to_planned() {
    assert_eq!(PlanMarkerKind::parse("???"), PlanMarkerKind::Planned);
    assert_eq!(PlanMarkerKind::parse(""), PlanMarkerKind::Planned);
}

#[test]
fn every_state_but_proposed_satisfies_the_gate() {
    assert!(PlanMarkerKind::Planned.satisfies_gate());
    assert!(PlanMarkerKind::AcknowledgedSimple.satisfies_gate());
    assert!(PlanMarkerKind::BoundedSecurityFix.satisfies_gate());
    assert!(
        !PlanMarkerKind::Proposed.satisfies_gate(),
        "a proposed plan awaits approval and must NOT satisfy the gate"
    );
}

#[test]
fn only_a_nonempty_list_of_plan_files_lands_plan_files_only() {
    let plan = "docs/plans/2026-10-10-x.md".to_string();
    assert!(lands_plan_files_only(std::slice::from_ref(&plan)));
    assert!(!lands_plan_files_only(&[plan, "src/main.rs".to_string()]));
    assert!(!lands_plan_files_only(&[]), "no files is no evidence");
}

#[test]
fn only_unhardened_work_beyond_a_plan_needs_hardening() {
    let plan = vec!["docs/plans/2026-10-10-x.md".to_string()];
    let code = vec![plan[0].clone(), "src/main.rs".to_string()];
    assert!(!needs_hardening(false, &plan), "a plan has nothing to test");
    assert!(needs_hardening(false, &code));
    assert!(!needs_hardening(true, &code));
    assert!(
        needs_hardening(false, &[]),
        "an unknown or empty list is no evidence of a plan"
    );
}

fn satisfies(state: PlanMarkerState) -> bool {
    use crate::engine::change_ops::{ApplyKindContext, PlanFloor};
    matches!(
        PlanFloor::decide(&ApplyKindContext::default(), state),
        PlanFloor::Clear(_)
    )
}

#[test]
fn bounded_security_fix_is_its_own_state() {
    assert_eq!(
        PlanMarkerKind::parse("bounded_security_fix"),
        PlanMarkerKind::BoundedSecurityFix
    );
    assert_eq!(
        PlanMarkerKind::BoundedSecurityFix.as_db(),
        "bounded_security_fix"
    );
    assert_ne!(
        PlanMarkerKind::BoundedSecurityFix,
        PlanMarkerKind::AcknowledgedSimple
    );
    assert_ne!(PlanMarkerKind::BoundedSecurityFix, PlanMarkerKind::Planned);
    assert!(PlanMarkerKind::BoundedSecurityFix.is_file_bounded());
    for kind in [
        PlanMarkerKind::Planned,
        PlanMarkerKind::AcknowledgedSimple,
        PlanMarkerKind::Proposed,
    ] {
        assert!(!kind.is_file_bounded(), "{kind:?} carries no file bound");
    }
}

#[test]
fn validate_plan_files_refuses_a_bound_that_cannot_be_enforced() {
    let one = vec!["a.rs".to_string()];
    assert!(validate_plan_files(PlanMarkerKind::BoundedSecurityFix, &one).is_ok());

    assert_eq!(
        validate_plan_files(PlanMarkerKind::BoundedSecurityFix, &[]),
        Err(PlanMarkerRejection::BoundedFixNeedsFiles),
    );

    let too_many: Vec<String> = (0..=MAX_BOUNDED_SECURITY_FIX_FILES)
        .map(|i| format!("f{i}.rs"))
        .collect();
    assert_eq!(
        validate_plan_files(PlanMarkerKind::BoundedSecurityFix, &too_many),
        Err(PlanMarkerRejection::BoundedFixTooManyFiles(
            MAX_BOUNDED_SECURITY_FIX_FILES + 1
        )),
    );

    let at_cap: Vec<String> = (0..MAX_BOUNDED_SECURITY_FIX_FILES)
        .map(|i| format!("f{i}.rs"))
        .collect();
    assert!(validate_plan_files(PlanMarkerKind::BoundedSecurityFix, &at_cap).is_ok());

    for kind in [
        PlanMarkerKind::Planned,
        PlanMarkerKind::AcknowledgedSimple,
        PlanMarkerKind::Proposed,
    ] {
        assert!(validate_plan_files(kind, &[]).is_ok());
        assert_eq!(
            validate_plan_files(kind, &one),
            Err(PlanMarkerRejection::FilesOnlyForBoundedFix),
            "{kind:?} has no bound to enforce, so a list on it would be dead weight",
        );
    }
}

#[test]
fn parse_strict_rejects_what_parse_would_call_planned() {
    assert_eq!(
        PlanMarkerKind::parse_strict("bounded_security_fix"),
        Some(PlanMarkerKind::BoundedSecurityFix)
    );
    assert_eq!(
        PlanMarkerKind::parse_strict("planned"),
        Some(PlanMarkerKind::Planned)
    );
    assert_eq!(PlanMarkerKind::parse_strict("bounded-security-fix"), None);
    assert_eq!(
        PlanMarkerKind::parse("bounded-security-fix"),
        PlanMarkerKind::Planned,
        "the lenient reader still defaults, which is why writes are strict",
    );
    assert_eq!(PlanMarkerKind::parse_strict("???"), None);
    assert_eq!(PlanMarkerKind::parse_strict(""), None);
}

#[test]
fn validate_plan_files_trims_and_drops_empty_entries() {
    let raw = vec![
        "  crates/a.rs".to_string(),
        " crates/b.rs ".to_string(),
        "   ".to_string(),
    ];
    assert_eq!(
        validate_plan_files(PlanMarkerKind::BoundedSecurityFix, &raw),
        Ok(vec!["crates/a.rs".to_string(), "crates/b.rs".to_string()]),
    );
    assert_eq!(
        validate_plan_files(PlanMarkerKind::BoundedSecurityFix, &["  ".to_string()]),
        Err(PlanMarkerRejection::BoundedFixNeedsFiles),
    );
    assert_eq!(
        validate_plan_files(PlanMarkerKind::Planned, &["".to_string()]),
        Ok(Vec::new()),
    );
}

#[test]
fn bounded_fix_violations_names_only_what_left_the_bound() {
    let bound = vec![
        "crates/lucidos-agents-api/src/api/proxy.rs".to_string(),
        "crates/lucidos-agents-api/src/api/proxy_tests.rs".to_string(),
    ];
    assert!(bounded_fix_violations(&bound, &bound).is_empty());
    assert!(bounded_fix_violations(&bound, &[]).is_empty());

    let strayed = vec![
        "crates/lucidos-agents-api/src/api/proxy.rs".to_string(),
        "crates/lucidos-app/src/main.tsx".to_string(),
    ];
    assert_eq!(
        bounded_fix_violations(&bound, &strayed),
        vec!["crates/lucidos-app/src/main.tsx".to_string()],
    );

    let with_plan = vec![
        "crates/lucidos-agents-api/src/api/proxy.rs".to_string(),
        "docs/plans/2026-08-29-wider.md".to_string(),
    ];
    assert!(bounded_fix_violations(&bound, &with_plan).is_empty());

    assert_eq!(
        bounded_fix_violations(&[], &strayed).len(),
        2,
        "an empty bound must refuse, never wave the branch through",
    );
}

fn bound_of(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

#[test]
fn bounded_fix_refusal_counts_uncommitted_files_and_fails_closed() {
    let bound = bound_of(&["a.rs", "a_tests.rs"]);

    assert_eq!(
        bounded_fix_refusal_for(BoundedFixInputs {
            bound: Ok(bound.clone()),
            committed: Ok(bound_of(&["a.rs"])),
            dirty: Ok(bound_of(&["a_tests.rs"])),
        }),
        None,
    );

    let refusal = bounded_fix_refusal_for(BoundedFixInputs {
        bound: Ok(bound.clone()),
        committed: Ok(bound_of(&["a.rs", "elsewhere.rs"])),
        dirty: Ok(Vec::new()),
    })
    .expect("a committed file outside the bound must refuse");
    assert!(refusal.contains("elsewhere.rs"), "{refusal}");

    let refusal = bounded_fix_refusal_for(BoundedFixInputs {
        bound: Ok(bound.clone()),
        committed: Ok(bound_of(&["a.rs"])),
        dirty: Ok(bound_of(&["elsewhere.rs"])),
    })
    .expect("a dirty file outside the bound lands on main, so it must refuse");
    assert!(refusal.contains("elsewhere.rs"), "{refusal}");

    let refusal = bounded_fix_refusal_for(BoundedFixInputs {
        bound: Ok(bound.clone()),
        committed: Ok(Vec::new()),
        dirty: Ok(bound_of(&["elsewhere.rs"])),
    })
    .expect("no commits is not the same as nothing landing");
    assert!(refusal.contains("elsewhere.rs"), "{refusal}");

    let engine_fault = bounded_fix_refusal_for(BoundedFixInputs {
        bound: Err("reading the recorded bound failed: pool timeout".into()),
        committed: Ok(bound_of(&["a.rs"])),
        dirty: Ok(Vec::new()),
    })
    .expect("an unreadable bound must refuse");
    assert!(
        engine_fault.contains("engine-side failure"),
        "an unreadable bound is not the agent's mistake, so it must not read as one: \
         {engine_fault}",
    );
    assert!(
        !engine_fault.contains("drop the extra files"),
        "and it must not tell the agent to widen a bound it kept: {engine_fault}",
    );

    for inputs in [
        BoundedFixInputs {
            bound: Ok(bound.clone()),
            committed: Err("git diff timed out".into()),
            dirty: Ok(Vec::new()),
        },
        BoundedFixInputs {
            bound: Ok(bound.clone()),
            committed: Ok(bound_of(&["a.rs"])),
            dirty: Err("git status timed out".into()),
        },
    ] {
        assert!(
            bounded_fix_refusal_for(inputs).is_some(),
            "a git call that cannot answer leaves the bound unchecked, so it must refuse",
        );
    }
}

#[tokio::test]
async fn plan_marker_missing_when_no_db_row() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "any-branch").await,
        PlanMarkerState::Missing,
        "No DB row should report Missing"
    );
    assert!(!satisfies(
        plan_marker_state(&pool, &repo_path, "any-branch").await
    ));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn plan_marker_present_after_record_planned() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("a.txt"), "a")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "feature work"], &repo_path).await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Planned,
        Some("docs/plans/2026-06-18-x.md"),
        None,
        &[],
        &head_sha,
    )
    .await
    .unwrap();

    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Planned),
    );
    assert!(satisfies(
        plan_marker_state(&pool, &repo_path, "feature").await
    ));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn plan_marker_present_after_acknowledged_simple() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::AcknowledgedSimple,
        None,
        Some("one-line typo fix"),
        &[],
        &head_sha,
    )
    .await
    .unwrap();

    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::AcknowledgedSimple),
        "an acknowledged-simple marker must satisfy the gate just like a full plan",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn plan_marker_stays_present_after_new_commit() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("a.txt"), "a")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "plan-time commit"], &repo_path).await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Planned,
        Some("docs/plans/x.md"),
        None,
        &[],
        &head_sha,
    )
    .await
    .unwrap();

    tokio::fs::write(repo_path.join("b.txt"), "b")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "later work"], &repo_path).await;

    assert!(
        satisfies(plan_marker_state(&pool, &repo_path, "feature").await),
        "a follow-up commit must not invalidate a settled plan decision",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn plan_marker_consumed_returns_to_missing() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Planned,
        Some("docs/plans/x.md"),
        None,
        &[],
        &head_sha,
    )
    .await
    .unwrap();
    assert!(satisfies(
        plan_marker_state(&pool, &repo_path, "feature").await
    ));

    consume_plan_marker(&pool, &repo_path, "feature").await;
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Missing,
        "consume must delete the row so a later branch reuse starts fresh",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn re_marking_an_approved_plan_proposes_it_again() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();
    let mark = |path: &'static str| {
        let (pool, repo_path, head_sha) = (pool.clone(), repo_path.clone(), head_sha.clone());
        async move {
            record_planned(
                &pool,
                &repo_path,
                "feature",
                PlanMarkerKind::Proposed,
                Some(path),
                None,
                &[],
                &head_sha,
            )
            .await
            .unwrap();
        }
    };
    mark("docs/plans/first.md").await;
    approve_plan(&pool, &repo_path, "feature", None)
        .await
        .unwrap();
    assert!(satisfies(
        plan_marker_state(&pool, &repo_path, "feature").await
    ));

    mark("docs/plans/second.md").await;
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Proposed),
        "an approved marker never survives a re-mark"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn consuming_leaves_a_proposed_marker_for_the_approval() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Proposed,
        Some("docs/plans/x.md"),
        None,
        &[],
        "abc",
    )
    .await
    .unwrap();
    consume_plan_marker(&pool, &repo_path, "feature").await;
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Proposed),
    );
    assert_eq!(
        approve_plan(&pool, &repo_path, "feature", None)
            .await
            .unwrap(),
        Some(ApprovedPlan {
            plan_only: false,
            plan_path: Some("docs/plans/x.md".into())
        }),
        "the approval hands back the file the CLI marks approved",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn plan_marker_record_upserts_state() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::AcknowledgedSimple,
        None,
        Some("started small"),
        &[],
        &head_sha,
    )
    .await
    .unwrap();
    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Planned,
        Some("docs/plans/grew.md"),
        None,
        &[],
        &head_sha,
    )
    .await
    .unwrap();

    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Planned),
        "second mark must upsert, not duplicate or error on the PK",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn proposed_plan_does_not_satisfy_gate_until_approved() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Proposed,
        Some("docs/plans/2026-06-19-x.md"),
        None,
        &[],
        &head_sha,
    )
    .await
    .unwrap();

    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Proposed),
    );
    assert!(
        !satisfies(plan_marker_state(&pool, &repo_path, "feature").await),
        "a proposed (unapproved) plan must NOT satisfy the gate",
    );

    let approved = approve_plan(&pool, &repo_path, "feature", None)
        .await
        .unwrap()
        .is_some();
    assert!(approved, "approving a proposed plan must report a flip");
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Planned),
    );
    assert!(satisfies(
        plan_marker_state(&pool, &repo_path, "feature").await
    ));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn approve_plan_is_targeted_and_preserves_plan_path() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    assert!(approve_plan(&pool, &repo_path, "feature", None)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Missing,
    );

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::AcknowledgedSimple,
        None,
        Some("typo"),
        &[],
        &head_sha,
    )
    .await
    .unwrap();
    assert!(
        approve_plan(&pool, &repo_path, "feature", None)
            .await
            .unwrap()
            .is_none(),
        "approve must only flip a proposed row, never a simple ack",
    );
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::AcknowledgedSimple),
    );

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Proposed,
        Some("docs/plans/keep-me.md"),
        None,
        &[],
        &head_sha,
    )
    .await
    .unwrap();
    assert!(approve_plan(&pool, &repo_path, "feature", None)
        .await
        .unwrap()
        .is_some());
    let stored_path: Option<String> =
        sqlx::query_scalar("SELECT plan_path FROM planned_branches WHERE branch_name = $1")
            .bind("feature")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored_path.as_deref(),
        Some("docs/plans/keep-me.md"),
        "approve must preserve plan_path when flipping proposed -> planned",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn bounded_security_fix_stores_and_returns_its_bound() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    let bound = vec![
        "crates/lucidos-agents-api/src/api/proxy.rs".to_string(),
        "crates/lucidos-agents-api/src/api/proxy_tests.rs".to_string(),
    ];
    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::BoundedSecurityFix,
        None,
        Some("unscoped local proxy key; proxy_tests::local_key_refuses_foreign_host"),
        &bound,
        &head_sha,
    )
    .await
    .unwrap();

    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::BoundedSecurityFix),
    );
    assert_eq!(
        plan_marker_snapshot(&pool, &repo_path, "feature").await,
        PlanMarkerSnapshot {
            state: PlanMarkerState::Present(PlanMarkerKind::BoundedSecurityFix),
            file_bound: Some(bound.clone()),
        },
    );
    assert!(
        satisfies(plan_marker_state(&pool, &repo_path, "feature").await),
        "the lane exists so an unattended run can edit; it must satisfy the gate",
    );
    assert_eq!(
        plan_marker_files(&pool, &repo_path, "feature").await,
        Ok(bound.clone())
    );

    let wider = vec![
        "crates/lucidos-agents-api/src/api/proxy.rs".to_string(),
        "crates/lucidos-agents-api/src/api/proxy_tests.rs".to_string(),
        "crates/lucidos-agents-api/src/api/proxy_builtin.rs".to_string(),
    ];
    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::BoundedSecurityFix,
        None,
        Some("also the builtin arm"),
        &wider,
        &head_sha,
    )
    .await
    .unwrap();
    assert_eq!(
        plan_marker_files(&pool, &repo_path, "feature").await,
        Ok(wider.clone())
    );

    record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::Proposed,
        Some("docs/plans/too-wide.md"),
        None,
        &[],
        &head_sha,
    )
    .await
    .unwrap();
    assert_eq!(
        plan_marker_files(&pool, &repo_path, "feature").await,
        Ok(Vec::new()),
        "a non-bounded state must carry no file bound",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn record_planned_refuses_an_unenforceable_bound() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    let err = record_planned(
        &pool,
        &repo_path,
        "feature",
        PlanMarkerKind::BoundedSecurityFix,
        None,
        Some("no bound"),
        &[],
        &head_sha,
    )
    .await
    .expect_err("a bounded fix with no files must be refused");
    assert!(
        err.to_string().contains("--files"),
        "the refusal must tell the agent how to fix the call: {err}",
    );
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Missing,
        "a refused mark must leave no row behind",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
