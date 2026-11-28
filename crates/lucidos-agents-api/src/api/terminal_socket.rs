//! The thread terminal's socket: one attachment per open view (ADR 0356
//! section 2). Frames are in [`ClientFrame`] and [`ServerFrame`].
//!
//! INVARIANT: The socket never waits for the shell, and the shell never waits
//! for a socket. A client that falls behind the live stream is redrawn from
//! the ring.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use uuid::Uuid;

use super::error::ApiError;
use super::socket_ticket::{self, SocketTarget};
use super::AppState;
use crate::engine::terminal::{lifecycle, Attachment, CatchUp, Exit, Output, Terminal};
use crate::engine::thread_events::TerminalExitCause;
use crate::runtime::pty::Size;

/// INVARIANT: The largest input frame a client may send. Enforced on the
/// socket, so an oversized frame is refused as it arrives.
const MAX_INPUT_FRAME: usize = 64 * 1024;
/// Output bytes gathered into one frame, at most.
const MAX_OUTPUT_FRAME: usize = 32 * 1024;
const HELLO_LIMIT: Duration = Duration::from_secs(10);
const NO_LIVE_TICKET: &str =
    "A terminal socket needs a live ticket for this terminal. Mint one at \
     POST /api/v1/threads/:thread_id/terminal/:terminal_id/ticket and dial before it expires.";

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(super) enum ClientFrame {
    /// First and once: the offset the client already holds, and its size.
    Hello {
        #[serde(default)]
        resume_from: Option<u64>,
        cols: u16,
        rows: u16,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    /// The client became the active view, so its size wins.
    Focus,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(super) enum ServerFrame {
    /// Output bytes that follow start at `offset`. `reset` clears the screen.
    Sync {
        terminal_id: Uuid,
        offset: u64,
        reset: bool,
    },
    Size {
        cols: u16,
        rows: u16,
    },
    Attachments {
        count: usize,
    },
    Exit {
        cause: TerminalExitCause,
        status: Option<i32>,
    },
}

impl ServerFrame {
    fn size(size: Size) -> ServerFrame {
        ServerFrame::Size {
            cols: size.cols,
            rows: size.rows,
        }
    }

    fn exit(exit: Exit) -> ServerFrame {
        ServerFrame::Exit {
            cause: exit.cause,
            status: exit.exit_code,
        }
    }
}

async fn socket(
    State(state): State<AppState>,
    Path((thread_id, terminal_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    uri: Uri,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let terminal = admit(&state, &headers, &uri, thread_id, terminal_id).await?;
    Ok(upgrade
        .max_message_size(MAX_INPUT_FRAME)
        .on_upgrade(move |socket| serve(socket, terminal)))
}

/// Every check the handshake makes, in order, before any upgrade.
pub(super) async fn admit(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    thread_id: Uuid,
    terminal_id: Uuid,
) -> Result<Arc<Terminal>, ApiError> {
    if !lifecycle::switch_is_on(&state.pool).await {
        return Err(super::terminal::disabled());
    }
    let generation = crate::core::snapshot::boundary::read(&state.pool)
        .await
        .map_err(ApiError::db)?
        .map(|boundary| boundary.restore_id);
    let target = SocketTarget::Terminal {
        thread_id,
        terminal_id,
    };
    let device_id = socket_ticket::ticket_in(uri)
        .and_then(|ticket| state.socket_tickets.redeem(&ticket, target, generation))
        .ok_or_else(|| ApiError::new(StatusCode::FORBIDDEN, NO_LIVE_TICKET))?;
    if !socket_ticket::header_agrees(headers, &device_id) {
        crate::log!(
            "[Terminal] Refusing a socket: the ticket names another device than the header"
        );
        return Err(ApiError::new(StatusCode::FORBIDDEN, NO_LIVE_TICKET));
    }
    super::terminal::named_terminal(state, thread_id, terminal_id).await
}

async fn serve(mut socket: WebSocket, terminal: Arc<Terminal>) {
    let Some((resume_from, size)) = hello(&mut socket).await else {
        return;
    };
    let attachment = terminal.attach(resume_from);
    if let Err(e) = terminal.resize(size) {
        crate::log!(
            "[Terminal] Could not resize a terminal: {}",
            lucidos_diagnostic_log::private(e.to_string())
        );
    }
    if let Err(e) = relay(&mut socket, attachment, size).await {
        crate::log!(
            "[Terminal] A terminal socket ended: {}",
            lucidos_diagnostic_log::private(e.to_string())
        );
    }
}

async fn hello(socket: &mut WebSocket) -> Option<(Option<u64>, Size)> {
    let frame = tokio::time::timeout(HELLO_LIMIT, socket.recv())
        .await
        .ok()??;
    let Ok(Message::Text(text)) = frame else {
        return None;
    };
    match serde_json::from_str(&text) {
        Ok(ClientFrame::Hello {
            resume_from,
            cols,
            rows,
        }) => Some((resume_from, Size::clamped(cols, rows))),
        _ => None,
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

async fn send_frame(socket: &mut WebSocket, frame: &ServerFrame) -> Result<(), BoxError> {
    socket
        .send(Message::Text(serde_json::to_string(frame)?))
        .await?;
    Ok(())
}

async fn send_catch_up(
    socket: &mut WebSocket,
    terminal_id: Uuid,
    catch_up: &CatchUp,
) -> Result<u64, BoxError> {
    let (offset, data, reset) = match catch_up {
        CatchUp::Bytes { offset, data } => (*offset, data, false),
        CatchUp::Reset { offset, data } => (*offset, data, true),
    };
    send_frame(
        socket,
        &ServerFrame::Sync {
            terminal_id,
            offset,
            reset,
        },
    )
    .await?;
    if !data.is_empty() {
        socket.send(Message::Binary(data.clone())).await?;
    }
    Ok(offset + data.len() as u64)
}

/// What the relay does with one item from the live stream.
enum Step {
    Bytes(Vec<u8>),
    Frame(ServerFrame),
    Redraw,
    Ended(Option<Exit>),
}

/// One attachment's read side. `pending` holds an item that arrived while
/// output was being gathered, so it goes out next, in order.
struct Stream {
    attachment: Attachment,
    pending: Option<Output>,
    expected: u64,
}

impl Stream {
    async fn next_step(&mut self) -> Step {
        let first = match self.pending.take() {
            Some(output) => output,
            None => match self.attachment.output.recv().await {
                Ok(output) => output,
                Err(RecvError::Lagged(_)) => return Step::Redraw,
                Err(RecvError::Closed) => return Step::Ended(None),
            },
        };
        let Output::Bytes { offset, data } = first else {
            return step_for(first);
        };
        if offset != self.expected {
            return Step::Redraw;
        }
        let mut bytes = data.to_vec();
        while bytes.len() < MAX_OUTPUT_FRAME {
            match self.attachment.output.try_recv() {
                Ok(Output::Bytes { data, .. }) => bytes.extend_from_slice(&data),
                Ok(other) => {
                    self.pending = Some(other);
                    break;
                }
                Err(TryRecvError::Lagged(_)) => return Step::Redraw,
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
        self.expected += bytes.len() as u64;
        Step::Bytes(bytes)
    }
}

fn step_for(output: Output) -> Step {
    match output {
        Output::Bytes { data, .. } => Step::Bytes(data.to_vec()),
        Output::Resized { size } => Step::Frame(ServerFrame::size(size)),
        Output::Attachments { count } => Step::Frame(ServerFrame::Attachments { count }),
        Output::Exited(exit) => Step::Ended(Some(exit)),
    }
}

async fn relay(
    socket: &mut WebSocket,
    attachment: Attachment,
    mut own_size: Size,
) -> Result<(), BoxError> {
    let terminal = attachment.terminal.clone();
    let expected = send_catch_up(socket, terminal.id, &attachment.catch_up).await?;
    send_frame(socket, &ServerFrame::size(terminal.size())).await?;
    send_frame(
        socket,
        &ServerFrame::Attachments {
            count: terminal.attachment_count(),
        },
    )
    .await?;
    if let Some(exit) = attachment.exited {
        return finish(socket, Some(exit)).await;
    }
    let mut stream = Stream {
        attachment,
        pending: None,
        expected,
    };
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Binary(input))) => write_input(&terminal, input).await,
                    Some(Ok(Message::Text(text))) => {
                        if let Some(size) = client_size(&text, own_size) {
                            own_size = size;
                            if let Err(e) = terminal.resize(size) {
                                crate::log!(
                                    "[Terminal] Could not resize a terminal: {}",
                                    lucidos_diagnostic_log::private(e.to_string())
                                );
                            }
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return Ok(()),
                }
            }
            step = stream.next_step() => match step {
                Step::Bytes(bytes) => socket.send(Message::Binary(bytes)).await?,
                Step::Frame(frame) => send_frame(socket, &frame).await?,
                Step::Redraw => {
                    stream.pending = None;
                    let catch_up = stream.attachment.redraw();
                    stream.expected = send_catch_up(socket, terminal.id, &catch_up).await?;
                    if let Some(exit) = stream.attachment.exited {
                        return finish(socket, Some(exit)).await;
                    }
                }
                Step::Ended(exit) => return finish(socket, exit).await,
            }
        }
    }
}

/// The size a text frame asks for: a resize, or this view's own size again
/// when it takes focus. Unreadable frames ask for nothing.
fn client_size(text: &str, own_size: Size) -> Option<Size> {
    match serde_json::from_str(text).ok()? {
        ClientFrame::Resize { cols, rows } => Some(Size::clamped(cols, rows)),
        ClientFrame::Focus => Some(own_size),
        ClientFrame::Hello { .. } => None,
    }
}

async fn write_input(terminal: &Arc<Terminal>, input: Vec<u8>) {
    let terminal = terminal.clone();
    let written = tokio::task::spawn_blocking(move || terminal.write(&input)).await;
    if let Ok(Err(e)) = written {
        crate::log!(
            "[Terminal] Could not write to a terminal: {}",
            lucidos_diagnostic_log::private(e.to_string())
        );
    }
}

async fn finish(socket: &mut WebSocket, exit: Option<Exit>) -> Result<(), BoxError> {
    if let Some(exit) = exit {
        send_frame(socket, &ServerFrame::exit(exit)).await?;
    }
    socket.send(Message::Close(None)).await?;
    Ok(())
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/threads/:thread_id/terminal/:terminal_id/socket",
        get(socket),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_frames_parse_from_their_wire_shape() {
        assert_eq!(
            serde_json::from_str::<ClientFrame>(
                r#"{"type":"hello","resume_from":42,"cols":120,"rows":40}"#
            )
            .unwrap(),
            ClientFrame::Hello {
                resume_from: Some(42),
                cols: 120,
                rows: 40
            }
        );
        assert_eq!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"hello","cols":80,"rows":24}"#).unwrap(),
            ClientFrame::Hello {
                resume_from: None,
                cols: 80,
                rows: 24
            }
        );
        assert_eq!(
            serde_json::from_str::<ClientFrame>(r#"{"type":"focus"}"#).unwrap(),
            ClientFrame::Focus
        );
    }

    #[test]
    fn server_frames_serialize_to_their_wire_shape() {
        let exit = ServerFrame::exit(Exit {
            cause: TerminalExitCause::Disabled,
            exit_code: None,
        });
        assert_eq!(
            serde_json::to_value(exit).unwrap(),
            serde_json::json!({"type": "exit", "cause": "disabled", "status": null})
        );
        assert_eq!(
            serde_json::to_value(ServerFrame::Attachments { count: 2 }).unwrap(),
            serde_json::json!({"type": "attachments", "count": 2})
        );
    }

    #[test]
    fn focus_reclaims_this_views_size_and_resize_clamps() {
        let own = Size { cols: 90, rows: 30 };
        assert_eq!(client_size(r#"{"type":"focus"}"#, own), Some(own));
        assert_eq!(
            client_size(r#"{"type":"resize","cols":0,"rows":9999}"#, own),
            Some(Size::clamped(0, 9999))
        );
        assert_eq!(client_size("not json", own), None);
    }
}
