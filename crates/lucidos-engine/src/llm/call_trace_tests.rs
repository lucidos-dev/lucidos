use super::*;

#[tokio::test]
async fn a_write_outside_a_scope_is_a_no_op() {
    attempt_began();
    content_arrived();
    let ((), trace) = scoped(async {}).await;
    assert_eq!(trace.attempts, 0);
    assert!(trace.first_delta.is_none());
}

#[tokio::test]
async fn a_retry_reports_only_its_own_attempt() {
    let ((), trace) = scoped(async {
        attempt_began();
        response_received(529, Some("req_first"));
        content_arrived();
        attempt_began();
        response_received(200, None);
    })
    .await;
    assert_eq!(trace.attempts, 2);
    assert_eq!(trace.status, Some(200));
    assert_eq!(trace.request_id, None, "an absent header stays absent");
    assert!(
        trace.first_delta.is_none(),
        "the first attempt's delta is not the answering stream's"
    );
}

#[tokio::test]
async fn only_the_first_content_counts() {
    let pause = std::time::Duration::from_millis(5);
    let ((), trace) = scoped(async move {
        attempt_began();
        content_arrived();
        tokio::time::sleep(pause).await;
        content_arrived();
    })
    .await;
    let first = trace.first_delta.expect("a delta arrived");
    assert!(
        first.elapsed() >= pause,
        "the later delta did not replace the first"
    );
}

#[tokio::test]
async fn the_router_names_the_provider() {
    let ((), trace) = scoped(async { routed_to(ProviderKind::ClaudeGateway) }).await;
    assert_eq!(trace.provider, Some(ProviderKind::ClaudeGateway));
}

#[tokio::test]
async fn each_failure_class_reads_from_the_last_attempt() {
    let classify = |steps: fn(), timed_out: bool| async move {
        let ((), trace) = scoped(async move { steps() }).await;
        trace.failure(timed_out)
    };
    assert_eq!(
        classify(
            || {
                attempt_began();
                response_received(429, None);
            },
            false
        )
        .await,
        CallFailure::Http
    );
    assert_eq!(
        classify(
            || {
                attempt_began();
                transport_failed(CallFailure::Connection);
            },
            false
        )
        .await,
        CallFailure::Connection
    );
    assert_eq!(
        classify(
            || {
                attempt_began();
                response_received(200, None);
            },
            false
        )
        .await,
        CallFailure::Stream
    );
    assert_eq!(classify(|| {}, false).await, CallFailure::Other);
    assert_eq!(classify(|| {}, true).await, CallFailure::Timeout);
}

#[tokio::test]
async fn a_new_attempt_clears_the_transport_failure_before_it() {
    let ((), trace) = scoped(async {
        attempt_began();
        transport_failed(CallFailure::Connection);
        attempt_began();
        response_received(500, None);
    })
    .await;
    assert_eq!(trace.failure(false), CallFailure::Http);
}

#[test]
fn a_failure_serializes_as_its_snake_case_name() {
    assert_eq!(
        serde_json::to_string(&CallFailure::Connection).unwrap(),
        "\"connection\""
    );
}
