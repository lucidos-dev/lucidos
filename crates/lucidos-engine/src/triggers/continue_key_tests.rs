use std::time::Duration;

use serde_json::json;

use super::*;

fn spec(value: Value) -> ContinueKeySpec {
    ContinueKeySpec::parse(Some(&value))
        .expect("valid spec")
        .expect("non-empty spec")
}

#[test]
fn parse_accepts_paths_and_alternatives() {
    let parsed = spec(json!([
        "repository.full_name",
        ["issue.number", "pull_request.number"]
    ]));
    assert_eq!(
        parsed.0,
        vec![
            ContinueKeyPart::Path("repository.full_name".into()),
            ContinueKeyPart::FirstOf(vec!["issue.number".into(), "pull_request.number".into()]),
        ]
    );
}

#[test]
fn parse_reads_null_and_empty_as_no_spec() {
    assert_eq!(ContinueKeySpec::parse(None), Ok(None));
    assert_eq!(ContinueKeySpec::parse(Some(&Value::Null)), Ok(None));
    assert_eq!(ContinueKeySpec::parse(Some(&json!([]))), Ok(None));
}

#[test]
fn parse_refuses_malformed_specs() {
    for bad in [
        json!("issue.number"),
        json!([""]),
        json!([[]]),
        json!([42]),
        json!([["a", 1]]),
        json!(["a", "b", "c", "d", "e", "f", "g", "h", "i"]),
    ] {
        assert!(
            ContinueKeySpec::parse(Some(&bad)).is_err(),
            "{bad} must be refused"
        );
    }
}

#[test]
fn the_toml_and_json_forms_round_trip() {
    let parsed = spec(json!(["channel", ["thread_ts", "ts"]]));
    let back = serde_json::to_value(&parsed).unwrap();
    assert_eq!(back, json!(["channel", ["thread_ts", "ts"]]));
}

#[test]
fn key_takes_the_first_path_with_a_value_per_part() {
    let parsed = spec(json!([
        "repository.full_name",
        ["issue.number", "pull_request.number"]
    ]));
    let issue =
        json!({"repository": {"full_name": "example-org/example-repo"}, "issue": {"number": 42}});
    let pull = json!({"repository": {"full_name": "example-org/example-repo"}, "pull_request": {"number": 42}});
    assert_eq!(
        parsed.key_for(&issue).as_deref(),
        Some(r#"["example-org/example-repo","42"]"#)
    );
    assert_eq!(parsed.key_for(&issue), parsed.key_for(&pull));
}

#[test]
fn key_skips_null_blank_and_structured_values() {
    let parsed = spec(json!([["thread_ts", "ts"]]));
    for payload in [
        json!({"thread_ts": null, "ts": "1.2"}),
        json!({"thread_ts": " ", "ts": "1.2"}),
        json!({"thread_ts": {"x": 1}, "ts": "1.2"}),
    ] {
        assert_eq!(parsed.key_for(&payload).as_deref(), Some(r#"["1.2"]"#));
    }
}

#[test]
fn a_part_without_a_value_means_no_key() {
    let parsed = spec(json!(["channel", "ts"]));
    assert_eq!(parsed.key_for(&json!({"channel": "C1"})), None);
}

#[test]
fn parts_cannot_collide_by_concatenation() {
    let parsed = spec(json!(["a", "b"]));
    assert_ne!(
        parsed.key_for(&json!({"a": "x:y", "b": "z"})),
        parsed.key_for(&json!({"a": "x", "b": "y:z"}))
    );
}

#[tokio::test]
async fn a_claim_excludes_a_second_claim_on_the_same_key_only() {
    let key = format!("claim-test-{}", Uuid::new_v4());
    let held = claim(key.clone()).await;
    let other =
        tokio::time::timeout(Duration::from_millis(50), claim(format!("{key}-other"))).await;
    assert!(other.is_ok(), "a different key is never blocked");
    let same = tokio::time::timeout(Duration::from_millis(50), claim(key.clone())).await;
    assert!(same.is_err(), "the same key waits while it is held");
    drop(held);
    let again = tokio::time::timeout(Duration::from_millis(500), claim(key)).await;
    assert!(again.is_ok(), "releasing the claim frees the key");
}

mod routing {
    use super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent, TriggerInvocation};
    use crate::test_support::{setup_test_db, teardown_test_db};

    fn trigger_started(continue_key: Option<&str>) -> ThreadEvent {
        ThreadEvent::TriggerStarted {
            retry_of: None,
            trigger_id: "t-keyed".into(),
            trigger_name: Some("Ticket intake".into()),
            prompt: Some("Handle the ticket".into()),
            invocation: Some(TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
            provider: None,
            continue_key: continue_key.map(str::to_string),
        }
    }

    async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta {
                channel: Some(EventChannel::Trigger),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    /// Where a fire landed: `None` for a new thread, else the continued one.
    /// It does what a real fire does up to releasing its claim.
    async fn fire(pool: &sqlx::PgPool, bus: &EventBus, payload: &Value) -> Option<Uuid> {
        let spec = spec(json!(["ticket.id"]));
        let target = resolve_fire_target(pool, Some(&spec), Some(payload))
            .await
            .unwrap();
        match target {
            FireTarget::NewThread(claim) => {
                let claim = claim.expect("the payload yields a key");
                // Widen the race window: the claim must hold across real work.
                tokio::time::sleep(Duration::from_millis(30)).await;
                emit(bus, Uuid::new_v4(), trigger_started(Some(&claim.key))).await;
                None
            }
            FireTarget::Continue(thread_id) => Some(thread_id),
        }
    }

    async fn threads_holding(pool: &sqlx::PgPool, key: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM thread_summaries WHERE continue_key = $1")
            .bind(key)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_second_fire_with_the_key_continues_the_first_thread() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let payload = json!({"ticket": {"id": "T-1"}});

        assert_eq!(fire(&pool, &bus, &payload).await, None);
        let first: Uuid = sqlx::query_scalar(
            "SELECT thread_id FROM thread_summaries WHERE continue_key = '[\"T-1\"]'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(fire(&pool, &bus, &payload).await, Some(first));
        // A continuing fire's own TriggerStarted carries no key, and keeps it.
        emit(&bus, first, trigger_started(None)).await;
        assert_eq!(threads_holding(&pool, r#"["T-1"]"#).await, 1);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn two_racing_fires_start_one_thread() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let payload = json!({"ticket": {"id": "T-race"}});

        let (a, b) = tokio::join!(fire(&pool, &bus, &payload), fire(&pool, &bus, &payload));
        let started = [a, b].iter().filter(|t| t.is_none()).count();
        assert_eq!(started, 1, "exactly one fire starts the thread");
        assert_eq!(threads_holding(&pool, r#"["T-race"]"#).await, 1);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn archiving_releases_the_key() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let payload = json!({"ticket": {"id": "T-2"}});

        fire(&pool, &bus, &payload).await;
        let first: Uuid = sqlx::query_scalar(
            "SELECT thread_id FROM thread_summaries WHERE continue_key = '[\"T-2\"]'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        emit(&bus, first, ThreadEvent::ThreadArchived).await;
        assert_eq!(threads_holding(&pool, r#"["T-2"]"#).await, 0);
        assert_eq!(fire(&pool, &bus, &payload).await, None);
        assert_eq!(threads_holding(&pool, r#"["T-2"]"#).await, 1);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    /// An unattended run lands in Archive when it finishes, with no
    /// `ThreadArchived`. Its work item is still open, so the key stays.
    #[tokio::test]
    async fn a_finished_unattended_run_keeps_the_key() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let payload = json!({"ticket": {"id": "T-3"}});

        fire(&pool, &bus, &payload).await;
        let first: Uuid = sqlx::query_scalar(
            "SELECT thread_id FROM thread_summaries WHERE continue_key = '[\"T-3\"]'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let reply = ThreadEvent::ResponseGenerated {
            text: "Done.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        };
        emit(&bus, first, reply).await;
        let section: String =
            sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
                .bind(first)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(section, "archived", "the unattended run lands in Archive");
        assert_eq!(fire(&pool, &bus, &payload).await, Some(first));

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_index_refuses_a_second_open_thread_with_the_key() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        emit(&bus, Uuid::new_v4(), trigger_started(Some("dup"))).await;
        let second = bus
            .emit(BusEvent::Thread {
                thread_id: Uuid::new_v4(),
                event: trigger_started(Some("dup")),
                meta: EventMeta::NONE,
            })
            .await;
        assert!(
            second.is_err(),
            "the projection must refuse a duplicate key"
        );
        assert_eq!(threads_holding(&pool, "dup").await, 1);

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_payload_without_the_key_starts_an_unkeyed_thread() {
        let (pool, db) = setup_test_db().await;
        let spec = spec(json!(["ticket.id"]));
        let target = resolve_fire_target(&pool, Some(&spec), Some(&json!({})))
            .await
            .unwrap();
        assert!(matches!(target, FireTarget::NewThread(None)));
        let unkeyed = resolve_fire_target(&pool, None, Some(&json!({"ticket": {"id": "x"}})))
            .await
            .unwrap();
        assert!(matches!(unkeyed, FireTarget::NewThread(None)));

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
