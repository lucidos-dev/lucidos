use super::{resample_extraction, Extraction};
use crate::memory::ExtractedFact;
use std::sync::atomic::{AtomicU32, Ordering};

type Reply = Result<Vec<ExtractedFact>, Box<dyn std::error::Error + Send + Sync>>;

fn fact() -> ExtractedFact {
    ExtractedFact {
        fact: "Prefers integration tests over mocks".into(),
        importance: 0.6,
        topic: "Testing".into(),
        entities: vec![],
    }
}

async fn run(replies: Vec<fn() -> Reply>) -> (Extraction, u32) {
    let calls = AtomicU32::new(0);
    let outcome = resample_extraction(false, || {
        let n = calls.fetch_add(1, Ordering::SeqCst) as usize;
        let reply = replies[n]();
        async move { reply }
    })
    .await;
    (outcome, calls.load(Ordering::SeqCst))
}

#[tokio::test(start_paused = true)]
async fn an_empty_reply_is_final_after_one_call() {
    let (outcome, calls) = run(vec![|| Ok(vec![])]).await;
    assert!(matches!(outcome, Extraction::Nothing), "got {outcome:?}");
    assert_eq!(calls, 1);
}

#[tokio::test(start_paused = true)]
async fn a_failed_call_is_retried_until_facts_arrive() {
    let (outcome, calls) = run(vec![
        || Err("503 Service Unavailable".into()),
        || Err("Failed to parse extraction JSON".into()),
        || Ok(vec![fact()]),
    ])
    .await;
    assert!(
        matches!(outcome, Extraction::Facts(ref f) if f.len() == 1),
        "got {outcome:?}"
    );
    assert_eq!(calls, 3);
}

#[tokio::test(start_paused = true)]
async fn three_failed_calls_read_as_failed() {
    let (outcome, calls) = run(vec![
        || Err("503 Service Unavailable".into()),
        || Err("503 Service Unavailable".into()),
        || Err("503 Service Unavailable".into()),
    ])
    .await;
    assert!(
        matches!(outcome, Extraction::Failed("call_failed")),
        "got {outcome:?}"
    );
    assert_eq!(calls, 3);
}

#[tokio::test(start_paused = true)]
async fn an_empty_reply_after_a_failure_is_still_final() {
    let (outcome, calls) = run(vec![|| Err("503 Service Unavailable".into()), || {
        Ok(vec![])
    }])
    .await;
    assert!(matches!(outcome, Extraction::Nothing), "got {outcome:?}");
    assert_eq!(calls, 2);
}

#[tokio::test]
async fn given_up_extraction_records_one_failure_per_source() {
    use crate::engine::aux_purpose::AuxCall;
    use crate::engine::event_bus::EventBus;
    use crate::engine::{AuxCapture, ContextPurpose};
    use crate::memory::MemorySource;
    use crate::test_support::{setup_test_db, teardown_test_db, ScriptedProvider};
    use std::{sync::Arc, time::Duration};
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let source = MemorySource::Event {
        id: uuid::Uuid::new_v4(),
    };
    let provider = Arc::new(ScriptedProvider::new(
        "claude-haiku-5-5",
        vec!["prose", "prose", "prose"],
    ));
    let call = AuxCall::over(provider.clone(), ContextPurpose::Memory);
    let capture = AuxCapture::discarding(ContextPurpose::Memory);
    let outcome = super::report_extraction(
        &bus,
        &source,
        "claude-haiku-5-5",
        Duration::from_secs(30),
        resample_extraction(false, || {
            crate::memory::extract_facts("Run the sandbox check.", None, None, &call, &capture)
        }),
    )
    .await;
    assert!(matches!(outcome, Extraction::Failed("unreadable_reply")));
    assert_eq!(provider.requests().len(), 3);
    let payloads: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE event_type = 'MemoryExtractionFailed'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(payloads.len(), 1);
    assert_eq!(
        payloads[0]["data"]["source"],
        serde_json::to_value(&source).unwrap()
    );
    assert_eq!(payloads[0]["data"]["model"], "claude-haiku-5-5");
    assert_eq!(payloads[0]["data"]["reason"], "unreadable_reply");

    super::report_extraction(
        &bus,
        &source,
        "test",
        Duration::from_secs(30),
        std::future::ready(Extraction::Failed("call_failed")),
    )
    .await;
    super::report_extraction(
        &bus,
        &source,
        "test",
        Duration::ZERO,
        std::future::pending(),
    )
    .await;
    let reasons: Vec<String> = sqlx::query_scalar(
        "SELECT payload->'data'->>'reason' FROM events WHERE event_type = 'MemoryExtractionFailed' ORDER BY sequence"
    ).fetch_all(&pool).await.unwrap();
    assert_eq!(reasons, ["unreadable_reply", "call_failed", "timed_out"]);
    super::report_extraction(
        &bus,
        &source,
        "test",
        Duration::from_secs(30),
        std::future::ready(Extraction::Nothing),
    )
    .await;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE event_type = 'MemoryExtractionFailed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 3, "an empty answer emits no failure");
    pool.close().await;
    teardown_test_db(&db).await;
}
