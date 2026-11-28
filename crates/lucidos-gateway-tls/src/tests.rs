use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;

use axum::routing::get;
use axum::Router;
use reqwest::StatusCode;
use tokio::net::TcpStream;
use tower::ServiceExt as _;

use super::acceptor::{accept_at, MaybeTlsStream, Tagged};
use super::*;

const NAME: &str = "node-7.some-base.example";

const REMOTE_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));

struct Pem {
    cert: Vec<u8>,
    key: Vec<u8>,
}

fn mint(name: &str) -> Pem {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let issued = rcgen::generate_simple_self_signed(vec![name.to_string()]).unwrap();
    Pem {
        cert: issued.cert.pem().into_bytes(),
        key: issued.key_pair.serialize_pem().into_bytes(),
    }
}

fn loopback() -> LoopbackTls {
    lucidos_local_token::test_support::server_config()
}

async fn store_holding(pem: &Pem) -> Arc<CertStore> {
    let store = Arc::new(CertStore::default());
    store
        .install(NAME, pem.cert.clone(), pem.key.clone())
        .await
        .unwrap();
    store.set_addresses(vec![REMOTE_IP]);
    store
}

#[derive(Clone)]
struct RemoteAcceptor(ListenerTls);

impl<S: Send + 'static> axum_server::accept::Accept<TcpStream, S> for RemoteAcceptor {
    type Stream = MaybeTlsStream;
    type Service = Tagged<S>;
    type Future = Pin<Box<dyn Future<Output = io::Result<(MaybeTlsStream, Tagged<S>)>> + Send>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        let tls = self.0.clone();
        Box::pin(async move {
            let local = SocketAddr::new(REMOTE_IP, stream.local_addr()?.port());
            accept_at(tls, stream, local, false, service).await
        })
    }
}

fn echo_router() -> Router {
    let echo = |connection: Connection| async move { format!("{connection:?}") };
    Router::new()
        .route("/", get(echo))
        .route("/*rest", get(echo))
        .layer(axum::middleware::from_fn(plaintext_gate))
}

async fn serve<A>(acceptor: A) -> u16
where
    A: axum_server::accept::Accept<TcpStream, Router> + Clone + Send + Sync + 'static,
    A::Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
    A::Service: tower::Service<
            axum::http::Request<hyper::body::Incoming>,
            Response = axum::response::Response,
            Error = std::convert::Infallible,
        > + Clone
        + Send
        + 'static,
    <A::Service as tower::Service<axum::http::Request<hyper::body::Incoming>>>::Future: Send,
    A::Future: Send,
{
    serve_on(
        std::net::TcpListener::bind("127.0.0.1:0").unwrap(),
        acceptor,
    )
    .await
}

async fn serve_on<A>(listener: std::net::TcpListener, acceptor: A) -> u16
where
    A: axum_server::accept::Accept<TcpStream, Router> + Clone + Send + Sync + 'static,
    A::Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
    A::Service: tower::Service<
            axum::http::Request<hyper::body::Incoming>,
            Response = axum::response::Response,
            Error = std::convert::Infallible,
        > + Clone
        + Send
        + 'static,
    <A::Service as tower::Service<axum::http::Request<hyper::body::Incoming>>>::Future: Send,
    A::Future: Send,
{
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum_server::from_tcp(listener)
            .acceptor(acceptor)
            .serve(echo_router().into_make_service())
            .await
            .ok();
    });
    port
}

fn client_trusting(pem: &Pem, port: u16) -> reqwest::Client {
    client_for(pem, NAME, port)
}

fn client_for(pem: &Pem, name: &str, port: u16) -> reqwest::Client {
    reqwest::Client::builder()
        .use_rustls_tls()
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_pem(&pem.cert).unwrap())
        .resolve(name, SocketAddr::from(([127, 0, 0, 1], port)))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn body(response: reqwest::Response) -> String {
    response.text().await.unwrap()
}

fn loopback_client() -> reqwest::Client {
    lucidos_local_token::client::client_builder()
        .build()
        .unwrap()
}

#[tokio::test]
async fn a_loopback_tls_hello_gets_the_loopback_ca_leaf_while_a_managed_cert_is_held() {
    let pem = mint(NAME);
    let port = serve(GatewayAcceptor::new(ListenerTls::Managed(
        store_holding(&pem).await,
        loopback(),
    )))
    .await;
    for host in ["localhost", "127.0.0.1"] {
        let response = loopback_client()
            .get(format!("https://{host}:{port}/~/"))
            .send()
            .await
            .unwrap();
        assert_eq!(body(response).await, "Tls", "{host}");
    }
}

#[tokio::test]
async fn the_ipv6_loopback_listener_serves_the_loopback_ca_leaf() {
    let Ok(listener) = std::net::TcpListener::bind("[::1]:0") else {
        eprintln!("skipped: this host has no IPv6 loopback");
        return;
    };
    let pem = mint(NAME);
    let port = serve_on(
        listener,
        GatewayAcceptor::new(ListenerTls::Managed(store_holding(&pem).await, loopback())),
    )
    .await;
    let response = loopback_client()
        .get(format!("https://[::1]:{port}/~/"))
        .send()
        .await
        .unwrap();
    assert_eq!(body(response).await, "Tls");
}

#[tokio::test]
async fn direct_plain_http_on_loopback_reaches_only_the_hand_off() {
    let port = serve(GatewayAcceptor::new(ListenerTls::Managed(
        Arc::default(),
        loopback(),
    )))
    .await;
    let plain = format!("http://127.0.0.1:{port}");
    let api = loopback_client()
        .post(format!("{plain}/~/api/v1/health?x=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(api.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        api.headers()[reqwest::header::LOCATION],
        format!("https://localhost:{port}/~/api/v1/health?x=1").as_str()
    );

    let page = loopback_client()
        .get(format!("{plain}/dev/"))
        .header("sec-fetch-dest", "document")
        .header(reqwest::header::COOKIE, "lucidos_device_x=secret")
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.headers().get(reqwest::header::SET_COOKIE).is_none());
    assert!(body(page).await.contains("lucidosOutboxHandoff.start()"));

    let clear = |site: &'static str| {
        loopback_client()
            .post(format!("{plain}{}", handoff::CLEAR_PATH))
            .header("sec-fetch-site", site)
            .send()
    };
    let cleared = clear("same-origin").await.unwrap();
    assert_eq!(cleared.status(), StatusCode::OK);
    assert_eq!(
        cleared.headers()["clear-site-data"],
        "\"cache\", \"storage\""
    );
    let cross = clear("cross-site").await.unwrap();
    assert_eq!(cross.status(), StatusCode::MOVED_PERMANENTLY);
    assert!(cross.headers().get("clear-site-data").is_none());
}

#[tokio::test]
async fn a_proxied_hop_on_plain_loopback_reaches_the_router() {
    let port = serve(GatewayAcceptor::new(ListenerTls::Managed(
        Arc::default(),
        loopback(),
    )))
    .await;
    let response = loopback_client()
        .get(format!("http://127.0.0.1:{port}/~/"))
        .header("x-forwarded-for", "100.64.0.7")
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, "Plain");
}

#[tokio::test]
async fn a_remote_tls_hello_is_served_the_managed_cert() {
    let pem = mint(NAME);
    let port = serve(RemoteAcceptor(ListenerTls::Managed(
        store_holding(&pem).await,
        loopback(),
    )))
    .await;
    let response = client_trusting(&pem, port)
        .get(format!("https://{NAME}:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(body(response).await, "Tls");
}

#[tokio::test]
async fn remote_plain_http_is_redirected_to_the_cert_s_own_origin() {
    let pem = mint(NAME);
    let port = serve(RemoteAcceptor(ListenerTls::Managed(
        store_holding(&pem).await,
        loopback(),
    )))
    .await;
    let response = client_trusting(&pem, port)
        .get(format!("http://{NAME}:{port}/~/pair?code=01234567"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        response.headers()[reqwest::header::LOCATION],
        format!("https://{NAME}:{port}/~/pair?code=01234567").as_str()
    );
}

#[tokio::test]
async fn a_remote_connection_stays_plain_until_a_cert_is_held() {
    let pem = mint(NAME);
    let port = serve(RemoteAcceptor(ListenerTls::Managed(
        Arc::default(),
        loopback(),
    )))
    .await;
    let response = client_trusting(&pem, port)
        .get(format!("http://{NAME}:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, "Plain");
}

#[tokio::test]
async fn an_address_tailscaled_did_not_report_stays_plain() {
    let pem = mint(NAME);
    let store = store_holding(&pem).await;
    store.set_addresses(vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 99))]);
    let port = serve(RemoteAcceptor(ListenerTls::Managed(store, loopback()))).await;
    let response = client_trusting(&pem, port)
        .get(format!("http://{NAME}:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, "Plain");
}

#[tokio::test]
async fn a_dual_stack_listener_matches_an_ipv4_mapped_tailnet_address() {
    let store = store_holding(&mint(NAME)).await;
    let mapped: IpAddr = "::ffff:192.0.2.10".parse().unwrap();
    assert!(store.serving(mapped).is_some());
    store.withdraw();
    assert!(store.serving(REMOTE_IP).is_none());
}

#[tokio::test]
async fn a_cert_swap_leaves_open_connections_serving_and_reaches_new_ones() {
    let old = mint(NAME);
    let new = mint(NAME);
    let store = store_holding(&old).await;
    let port = serve(RemoteAcceptor(ListenerTls::Managed(
        store.clone(),
        loopback(),
    )))
    .await;
    let url = format!("https://{NAME}:{port}/");

    let open = client_trusting(&old, port);
    assert_eq!(body(open.get(&url).send().await.unwrap()).await, "Tls");

    store
        .install(NAME, new.cert.clone(), new.key.clone())
        .await
        .unwrap();

    assert_eq!(
        body(open.get(&url).send().await.unwrap()).await,
        "Tls",
        "the open connection keeps serving across the swap"
    );
    assert!(
        client_trusting(&old, port).get(&url).send().await.is_err(),
        "a new connection must not get the old cert"
    );
    assert_eq!(
        body(client_trusting(&new, port).get(&url).send().await.unwrap()).await,
        "Tls"
    );
}

#[tokio::test]
async fn a_pair_that_will_not_parse_leaves_the_current_cert_serving() {
    let pem = mint(NAME);
    let store = store_holding(&pem).await;
    assert!(store
        .install("other.example", b"junk".to_vec(), b"junk".to_vec())
        .await
        .is_err());
    assert_eq!(store.current().unwrap().name, NAME);
}

#[tokio::test]
async fn an_operator_listener_serves_the_loopback_ca_on_loopback() {
    let pem = mint(NAME);
    let config = RustlsConfig::from_pem(pem.cert.clone(), pem.key.clone())
        .await
        .unwrap();
    let port = serve(GatewayAcceptor::new(ListenerTls::Operator(
        config,
        Arc::default(),
        loopback(),
    )))
    .await;
    let response = loopback_client()
        .get(format!("https://localhost:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(body(response).await, "Tls");
}

#[tokio::test]
async fn an_operator_listener_picks_the_certificate_by_name() {
    const OPERATOR_NAME: &str = "gateway.example";
    let managed = mint(NAME);
    let operator = mint(OPERATOR_NAME);
    let config = RustlsConfig::from_pem(operator.cert.clone(), operator.key.clone())
        .await
        .unwrap();
    let store = store_holding(&managed).await;
    let port = serve(RemoteAcceptor(ListenerTls::Operator(
        config,
        store,
        loopback(),
    )))
    .await;
    let tailnet = client_for(&managed, NAME, port)
        .get(format!("https://{NAME}:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(body(tailnet).await, "Tls");
    let other = client_for(&operator, OPERATOR_NAME, port)
        .get(format!("https://{OPERATOR_NAME}:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(body(other).await, "Tls");
}

#[tokio::test]
async fn only_plain_http_with_https_on_offer_is_redirected() {
    let request = |connection: Connection| {
        let mut request = axum::http::Request::builder()
            .uri("/x")
            .body(axum::body::Body::empty())
            .unwrap();
        request.extensions_mut().insert(connection);
        request
    };
    for connection in [Connection::Plain, Connection::Tls] {
        let response = echo_router()
            .oneshot(request(connection.clone()))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::OK,
            "{connection:?}"
        );
    }
    let response = echo_router()
        .oneshot(request(Connection::PlainWithHttps {
            authority: format!("{NAME}:5252"),
        }))
        .await
        .unwrap();
    assert_eq!(
        response.headers()[header::LOCATION],
        format!("https://{NAME}:5252/x").as_str()
    );
}

#[test]
fn an_ipv4_mapped_loopback_caller_counts_as_loopback() {
    assert!(is_loopback("[::ffff:127.0.0.1]:5252".parse().unwrap()));
    assert!(is_loopback("[::1]:5252".parse().unwrap()));
    assert!(!is_loopback(SocketAddr::new(REMOTE_IP, 5252)));
}
