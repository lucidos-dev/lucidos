use super::*;

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

#[tokio::test]
async fn a_delivered_callback_reaches_its_flow_and_closes_the_slot() {
    let state = unique_state("/dev/");
    let pending = PendingCallback::open(&state, "testprov");
    let query = format!("code=the-code&state={}", urlencoding::encode(&state));

    let CallbackDelivery::Accepted { ok, page } = deliver_callback(&query) else {
        panic!("the open flow owns its state");
    };
    assert!(ok);
    assert!(page.contains("Authorization complete"), "{page}");
    assert_eq!(pending.received().await.unwrap().code, "the-code");

    assert!(
        matches!(deliver_callback(&query), CallbackDelivery::Unclaimed { .. }),
        "a replay finds no slot"
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
    assert!(matches!(
        deliver_callback(&query),
        CallbackDelivery::Accepted { ok: true, .. }
    ));
    assert_eq!(
        pending.received().await.unwrap(),
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
        deliver_callback(&query),
        CallbackDelivery::Accepted { ok: false, .. }
    ));
    let error = pending.received().await.unwrap_err().to_string();
    assert!(error.contains("access_denied"), "{error}");
}

#[test]
fn an_unknown_or_tampered_state_is_refused() {
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
                deliver_callback(&forged),
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

#[test]
fn a_finished_flow_cannot_be_delivered_to() {
    let state = unique_state("/dev/");
    drop(PendingCallback::open(&state, "testprov"));
    let query = format!("code=x&state={}", urlencoding::encode(&state));
    assert!(matches!(
        deliver_callback(&query),
        CallbackDelivery::Unclaimed { .. }
    ));
}
