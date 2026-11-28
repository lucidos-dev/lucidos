use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::http::Request;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;
use tokio_rustls::{LazyConfigAcceptor, TlsAcceptor};

use super::{is_loopback, Connection, ListenerTls, ManagedCert, Peer};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

const TLS_HANDSHAKE_RECORD: u8 = 0x16;

#[derive(Clone)]
pub struct GatewayAcceptor {
    tls: ListenerTls,
    public_listener: bool,
}

impl GatewayAcceptor {
    pub fn new(tls: ListenerTls) -> Self {
        GatewayAcceptor {
            tls,
            public_listener: false,
        }
    }

    pub fn public(tls: ListenerTls) -> Self {
        GatewayAcceptor {
            tls,
            public_listener: true,
        }
    }
}

impl<S: Send + 'static> axum_server::accept::Accept<TcpStream, S> for GatewayAcceptor {
    type Stream = MaybeTlsStream;
    type Service = Tagged<S>;
    type Future = Pin<Box<dyn Future<Output = io::Result<(MaybeTlsStream, Tagged<S>)>> + Send>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        let tls = self.tls.clone();
        let public_listener = self.public_listener;
        Box::pin(async move {
            let local = stream.local_addr()?;
            accept_at(tls, stream, local, public_listener, service).await
        })
    }
}

pub(super) async fn accept_at<S>(
    tls: ListenerTls,
    stream: TcpStream,
    local: SocketAddr,
    public_listener: bool,
    service: S,
) -> io::Result<(MaybeTlsStream, Tagged<S>)> {
    let peer = Peer {
        remote: stream.peer_addr()?,
        local,
        public_listener,
    };
    let (stream, connection) = negotiate(tls, stream, local).await?;
    Ok((
        stream,
        Tagged {
            inner: service,
            connection,
            peer,
        },
    ))
}

async fn negotiate(
    tls: ListenerTls,
    stream: TcpStream,
    local: SocketAddr,
) -> io::Result<(MaybeTlsStream, Connection)> {
    let plain = |stream| Ok((MaybeTlsStream::Plain(stream), Connection::Plain));
    match tls {
        ListenerTls::Plain => plain(stream),
        ListenerTls::Operator(_, _, loopback) | ListenerTls::Managed(_, loopback)
            if is_loopback(local) =>
        {
            if opens_with_tls(&stream).await? {
                handshake(loopback, stream).await
            } else {
                Ok((MaybeTlsStream::Plain(stream), Connection::PlainLoopback))
            }
        }
        ListenerTls::Operator(config, store, _) => match store.serving(local.ip()) {
            Some(managed) => handshake_by_name(config.get_inner(), managed, stream).await,
            None => handshake(config.get_inner(), stream).await,
        },
        ListenerTls::Managed(store, _) => {
            let Some(cert) = store.serving(local.ip()) else {
                return plain(stream);
            };
            if opens_with_tls(&stream).await? {
                handshake(cert.config.clone(), stream).await
            } else {
                let authority = format!("{}:{}", cert.name, local.port());
                Ok((
                    MaybeTlsStream::Plain(stream),
                    Connection::PlainWithHttps { authority },
                ))
            }
        }
    }
}

async fn opens_with_tls(stream: &TcpStream) -> io::Result<bool> {
    let mut first = [0u8; 1];
    let read = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut first))
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    if read == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(first[0] == TLS_HANDSHAKE_RECORD)
}

async fn handshake(
    config: Arc<rustls::ServerConfig>,
    stream: TcpStream,
) -> io::Result<(MaybeTlsStream, Connection)> {
    let tls = tokio::time::timeout(HANDSHAKE_TIMEOUT, TlsAcceptor::from(config).accept(stream))
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    Ok((MaybeTlsStream::Tls(Box::new(tls)), Connection::Tls))
}

async fn handshake_by_name(
    operator: Arc<rustls::ServerConfig>,
    managed: Arc<ManagedCert>,
    stream: TcpStream,
) -> io::Result<(MaybeTlsStream, Connection)> {
    let accept = async {
        let start = LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream).await?;
        let asks_for_managed = start
            .client_hello()
            .server_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(&managed.name));
        let config = if asks_for_managed {
            managed.config.clone()
        } else {
            operator
        };
        start.into_stream(config).await
    };
    let tls = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    Ok((MaybeTlsStream::Tls(Box::new(tls)), Connection::Tls))
}

pub enum MaybeTlsStream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for MaybeTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            MaybeTlsStream::Plain(s) => s.is_write_vectored(),
            MaybeTlsStream::Tls(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

#[derive(Clone)]
pub struct Tagged<S> {
    inner: S,
    connection: Connection,
    peer: Peer,
}

impl<S, B> tower::Service<Request<B>> for Tagged<S>
where
    S: tower::Service<Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        req.extensions_mut().insert(self.connection.clone());
        req.extensions_mut().insert(self.peer);
        self.inner.call(req)
    }
}
