use std::collections::VecDeque;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::ready;

use async_tungstenite::tungstenite::Message as WsMessage;
use async_tungstenite::{tungstenite::protocol::WebSocketConfig, WebSocketStream};
use futures::stream::Stream;
use futures::task::{Context, Poll};
use futures::{SinkExt, StreamExt};

use chromiumoxide_cdp::cdp::browser_protocol::target::SessionId;
use chromiumoxide_types::{CallId, EventMessage, Message, MethodCall, MethodId};

use crate::error::CdpError;
use crate::error::Result;

cfg_if::cfg_if! {
    if #[cfg(feature = "async-std-runtime")] {
       use async_tungstenite::async_std::ConnectStream;
    } else if #[cfg(feature = "tokio-runtime")] {
        use async_tungstenite::tokio::ConnectStream;
    }
}
/// How a [`Connection`] reaches Chrome.
#[derive(Debug)]
enum Transport {
    WebSocket(WebSocketStream<ConnectStream>),
    /// `--remote-debugging-pipe`, which opens no socket. See [`crate::pipe`].
    #[cfg(unix)]
    Pipe(crate::pipe::PipeTransport),
}

/// One read off a [`Transport`].
enum Received {
    Text(String),
    /// A WebSocket ping or pong, which carries no CDP message.
    Control,
    Closed,
}

impl Transport {
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match self {
            Self::WebSocket(ws) => ws.poll_ready_unpin(cx).map_err(CdpError::Ws),
            #[cfg(unix)]
            Self::Pipe(_) => Poll::Ready(Ok(())),
        }
    }

    fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match self {
            Self::WebSocket(ws) => ws.poll_flush_unpin(cx).map_err(CdpError::Ws),
            #[cfg(unix)]
            Self::Pipe(_) => Poll::Ready(Ok(())),
        }
    }

    fn start_send(&mut self, message: String) -> Result<()> {
        match self {
            Self::WebSocket(ws) => Ok(ws.start_send_unpin(message.into())?),
            #[cfg(unix)]
            Self::Pipe(pipe) => Ok(pipe.send(message)?),
        }
    }

    fn poll_receive(&mut self, cx: &mut Context<'_>) -> Poll<Result<Received>> {
        match self {
            Self::WebSocket(ws) => Poll::Ready(match ready!(ws.poll_next_unpin(cx)) {
                Some(Ok(WsMessage::Text(text))) => Ok(Received::Text(text)),
                Some(Ok(WsMessage::Close(_))) | None => Ok(Received::Closed),
                Some(Ok(WsMessage::Ping(_))) | Some(Ok(WsMessage::Pong(_))) => {
                    Ok(Received::Control)
                }
                Some(Ok(msg)) => Err(CdpError::UnexpectedWsMessage(msg)),
                Some(Err(err)) => Err(CdpError::Ws(err)),
            }),
            #[cfg(unix)]
            Self::Pipe(pipe) => Poll::Ready(match ready!(pipe.poll_next(cx)) {
                Some(Ok(text)) => Ok(Received::Text(text)),
                Some(Err(err)) => Err(err.into()),
                None => Ok(Received::Closed),
            }),
        }
    }
}

/// Exchanges the messages with the browser
#[must_use = "streams do nothing unless polled"]
#[derive(Debug)]
pub struct Connection<T: EventMessage> {
    /// Queue of commands to send.
    pending_commands: VecDeque<MethodCall>,
    /// The channel to the chromium instance
    transport: Transport,
    /// The identifier for a specific command
    next_id: usize,
    needs_flush: bool,
    /// The message that is currently being proceessed
    pending_flush: Option<MethodCall>,
    _marker: PhantomData<T>,
}

impl<T: EventMessage + Unpin> Connection<T> {
    pub async fn connect(debug_ws_url: impl AsRef<str>) -> Result<Self> {
        let config = WebSocketConfig {
            max_message_size: None,
            max_frame_size: None,
            ..Default::default()
        };

        cfg_if::cfg_if! {
            if #[cfg(feature = "async-std-runtime")] {
               let (ws, _) = async_tungstenite::async_std::connect_async_with_config(debug_ws_url.as_ref(), Some(config)).await?;
            } else if #[cfg(feature = "tokio-runtime")] {
                 let (ws, _) = async_tungstenite::tokio::connect_async_with_config(debug_ws_url.as_ref(), Some(config)).await?;
            }
        }

        Ok(Self::over(Transport::WebSocket(ws)))
    }

    /// A connection over a pipe set up by [`crate::pipe::attach`].
    #[cfg(unix)]
    pub(crate) fn pipe(pipe: crate::pipe::PipeTransport) -> Self {
        Self::over(Transport::Pipe(pipe))
    }

    fn over(transport: Transport) -> Self {
        Self {
            pending_commands: Default::default(),
            transport,
            next_id: 0,
            needs_flush: false,
            pending_flush: None,
            _marker: Default::default(),
        }
    }
}

impl<T: EventMessage> Connection<T> {
    fn next_call_id(&mut self) -> CallId {
        let id = CallId::new(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Queue in the command to send over the socket and return the id for this
    /// command
    pub fn submit_command(
        &mut self,
        method: MethodId,
        session_id: Option<SessionId>,
        params: serde_json::Value,
    ) -> serde_json::Result<CallId> {
        let id = self.next_call_id();
        let call = MethodCall {
            id,
            method,
            session_id: session_id.map(Into::into),
            params,
        };
        self.pending_commands.push_back(call);
        Ok(id)
    }

    /// flush any processed message and start sending the next over the conn
    /// sink
    fn start_send_next(&mut self, cx: &mut Context<'_>) -> Result<()> {
        if self.needs_flush {
            if let Poll::Ready(Ok(())) = self.transport.poll_flush(cx) {
                self.needs_flush = false;
            }
        }
        if self.pending_flush.is_none() && !self.needs_flush {
            if let Some(cmd) = self.pending_commands.pop_front() {
                tracing::trace!("Sending {:?}", cmd);
                let msg = serde_json::to_string(&cmd)?;
                self.transport.start_send(msg)?;
                self.pending_flush = Some(cmd);
            }
        }
        Ok(())
    }
}

impl<T: EventMessage + Unpin> Stream for Connection<T> {
    type Item = Result<Message<T>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let pin = self.get_mut();

        loop {
            // queue in the next message if not currently flushing
            if let Err(err) = pin.start_send_next(cx) {
                return Poll::Ready(Some(Err(err)));
            }

            // send the message
            if let Some(call) = pin.pending_flush.take() {
                if pin.transport.poll_ready(cx).is_ready() {
                    pin.needs_flush = true;
                    // try another flush
                    continue;
                } else {
                    pin.pending_flush = Some(call);
                }
            }

            break;
        }

        // read from the browser
        match ready!(pin.transport.poll_receive(cx)) {
            Ok(Received::Text(text)) => {
                let ready = match serde_json::from_str::<Message<T>>(&text) {
                    Ok(msg) => {
                        tracing::trace!("Received {:?}", msg);
                        Ok(msg)
                    }
                    Err(err) => {
                        tracing::debug!(target: "chromiumoxide::conn::raw_ws::parse_errors", msg = text, "Failed to parse raw WS message");
                        tracing::error!("Failed to deserialize WS response {}", err);
                        Err(err.into())
                    }
                };
                Poll::Ready(Some(ready))
            }
            Ok(Received::Closed) => Poll::Ready(None),
            Ok(Received::Control) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(err) => Poll::Ready(Some(Err(err))),
        }
    }
}
