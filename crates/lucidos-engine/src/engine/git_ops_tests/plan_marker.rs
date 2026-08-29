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
    // An unknown/drifted value must not turn into a gate-blocking Missing, so it
    // parses as the settled (satisfying) Planned state. `proposed` is matched
    // explicitly, so a drift never masquerades as awaiting-approval.
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
    // State-level mirror.
    assert!(PlanMarkerState::Present(PlanMarkerKind::Planned).satisfies_gate());
    assert!(!PlanMarkerState::Present(PlanMarkerKind::Proposed).satisfies_gate());
    assert!(!PlanMarkerState::Missing.satisfies_gate());
}

/// The lane is a fourth value, not a reuse of a neighbour. Reusing
/// `acknowledged_simple` would claim a cross-module security fix is local.
/// Reusing `planned` would claim a human approved it. Both were available to
/// the sessions that deadlocked here, and both are lies.
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
    // It is the only kind carrying a bound for Apply to enforce.
    assert!(PlanMarkerKind::BoundedSecurityFix.is_file_bounded());
    for kind in [
        PlanMarkerKind::Planned,
        PlanMarkerKind::AcknowledgedSimple,
        PlanMarkerKind::Proposed,
    ] {
        assert!(!kind.is_file_bounded(), "{kind:?} carries no file bound");
    }
}

/// The bound is what makes the lane bounded. Three shapes are refused before
/// they reach the database: no list, a list over the cap, and a list on a kind
/// that cannot enforce one.
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

/// A write must not smuggle an unknown state past the gate. The lenient
/// `parse` is for reading a stored row, where drift should not nag. On a write
/// it would record a typo as `planned`, claiming a human approved.
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
    // The kebab-case spelling an agent reaches for, since CLAUDE.md makes it
    // the convention for public API values. Lenient parse calls it `planned`.
    assert_eq!(PlanMarkerKind::parse_strict("bounded-security-fix"), None);
    assert_eq!(
        PlanMarkerKind::parse("bounded-security-fix"),
        PlanMarkerKind::Planned,
        "the lenient reader still defaults, which is why writes are strict",
    );
    assert_eq!(PlanMarkerKind::parse_strict("???"), None);
    assert_eq!(PlanMarkerKind::parse_strict(""), None);
}

/// A comma-separated `--files` list arrives with the spaces still on it, and
/// the bound is later compared to git's output with `==`. An untrimmed entry
/// would match nothing and refuse the apply forever.
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
    // Whitespace-only entries are not a bound, so a list of them is no list.
    assert_eq!(
        validate_plan_files(PlanMarkerKind::BoundedSecurityFix, &["  ".to_string()]),
        Err(PlanMarkerRejection::BoundedFixNeedsFiles),
    );
    // And a blank entry beside an unbounded kind is still no bound at all.
    assert_eq!(
        validate_plan_files(PlanMarkerKind::Planned, &["".to_string()]),
        Ok(Vec::new()),
    );
}

/// The Apply floor's own predicate. An empty bound refuses everything, which is
/// how an unreadable file list fails closed.
#[test]
fn bounded_fix_violations_names_only_what_left_the_bound() {
    let bound = vec![
        "crates/lucidos-engine/src/api/proxy.rs".to_string(),
        "crates/lucidos-engine/src/api/proxy_tests.rs".to_string(),
    ];
    assert!(bounded_fix_violations(&bound, &bound).is_empty());
    assert!(bounded_fix_violations(&bound, &[]).is_empty());

    let strayed = vec![
        "crates/lucidos-engine/src/api/proxy.rs".to_string(),
        "crates/lucidos-app/src/main.tsx".to_string(),
    ];
    assert_eq!(
        bounded_fix_violations(&bound, &strayed),
        vec!["crates/lucidos-app/src/main.tsx".to_string()],
    );

    // A plan file is always in bounds: a session whose fix turned out wider
    // still has to write one, and that file is how it reports the decision.
    let with_plan = vec![
        "crates/lucidos-engine/src/api/proxy.rs".to_string(),
        "docs/plans/2026-08-29-wider.md".to_string(),
    ];
    assert!(bounded_fix_violations(&bound, &with_plan).is_empty());

    // No bound means nothing is in bounds. A row can only reach this state
    // through drift, since the schema and `validate_plan_files` both refuse an
    // empty bound, so refusing is the right answer.
    assert_eq!(
        bounded_fix_violations(&[], &strayed).len(),
        2,
        "an empty bound must refuse, never wave the branch through",
    );
}

fn bound_of(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

/// The Apply floor's decision, every arm.
///
/// The dirty-file arm is the one that matters most. Apply `git add -A`s the
/// worktree before merging. Reading only the committed diff therefore let an
/// out-of-bound edit reach main. It also let a branch with no commits pass on
/// an empty list.
#[test]
fn bounded_fix_refusal_counts_uncommitted_files_and_fails_closed() {
    let bound = bound_of(&["a.rs", "a_tests.rs"]);

    // In bounds, committed and dirty alike.
    assert_eq!(
        bounded_fix_refusal_for(BoundedFixInputs {
            bound: Ok(bound.clone()),
            committed: Ok(bound_of(&["a.rs"])),
            dirty: Ok(bound_of(&["a_tests.rs"])),
        }),
        None,
    );

    // Committed out of bounds: refused, and the message names the stray file.
    let refusal = bounded_fix_refusal_for(BoundedFixInputs {
        bound: Ok(bound.clone()),
        committed: Ok(bound_of(&["a.rs", "elsewhere.rs"])),
        dirty: Ok(Vec::new()),
    })
    .expect("a committed file outside the bound must refuse");
    assert!(refusal.contains("elsewhere.rs"), "{refusal}");

    // UNCOMMITTED out of bounds: refused too, because Apply will commit it.
    let refusal = bounded_fix_refusal_for(BoundedFixInputs {
        bound: Ok(bound.clone()),
        committed: Ok(bound_of(&["a.rs"])),
        dirty: Ok(bound_of(&["elsewhere.rs"])),
    })
    .expect("a dirty file outside the bound lands on main, so it must refuse");
    assert!(refusal.contains("elsewhere.rs"), "{refusal}");

    // A branch with nothing committed, and a dirty tree Apply would commit.
    let refusal = bounded_fix_refusal_for(BoundedFixInputs {
        bound: Ok(bound.clone()),
        committed: Ok(Vec::new()),
        dirty: Ok(bound_of(&["elsewhere.rs"])),
    })
    .expect("no commits is not the same as nothing landing");
    assert!(refusal.contains("elsewhere.rs"), "{refusal}");

    // Each unreadable input fails closed, and says whose fault it is.
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
    assert!(!plan_marker_state(&pool, &repo_path, "any-branch")
        .await
        .satisfies_gate());
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
    assert!(plan_marker_state(&pool, &repo_path, "feature")
        .await
        .satisfies_gate());
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

/// Unlike the harden marker, the plan gate is binary present/absent — a new
/// commit after the planning decision must NOT invalidate it.
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
        plan_marker_state(&pool, &repo_path, "feature")
            .await
            .satisfies_gate(),
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
    assert!(plan_marker_state(&pool, &repo_path, "feature")
        .await
        .satisfies_gate());

    consume_plan_marker(&pool, &repo_path, "feature").await;
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Missing,
        "consume must delete the row so a later branch reuse starts fresh",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The upsert path: re-marking the same branch flips the state (e.g. an
/// acknowledged-simple branch that later grows into a real plan).
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

/// The skill records `proposed`, which is present-but-not-satisfying: the gate
/// stays closed until the human approves and the agent flips it to `planned`.
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
        !plan_marker_state(&pool, &repo_path, "feature")
            .await
            .satisfies_gate(),
        "a proposed (unapproved) plan must NOT satisfy the gate",
    );

    // Approve flips proposed -> planned and now satisfies the gate.
    let approved = approve_plan(&pool, &repo_path, "feature").await.unwrap();
    assert!(approved, "approving a proposed plan must report a flip");
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::Planned),
    );
    assert!(plan_marker_state(&pool, &repo_path, "feature")
        .await
        .satisfies_gate());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Approve preserves `plan_path` and only ever touches a `proposed` row — it
/// never fabricates a marker on an unplanned branch nor clobbers a `--simple`
/// acknowledgment.
#[tokio::test]
async fn approve_plan_is_targeted_and_preserves_plan_path() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    // No row: approve is a no-op and must not fabricate a marker.
    assert!(!approve_plan(&pool, &repo_path, "feature").await.unwrap());
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Missing,
    );

    // acknowledged_simple: approve must not touch it.
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
        !approve_plan(&pool, &repo_path, "feature").await.unwrap(),
        "approve must only flip a proposed row, never a simple ack",
    );
    assert_eq!(
        plan_marker_state(&pool, &repo_path, "feature").await,
        PlanMarkerState::Present(PlanMarkerKind::AcknowledgedSimple),
    );

    // proposed with a plan_path: approve flips and keeps the path.
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
    assert!(approve_plan(&pool, &repo_path, "feature").await.unwrap());
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

/// The lane end to end at the storage layer: the bound survives the round trip,
/// it reads back only for the bounded kind, and a re-mark widens it.
#[tokio::test]
async fn bounded_security_fix_stores_and_returns_its_bound() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    let head_sha = current_head_sha(&repo_path).await.unwrap();

    let bound = vec![
        "crates/lucidos-engine/src/api/proxy.rs".to_string(),
        "crates/lucidos-engine/src/api/proxy_tests.rs".to_string(),
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
    assert!(
        plan_marker_state(&pool, &repo_path, "feature")
            .await
            .satisfies_gate(),
        "the lane exists so an unattended run can edit; it must satisfy the gate",
    );
    assert_eq!(
        plan_marker_files(&pool, &repo_path, "feature").await,
        Ok(bound.clone())
    );

    // A fix that has to grow re-marks with the full list rather than sneaking
    // past Apply, so the upsert must replace the bound.
    let wider = vec![
        "crates/lucidos-engine/src/api/proxy.rs".to_string(),
        "crates/lucidos-engine/src/api/proxy_tests.rs".to_string(),
        "crates/lucidos-engine/src/api/proxy_builtin.rs".to_string(),
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

    // Re-marking as an ordinary plan must clear the bound, or a later flip back
    // would inherit a stale one.
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

/// `record_planned` refuses the same shapes `validate_plan_files` does, so a
/// caller that skips the endpoint's check still cannot write an unenforceable
/// bound.
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
