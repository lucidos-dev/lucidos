use super::*;
use crate::test_support::{aux_captures, setup_test_db, teardown_test_db, ScriptedProvider};

const MODEL: &str = "gemini-3-flash-preview";
const TWO_COMMITS: &str = "fix: settle the read\nfeat: add change summaries";

fn call(replies: Vec<&str>) -> SummaryCall {
    SummaryCall {
        provider: Arc::new(ScriptedProvider::new(MODEL, replies)),
        effort: Some("none".to_string()),
        deadline: Duration::from_secs(5),
    }
}

async fn summarized_events(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'ChangeSummarized'",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[test]
fn a_single_commit_change_needs_no_summary() {
    assert!(!needs_summary("feat: add change summaries", None));
    assert!(!needs_summary("feat: add change summaries\n\n", None));
    assert!(!needs_summary("", None));
}

#[test]
fn a_change_of_several_commits_needs_one_until_it_has_one() {
    assert!(needs_summary(TWO_COMMITS, None));
    assert!(!needs_summary(TWO_COMMITS, Some("Adds change summaries")));
}

#[test]
fn the_model_reads_the_commits_oldest_first() {
    assert_eq!(
        model_input("fix: settle the read\n\nfeat: add change summaries\n"),
        "feat: add change summaries\nfix: settle the read"
    );
}

/// On a long branch the oldest subjects name the main work, so the cap keeps
/// them and drops the newest, whole lines only.
#[test]
fn a_long_commit_list_keeps_its_oldest_subjects() {
    let newest_first: Vec<String> = (0..200)
        .rev()
        .map(|i| format!("commit {i:03} {}", "x".repeat(40)))
        .collect();
    let input = model_input(&newest_first.join("\n"));
    assert!(input.chars().count() <= MAX_INPUT_CHARS);
    assert!(input.starts_with("commit 000 "));
    assert!(!input.contains("commit 199 "));
    assert!(input.lines().all(|line| line.ends_with(&"x".repeat(40))));
}

#[test]
fn a_reply_is_cleaned_to_a_bare_line() {
    assert_eq!(
        validate_summary("  \"Adds change summaries to the change card.\"  ").unwrap(),
        "Adds change summaries to the change card"
    );
    assert_eq!(validate_summary("`Adds a thing`").unwrap(), "Adds a thing");
}

#[test]
fn a_reply_that_is_not_a_headline_is_refused() {
    assert!(validate_summary("").is_err());
    assert!(validate_summary("  \"\" ").is_err());
    assert!(validate_summary("Adds summaries.\nAlso fixes a read.").is_err());
    assert!(validate_summary(&"word ".repeat(30)).is_err());
}

#[tokio::test]
async fn a_summary_is_emitted_for_the_commit_list_it_read() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();

    let summary = write_summary(
        &bus,
        &call(vec!["Adds change summaries."]),
        thread_id,
        change_id,
        TWO_COMMITS,
    )
    .await
    .expect("a summary");
    assert_eq!(summary, "Adds change summaries");

    let events = summarized_events(&pool, thread_id).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["change_id"], change_id.to_string());
    assert_eq!(events[0]["summary"], "Adds change summaries");
    assert_eq!(events[0]["description"], TWO_COMMITS);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A resample is a second call that spent a second set of tokens, so both
/// attempts are recorded, the rejected one included.
#[tokio::test]
async fn every_attempt_records_its_cost() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    write_summary(
        &bus,
        &call(vec!["Line one.\nLine two.", "Adds change summaries"]),
        thread_id,
        Uuid::new_v4(),
        TWO_COMMITS,
    )
    .await
    .expect("the second attempt is a line");

    let captures = aux_captures(&pool, thread_id, "change_summary").await;
    assert_eq!(captures.len(), 2, "both attempts cost tokens: {captures:?}");
    for payload in &captures {
        assert_eq!(payload["producer"], "auxiliary");
        assert_eq!(payload["model"], MODEL);
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A failed summary costs the change nothing but its headline: no event, and
/// the spend still on record.
#[tokio::test]
async fn a_summary_that_never_validates_emits_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    assert!(write_summary(
        &bus,
        &call(vec!["", "\"\""]),
        thread_id,
        Uuid::new_v4(),
        TWO_COMMITS
    )
    .await
    .is_err());

    assert!(summarized_events(&pool, thread_id).await.is_empty());
    assert_eq!(
        aux_captures(&pool, thread_id, "change_summary").await.len(),
        2
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

fn change(
    description: &str,
    summary: Option<&str>,
    thread_id: Option<Uuid>,
) -> crate::core::changes::Change {
    crate::core::changes::Change {
        id: Uuid::new_v4(),
        request_id: Uuid::nil(),
        thread_id,
        branch_name: "b".to_string(),
        repo_root: "/repo".to_string(),
        description: description.to_string(),
        file_count: 1,
        files: vec!["a.rs".to_string()],
        requires_restart: false,
        state: crate::core::changes::ChangeStatusData::Discarded,
        created_at: chrono::Utc::now(),
        resolved_at: None,
        hardened: false,
        thread_title: None,
        commits: vec![],
        summary: summary.map(str::to_string),
        incomplete: false,
    }
}

/// A restart backfills what the panel lists without a summary: several
/// commits, no summary, and a thread to record the model call on.
#[test]
fn the_backfill_picks_only_changes_that_still_need_a_summary() {
    let thread = Uuid::new_v4();
    let wanted = change(TWO_COMMITS, None, Some(thread));
    let changes = [
        wanted.clone(),
        change(TWO_COMMITS, Some("Adds change summaries"), Some(thread)),
        change("feat: one commit", None, Some(thread)),
        change(TWO_COMMITS, None, None),
    ];
    assert_eq!(backfill_targets(changes.iter()), vec![(thread, wanted.id)]);
}
