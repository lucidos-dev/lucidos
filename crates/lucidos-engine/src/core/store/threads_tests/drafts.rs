//! What a thread holds unsent, as the `threads` tool, the CLI and scripts read
//! it: the `drafts` read, the reader fields on `list`, the draft filters, and
//! draft text in search.

use super::super::*;
use super::test_helpers::*;
use crate::core::store::EventStore;

const WS: &str = "myws";

/// Insert a thread holding `text` and `images` as its draft. `edited_minutes_ago`
/// stamps `compose_updated_at`; `None` leaves it unknown, as on a draft written
/// before the column existed.
async fn insert_draft_thread(
    pool: &PgPool,
    title: &str,
    text: &str,
    images: serde_json::Value,
    edited_minutes_ago: Option<i64>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries \
         (thread_id, title, source, message_count, last_activity, has_response, \
          compose_text, compose_images, compose_updated_at) \
         VALUES ($1, $2, 'chat', 1, NOW(), TRUE, $3, $4, \
                 CASE WHEN $5::bigint IS NULL THEN NULL \
                      ELSE NOW() - make_interval(mins => $5::int) END)",
    )
    .bind(id)
    .bind(title)
    .bind(text)
    .bind(images)
    .bind(edited_minutes_ago)
    .execute(pool)
    .await
    .expect("insert draft thread");
    id
}

fn no_images() -> serde_json::Value {
    serde_json::json!([])
}

fn filters(has_draft: Option<bool>, has_diff: Option<bool>) -> ThreadSummaryFilters<'static> {
    ThreadSummaryFilters {
        status: StatusFilter::Any,
        sources: None,
        parent: None,
        has_draft,
        has_diff,
        limit: 1000,
    }
}

#[test]
fn a_preview_cuts_on_characters_and_the_length_counts_them() {
    let draft = "é".repeat(PREVIEW_CHARS + 50);
    let preview = text_preview(&draft);
    assert_eq!(preview.chars().count(), PREVIEW_CHARS);
    assert_eq!(char_length(&draft), PREVIEW_CHARS + 50);
    assert!(
        draft.len() > char_length(&draft),
        "a byte count would differ"
    );

    let short = "Ask the plumber about Tuesday";
    assert_eq!(text_preview(short), short);
    assert_eq!(char_length(short), short.chars().count());
}

#[test]
fn a_thread_link_is_the_workspace_qualified_thread_target() {
    let id = Uuid::new_v4();
    assert_eq!(thread_link(WS, id), format!("thread:myws/{id}"));
}

#[test]
fn a_draft_is_any_text_or_any_image() {
    assert!(!has_draft("", &no_images()));
    assert!(!has_draft("", &serde_json::Value::Null));
    assert!(
        has_draft(" ", &no_images()),
        "whitespace is text the user typed"
    );
    assert!(has_draft("", &serde_json::json!(["a".repeat(64)])));
}

/// The read the agent was missing: one call names every draft and its owning
/// thread, and nothing without a draft rides along.
#[tokio::test]
async fn drafts_lists_every_draft_and_no_empty_one() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let empty = insert_draft_thread(&pool, "Empty", "", no_images(), Some(1)).await;
    let older = insert_draft_thread(&pool, "Older", "Book the vet", no_images(), Some(30)).await;
    let newer = insert_draft_thread(&pool, "Newer", "Reply to Sam", no_images(), Some(2)).await;
    let image_only = insert_draft_thread(
        &pool,
        "Image",
        "",
        serde_json::json!(["b".repeat(64)]),
        Some(10),
    )
    .await;
    let unknown = insert_draft_thread(&pool, "Unknown", "Old draft", no_images(), None).await;

    let drafts = store.list_drafts(WS, 50).await.expect("list drafts");
    let ids: Vec<String> = drafts.iter().map(|d| d.thread_id.clone()).collect();
    assert_eq!(
        ids,
        [newer, image_only, older, unknown].map(|id| id.to_string()),
        "newest edit first, an unknown edit time last, and no empty draft"
    );
    assert!(!ids.contains(&empty.to_string()));

    let first = &drafts[0];
    assert_eq!(first.title, "Newer");
    assert_eq!(first.preview, "Reply to Sam");
    assert_eq!(first.length, "Reply to Sam".chars().count());
    assert_eq!(first.link, format!("thread:myws/{newer}"));
    assert!(first.last_edited.is_some());
    assert!(first.text.is_none(), "the list carries previews only");
    assert_eq!(drafts[1].image_count, 1);
    assert!(drafts[3].last_edited.is_none());

    sqlx::query("UPDATE thread_summaries SET title = NULL WHERE thread_id = $1")
        .bind(newer)
        .execute(&pool)
        .await
        .expect("drop the title");
    let untitled = store
        .get_draft(WS, newer)
        .await
        .expect("get")
        .expect("a draft");
    assert_eq!(
        untitled.title, UNTITLED_THREAD,
        "a never-sent draft still has a name"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A draft longer than the preview is still readable whole, by thread id.
#[tokio::test]
async fn one_draft_by_id_carries_its_whole_text() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let long = "word ".repeat(100);
    let id = insert_draft_thread(&pool, "Long", &long, no_images(), Some(1)).await;
    let none = insert_draft_thread(&pool, "None", "", no_images(), Some(1)).await;

    let listed = store.list_drafts(WS, 50).await.expect("list");
    assert_eq!(listed[0].preview.chars().count(), PREVIEW_CHARS);
    assert_eq!(listed[0].length, long.chars().count());

    let one = store
        .get_draft(WS, id)
        .await
        .expect("get")
        .expect("a draft");
    assert_eq!(one.text.as_deref(), Some(long.as_str()));
    assert!(store.get_draft(WS, none).await.expect("get").is_none());
    assert!(store
        .get_draft(WS, Uuid::new_v4())
        .await
        .expect("get")
        .is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A workspace reads its own drafts, never another's. Each workspace is its
/// own database, which is what the two pools stand in for.
#[tokio::test]
async fn a_workspace_reads_only_its_own_drafts() {
    let (pool_a, db_a) = setup_test_db().await;
    let (pool_b, db_b) = setup_test_db().await;
    let a = insert_draft_thread(&pool_a, "A", "only in a", no_images(), Some(1)).await;

    let in_b = EventStore::new(pool_b.clone())
        .list_drafts("other", 50)
        .await;
    assert!(in_b.expect("list b").is_empty());
    let in_a = EventStore::new(pool_a.clone())
        .list_drafts(WS, 50)
        .await
        .expect("list a");
    assert_eq!(in_a.len(), 1);
    assert_eq!(in_a[0].link, format!("thread:myws/{a}"));

    pool_a.close().await;
    teardown_test_db(&db_a).await;
    pool_b.close().await;
    teardown_test_db(&db_b).await;
}

/// One `list` call maps every thread to its draft, and the draft filter keeps
/// the count in step with the rows.
#[tokio::test]
async fn list_rows_carry_the_draft_and_the_filter_selects_on_it() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let drafted =
        insert_draft_thread(&pool, "Drafted", "Call the bank", no_images(), Some(1)).await;
    let plain = insert_draft_thread(&pool, "Plain", "", no_images(), None).await;

    let mut rows = store
        .list_thread_summaries(filters(None, None))
        .await
        .expect("list");
    attach_reader_fields(&mut rows, WS);
    let row = |id: Uuid| rows.iter().find(|r| r.thread_id == id.to_string()).unwrap();
    assert_eq!(row(drafted).has_draft, Some(true));
    assert_eq!(row(drafted).draft_preview.as_deref(), Some("Call the bank"));
    assert_eq!(row(drafted).draft_length, Some(13));
    assert_eq!(row(plain).has_draft, Some(false));
    assert_eq!(row(plain).draft_preview, None);
    assert_eq!(row(plain).link, Some(format!("thread:myws/{plain}")));

    for (want, expected) in [(true, drafted), (false, plain)] {
        let rows = store
            .list_thread_summaries(filters(Some(want), None))
            .await
            .expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].thread_id, expected.to_string());
        let count = store
            .count_thread_summaries(filters(Some(want), None))
            .await
            .expect("count");
        assert_eq!(count, 1, "the count must agree with the rows");
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_diff_filter_selects_on_the_git_fact() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());
    let with_diff = insert_draft_thread(&pool, "Diff", "", no_images(), None).await;
    let _without = insert_draft_thread(&pool, "Clean", "", no_images(), None).await;
    sqlx::query("UPDATE thread_summaries SET coding_agent_has_diff = TRUE WHERE thread_id = $1")
        .bind(with_diff)
        .execute(&pool)
        .await
        .expect("mark diff");

    let rows = store
        .list_thread_summaries(filters(None, Some(true)))
        .await
        .expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].thread_id, with_diff.to_string());
    let rest = store
        .count_thread_summaries(filters(None, Some(false)))
        .await
        .expect("count");
    assert_eq!(rest, 1);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A phrase from a draft finds its thread, though no message ever said it.
#[tokio::test]
async fn search_finds_a_thread_by_its_draft() {
    let (pool, db) = setup_test_db().await;
    ensure_memory_entries_table(&pool).await;
    let store = EventStore::new(pool.clone());
    let id = insert_draft_thread(
        &pool,
        "Weekend",
        "Remind Sam about the cabin keys",
        no_images(),
        Some(1),
    )
    .await;
    let _other = insert_draft_thread(&pool, "Other", "Unrelated", no_images(), Some(1)).await;

    let hits = store
        .search_threads_by_text("cabin keys", 20)
        .await
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].info.thread_id, id.to_string());
    let miss = store
        .search_threads_by_text("cabin lighthouse", 20)
        .await
        .expect("search");
    assert!(miss.is_empty(), "every token must be in the draft");

    pool.close().await;
    teardown_test_db(&db).await;
}
