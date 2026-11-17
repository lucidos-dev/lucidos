use super::*;
use crate::core::oauth::AuthorizationGrant;

/// A state no other test opens, so parallel tests never share a slot.
fn unique_state(prefix: &str) -> String {
    format!(
        "{prefix}{STATE_PREFIX_SEPARATOR}{}",
        uuid::Uuid::new_v4().simple()
    )
}

fn browser(prefix: &str, local: bool, origin: Option<&str>) -> BrowserContext {
    BrowserContext {
        prefix: prefix.into(),
        local,
        origin: origin.map(Into::into),
    }
}

#[test]
fn without_a_gateway_the_engine_binds_the_port_itself() {
    assert_eq!(
        CallbackRoute::select(
            false,
            Some(browser("/dev/", true, None)),
            Some("dev".into())
        ),
        CallbackRoute::OwnListener
    );
    assert_eq!(CallbackRoute::OwnListener.state_for("n0nce"), "n0nce");
}

#[test]
fn under_a_gateway_the_state_names_the_browser_prefix() {
    let browser = browser("/dev@cloud/", true, None);
    let route = CallbackRoute::select(true, Some(browser), Some("dev".into()));
    assert_eq!(route.state_for("n0nce"), "/dev@cloud/~n0nce");
}

#[test]
fn an_unknown_browser_falls_back_to_a_local_one_on_the_engines_own_workspace() {
    let route = CallbackRoute::select(true, None, Some("dev".into()));
    assert_eq!(route.state_for("n0nce"), "/dev/~n0nce");
    assert_eq!(route, CallbackRoute::Gateway(browser("/dev/", true, None)));
}

#[test]
fn only_a_proxied_request_carries_a_browser_context() {
    let mut headers = HeaderMap::new();
    assert_eq!(BrowserContext::from_headers(&headers), None);
    headers.insert("x-forwarded-prefix", "/dev@cloud/".parse().unwrap());
    assert_eq!(
        BrowserContext::from_headers(&headers),
        Some(browser("/dev@cloud/", false, None))
    );
}

#[test]
fn a_browser_is_local_only_by_the_gateways_stamp() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-prefix", "/dev/".parse().unwrap());
    headers.insert(HEADER_LOCAL_BROWSER, "yes".parse().unwrap());
    assert!(!BrowserContext::from_headers(&headers).unwrap().local);
    headers.insert(HEADER_LOCAL_BROWSER, "1".parse().unwrap());
    assert!(BrowserContext::from_headers(&headers).unwrap().local);
}

#[test]
fn the_origin_is_read_as_scheme_host_and_port() {
    let read = |origin: &str| {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-prefix", "/dev/".parse().unwrap());
        headers.insert("origin", origin.parse().unwrap());
        BrowserContext::from_headers(&headers).unwrap().origin
    };
    assert_eq!(
        read("https://Laptop.Example.ts.net").as_deref(),
        Some("https://laptop.example.ts.net")
    );
    assert_eq!(
        read("https://host.example:8443").as_deref(),
        Some("https://host.example:8443")
    );
    assert_eq!(
        read("https://host.example:443").as_deref(),
        Some("https://host.example")
    );
    assert_eq!(read("null"), None);
    assert_eq!(read("file:///etc/passwd"), None);
}

#[test]
fn the_web_redirect_uri_sits_on_the_browsers_origin() {
    let remote = browser("/dev/", false, Some("https://host.example"));
    assert_eq!(
        remote.web_redirect_uri().as_deref(),
        Some("https://host.example/~/oauth/callback")
    );
    assert_eq!(browser("/dev/", false, None).web_redirect_uri(), None);
}

#[test]
fn a_device_is_remembered_by_its_latest_prefix_and_last_origin() {
    let device = uuid::Uuid::new_v4().to_string();
    assert_eq!(browser_for_device(&device), None);
    note_browser(
        &device,
        browser("/dev/", false, Some("https://host.example")),
    );
    note_browser(&device, browser("/dev@cloud/", false, None));
    assert_eq!(
        browser_for_device(&device),
        Some(browser("/dev@cloud/", false, Some("https://host.example")))
    );
}

/// Deliver `query` while a stand-in flow takes the grant and settles the page
/// with `connected`. Returns the grant the flow saw, and the delivery.
async fn deliver_to_flow(
    pending: PendingCallback,
    query: &str,
    connected: bool,
) -> (AuthorizationGrant, CallbackDelivery) {
    let flow = tokio::spawn(async move {
        let received = pending.received().await.unwrap();
        received.reply.settle(connected);
        received.grant
    });
    let delivery = deliver_callback(query).await;
    (flow.await.unwrap(), delivery)
}

#[tokio::test]
async fn a_delivered_callback_reaches_its_flow_and_closes_the_slot() {
    let state = unique_state("/dev/");
    let pending = PendingCallback::open(&state, "testprov");
    let query = format!("code=the-code&state={}", urlencoding::encode(&state));

    let (grant, delivery) = deliver_to_flow(pending, &query, true).await;
    let CallbackDelivery::Accepted {
        state: page_state,
        page,
    } = delivery
    else {
        panic!("the open flow owns its state");
    };
    assert_eq!(page_state, CallbackPageState::Connected);
    assert!(page.contains("Connected"), "{page}");
    assert_eq!(grant.code, "the-code");

    assert!(
        matches!(
            deliver_callback(&query).await,
            CallbackDelivery::Unclaimed { .. }
        ),
        "a replay finds no slot"
    );
}

#[tokio::test]
async fn a_flow_that_fails_after_the_callback_gets_the_failure_page() {
    let state = unique_state("/dev/");
    let pending = PendingCallback::open(&state, "testprov");
    let query = format!("code=the-code&state={}", urlencoding::encode(&state));

    let (_, delivery) = deliver_to_flow(pending, &query, false).await;
    let CallbackDelivery::Accepted { state, page } = delivery else {
        panic!("the open flow owns its state");
    };
    assert_eq!(state, CallbackPageState::Failed);
    assert!(page.contains("Connection failed"), "{page}");
    assert!(!page.contains("Connected"), "{page}");
}

#[tokio::test(start_paused = true)]
async fn a_flow_that_has_not_settled_in_time_leaves_the_page_pending() {
    let state = unique_state("/dev/");
    let pending = PendingCallback::open(&state, "testprov");
    let query = format!("code=the-code&state={}", urlencoding::encode(&state));
    // The flow holds the reply and never settles it.
    let flow = tokio::spawn(async move {
        let received = pending.received().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        drop(received);
    });

    let CallbackDelivery::Accepted { state, page } = deliver_callback(&query).await else {
        panic!("the open flow owns its state");
    };
    assert_eq!(state, CallbackPageState::Pending);
    assert!(page.contains("Authorization received"), "{page}");
    flow.abort();
}

#[tokio::test]
async fn a_flow_that_ends_without_settling_leaves_the_page_pending() {
    let state = unique_state("/dev/");
    let pending = PendingCallback::open(&state, "testprov");
    let query = format!("code=the-code&state={}", urlencoding::encode(&state));
    let flow = tokio::spawn(async move { drop(pending.received().await) });

    let CallbackDelivery::Accepted { state, .. } = deliver_callback(&query).await else {
        panic!("the open flow owns its state");
    };
    assert_eq!(state, CallbackPageState::Pending);
    flow.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_callback_for_a_flow_that_already_ended_fails_at_once() {
    let state = unique_state("/dev/");
    // The flow's receiver is gone while its slot is still open: the race
    // between the delivery taking the slot and the flow giving up.
    let (deliver, received) = tokio::sync::oneshot::channel();
    drop(received);
    pending().insert(
        state.clone(),
        PendingEntry {
            provider: "testprov".to_string(),
            deliver,
        },
    );
    let query = format!("code=the-code&state={}", urlencoding::encode(&state));

    let started = tokio::time::Instant::now();
    let CallbackDelivery::Accepted { state, .. } = deliver_callback(&query).await else {
        panic!("the open slot owns its state");
    };
    assert_eq!(state, CallbackPageState::Failed);
    assert_eq!(
        started.elapsed(),
        std::time::Duration::ZERO,
        "no flow can settle the page"
    );
}

#[test]
fn the_page_wait_stays_under_the_gateways_engine_timeout() {
    assert!(
        crate::core::oauth::PAGE_SETTLE_WAIT < lucidos_engine_socket::OAUTH_CALLBACK_HOP_TIMEOUT
    );
}

#[tokio::test]
async fn a_delivered_callback_hands_its_flow_the_issued_client_id() {
    let state = unique_state("/dev/");
    let pending = PendingCallback::open(&state, "ChatGPT");
    let query = format!(
        "code=the-code&state={}&client_id=app_issued",
        urlencoding::encode(&state)
    );
    let (grant, delivery) = deliver_to_flow(pending, &query, true).await;
    assert!(matches!(delivery, CallbackDelivery::Accepted { .. }));
    assert_eq!(
        grant,
        AuthorizationGrant {
            code: "the-code".into(),
            client_id: Some("app_issued".into()),
        }
    );
}

#[tokio::test]
async fn a_provider_denial_is_delivered_as_the_flows_error() {
    let state = unique_state("/dev/");
    let pending = PendingCallback::open(&state, "testprov");
    let query = format!("error=access_denied&state={}", urlencoding::encode(&state));
    assert!(matches!(
        deliver_callback(&query).await,
        CallbackDelivery::Accepted {
            state: CallbackPageState::Refused,
            ..
        }
    ));
    let error = pending.received().await.unwrap_err().to_string();
    assert!(error.contains("access_denied"), "{error}");
}

#[tokio::test]
async fn an_unknown_or_tampered_state_is_refused() {
    let state = unique_state("/dev/");
    let _pending = PendingCallback::open(&state, "testprov");
    let nonce = state.rsplit_once(STATE_PREFIX_SEPARATOR).unwrap().1;

    for forged in [
        format!("code=x&state=/other/~{nonce}"),
        format!("code=x&state={nonce}"),
        "code=x&state=/dev/~not-a-nonce-we-issued".to_string(),
        "code=x".to_string(),
    ] {
        assert!(
            matches!(
                deliver_callback(&forged).await,
                CallbackDelivery::Unclaimed { .. }
            ),
            "{forged} must not reach the flow"
        );
    }
    assert!(
        pending().contains_key(&state),
        "a refused callback leaves the real flow waiting"
    );
}

#[tokio::test]
async fn a_finished_flow_cannot_be_delivered_to() {
    let state = unique_state("/dev/");
    drop(PendingCallback::open(&state, "testprov"));
    let query = format!("code=x&state={}", urlencoding::encode(&state));
    assert!(matches!(
        deliver_callback(&query).await,
        CallbackDelivery::Unclaimed { .. }
    ));
}
