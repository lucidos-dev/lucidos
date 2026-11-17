use super::*;
use crate::auth;
use axum::body::{to_bytes, Body};
use serde_json::json;

fn staged(state: &GatewayState) -> PathBuf {
    let dir = state.release_check().local_channel_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Lucidos.tar.gz"), b"one bundle").unwrap();
    let feed = json!({"version":"99.0.0", "platforms":{"darwin-aarch64":{
        "url":"http://127.0.0.1:9/old/Lucidos.tar.gz", "signature":"signed"
    }}});
    std::fs::write(dir.join("latest.json"), feed.to_string()).unwrap();
    dir
}

fn request(path: &str, token: &str, ip: &str, port: u16) -> Request {
    let mut req = Request::builder()
        .uri(path)
        .header(auth::HEADER_LOCAL_TOKEN, token)
        .header(header::HOST, "untrusted.example")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(pairing::Peer {
        remote: format!("{ip}:4321").parse().unwrap(),
        local: format!("127.0.0.1:{port}").parse().unwrap(),
        public_listener: false,
    });
    req
}

#[tokio::test]
async fn local_routes_refuse_peers_tokens_and_unlisted_paths() {
    let temp = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(temp.path().into()));
    staged(&state);
    state
        .seed_device_for_test(auth::PairedDevice {
            id: "local-route-test".into(),
            label: "Test browser".into(),
            credential_digest: auth::digest("device-token"),
            paired_at: "2020-01-01T00:00:00Z".into(),
            last_seen_at: None,
        })
        .unwrap();
    let cookie_name = state.device_cookie_name().to_owned();
    let app = crate::server::gateway_router(state);
    for path in ["latest.json", "Lucidos.tar.gz"] {
        for (token, ip) in [
            ("", "127.0.0.1"),
            ("wrong", "127.0.0.1"),
            ("test-local-token", "192.0.2.1"),
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    &format!("/~/updates/local/{path}"),
                    token,
                    ip,
                    12345,
                ))
                .await
                .unwrap();
            assert!(matches!(
                response.status(),
                StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
            ));
        }
    }
    for path in ["secret", "other.tar.gz", "%2e%2e", "%2e%2e%2fsecret"] {
        let response = app
            .clone()
            .oneshot(request(
                &format!("/~/updates/local/{path}"),
                "test-local-token",
                "127.0.0.1",
                12345,
            ))
            .await
            .unwrap();
        assert!(!response.status().is_success(), "{path}");
    }
    let mut paired = request("/~/updates/local/latest.json", "", "127.0.0.1", 12345);
    paired.headers_mut().insert(
        header::COOKIE,
        format!("{cookie_name}=device-token").parse().unwrap(),
    );
    assert_eq!(
        app.clone().oneshot(paired).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut public = request(
        "/~/updates/local/latest.json",
        "test-local-token",
        "127.0.0.1",
        12345,
    );
    public
        .extensions_mut()
        .get_mut::<pairing::Peer>()
        .unwrap()
        .public_listener = true;
    assert!(!app
        .clone()
        .oneshot(public)
        .await
        .unwrap()
        .status()
        .is_success());
    let mut forwarded = request(
        "/~/updates/local/latest.json",
        "test-local-token",
        "127.0.0.1",
        12345,
    );
    forwarded
        .headers_mut()
        .insert("x-forwarded-for", "192.0.2.1".parse().unwrap());
    assert_eq!(
        app.oneshot(forwarded).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn local_feed_and_bundle_follow_each_routers_port() {
    let temp = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(temp.path().into()));
    staged(&state);
    // Two independently bound listeners over the same staged home.
    for _ in 0..2 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local = listener.local_addr().unwrap();
        let app = crate::server::gateway_router(state.clone()).layer(axum::middleware::from_fn(
            move |mut req: Request, next: axum::middleware::Next| async move {
                let remote = req
                    .extensions()
                    .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                    .unwrap()
                    .0;
                req.extensions_mut().insert(pairing::Peer {
                    remote,
                    local,
                    public_listener: false,
                });
                next.run(req).await
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let feed_body = client
            .get(format!("http://{local}/~/updates/local/latest.json"))
            .header(auth::HEADER_LOCAL_TOKEN, "test-local-token")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let feed: Value = serde_json::from_str(&feed_body).unwrap();
        let url = feed["platforms"]["darwin-aarch64"]["url"].as_str().unwrap();
        assert_eq!(
            url,
            format!("http://{local}/~/updates/local/Lucidos.tar.gz")
        );
        let bytes = client
            .get(url)
            .header(auth::HEADER_LOCAL_TOKEN, "test-local-token")
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"one bundle");
        server.abort();
    }
}

#[tokio::test]
async fn mapped_ipv4_loopback_uses_the_canonical_local_origin() {
    let temp = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(temp.path().into()));
    staged(&state);
    let mut req = request(
        "/~/updates/local/latest.json",
        "test-local-token",
        "127.0.0.1",
        12345,
    );
    *req.extensions_mut().get_mut::<pairing::Peer>().unwrap() = pairing::Peer {
        remote: "[::ffff:127.0.0.1]:4321".parse().unwrap(),
        local: "[::ffff:127.0.0.1]:12345".parse().unwrap(),
        public_listener: false,
    };
    let response = crate::server::gateway_router(state)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let feed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        feed["platforms"]["darwin-aarch64"]["url"],
        "http://127.0.0.1:12345/~/updates/local/Lucidos.tar.gz"
    );
}

#[tokio::test]
async fn missing_local_bundle_is_loud_and_symlinks_cannot_escape() {
    let temp = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(temp.path().into()));
    let dir = staged(&state);
    std::fs::remove_file(dir.join("Lucidos.tar.gz")).unwrap();
    let response = crate::server::gateway_router(state.clone())
        .oneshot(request(
            "/~/updates/local/latest.json",
            "test-local-token",
            "127.0.0.1",
            12345,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!to_bytes(response.into_body(), 4096)
        .await
        .unwrap()
        .is_empty());
    #[cfg(unix)]
    {
        let secret = temp.path().join("secret");
        std::fs::write(&secret, "private").unwrap();
        std::os::unix::fs::symlink(secret, dir.join("Lucidos.tar.gz")).unwrap();
        assert!(read_feed(&dir, "http://127.0.0.1:12345").is_err());
    }
}
