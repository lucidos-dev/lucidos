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

/// Runs the resample over scripted replies and counts the calls it made.
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

/// `[]` is the model's answer: nothing here is worth remembering. Asking
/// again only buys a different answer to the same question.
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

/// Three failures leave the content unread, which is the one case the
/// fallback fact exists for.
#[tokio::test(start_paused = true)]
async fn three_failed_calls_read_as_failed() {
    let (outcome, calls) = run(vec![
        || Err("503 Service Unavailable".into()),
        || Err("503 Service Unavailable".into()),
        || Err("503 Service Unavailable".into()),
    ])
    .await;
    assert!(matches!(outcome, Extraction::Failed), "got {outcome:?}");
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
