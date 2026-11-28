//! Serve the loopback port during recovery without dispatching API requests.

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, Response, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use axum_server::Handle;
use std::io;
use std::net::{SocketAddr, TcpListener};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tower::{service_fn, ServiceExt};

const BOOTING_HEADER: &str = "x-lucidos-engine-booting";

pub(crate) struct BootServer {
    router: watch::Sender<Option<Router>>,
    handles: Vec<Handle>,
    task: JoinSet<io::Result<()>>,
    address: SocketAddr,
}

impl BootServer {
    pub(crate) async fn start(
        listeners: Vec<TcpListener>,
        tls: Option<RustlsConfig>,
    ) -> io::Result<Self> {
        let address = listeners[0].local_addr()?;
        let handles: Vec<_> = listeners.iter().map(|_| Handle::new()).collect();
        let (router, live) = watch::channel(None::<Router>);
        let startup_router =
            Router::new().fallback_service(service_fn(move |request: Request<Body>| {
                let router = live.borrow().clone();
                async move {
                    match router {
                        Some(router) => {
                            let mut response = router.oneshot(request).await?;
                            response.headers_mut().remove(BOOTING_HEADER);
                            Ok(response)
                        }
                        None => Ok(booting_response()),
                    }
                }
            }));

        let mut task = JoinSet::new();
        let server_handles = handles.clone();
        task.spawn(async move {
            let listeners = listeners.into_iter().zip(server_handles);
            if let Some(tls) = tls {
                futures::future::try_join_all(listeners.map(|(listener, handle)| {
                    axum_server::from_tcp_rustls(listener, tls.clone())
                        .handle(handle)
                        .serve(startup_router.clone().into_make_service())
                }))
                .await?;
            } else {
                futures::future::try_join_all(listeners.map(|(listener, handle)| {
                    axum_server::from_tcp(listener)
                        .handle(handle)
                        .serve(startup_router.clone().into_make_service())
                }))
                .await?;
            }
            Ok(())
        });

        tokio::select! {
            result = task.join_next() => {
                result.expect("the TCP server task was started")??;
                return Err(io::Error::other("the TCP server stopped during startup"));
            }
            bound = futures::future::join_all(handles.iter().map(Handle::listening)) => {
                if bound.iter().any(Option::is_none) {
                    return Err(io::Error::other("a TCP listener stopped during startup"));
                }
            }
        }

        Ok(Self {
            router,
            handles,
            task,
            address,
        })
    }

    pub(crate) fn handles(&self) -> &[Handle] {
        &self.handles
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(crate) fn publish(&self, router: Router) {
        self.router.send_replace(Some(router));
    }

    pub(crate) async fn join(&mut self) -> io::Result<()> {
        self.task
            .join_next()
            .await
            .expect("the TCP server task was started")?
    }
}

fn booting_response() -> Response<Body> {
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, "The engine is starting.").into_response();
    response
        .headers_mut()
        .insert(BOOTING_HEADER, HeaderValue::from_static("1"));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    async fn post_mutation(address: SocketAddr) -> String {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream
                .write_all(
                    b"POST /mutation HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            let mut reply = String::new();
            stream.read_to_string(&mut reply).await.unwrap();
            reply
        })
        .await
        .expect("the listener should reply without waiting for recovery")
    }

    #[tokio::test]
    async fn boot_post_is_rejected_and_never_dispatched() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut server = BootServer::start(vec![listener], None).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/mutation",
            post({
                let calls = calls.clone();
                move || async move {
                    calls.fetch_add(1, Ordering::Relaxed);
                    (StatusCode::SERVICE_UNAVAILABLE, [(BOOTING_HEADER, "1")])
                }
            }),
        );

        let reply = post_mutation(server.address()).await;
        assert!(reply.starts_with("HTTP/1.1 503 "));
        assert!(reply.contains("x-lucidos-engine-booting: 1\r\n"));
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        server.publish(app);
        let reply = post_mutation(server.address()).await;
        assert!(reply.starts_with("HTTP/1.1 503 "));
        assert!(!reply.contains("x-lucidos-engine-booting:"));
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        server.handles()[0].graceful_shutdown(Some(Duration::from_secs(1)));
        server.join().await.unwrap();
    }
}
