//! The row subscription: one task per stack that follows its engine's row
//! stream and keeps the picker row current (ADR 0756).
//!
//! A frame is a poke, never the value. The unread count depends on who reads
//! it, so on a poke the task reads the engine's two snapshot routes again.
//! The gateway holds no database handle (ADR 0014 §1), and those reads stay
//! the only road to either fact.

use std::collections::HashMap;
use std::sync::Weak;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt as _;
use lucidos_diagnostic_log::private;
use lucidos_gateway_stamp::row_stream::{self, Poke};
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::engine_hop::{self, Credential, EngineAddress};
use crate::stack::{self, StackRuntime};

/// How long a failed snapshot read waits before it runs again. The stream is
/// live, so no later frame is owed, and the row must not stay blank.
const SNAPSHOT_RETRY: Duration = Duration::from_secs(2);

/// The longest frame line the task buffers. A frame is under a hundred bytes.
const MAX_LINE_BYTES: usize = 64 * 1024;

/// One stack's row subscription, and the counts that live only while it does.
pub struct RowSubscription {
    task: Option<Task>,
    /// Each paired device's own unread count, read on a listing. Held only
    /// between two frames that could move it.
    device_unread: HashMap<String, u64>,
    /// Rises whenever the held counts are dropped, so a read that started
    /// before the drop cannot store its answer after it.
    epoch: u64,
    /// The status the last stream was refused with, so a standing refusal
    /// logs once and not on every supervise pass.
    refused: Option<StatusCode>,
    /// How long the stream may stay silent before the task drops it.
    silence_limit: Duration,
    /// What the gateway has done about an engine too old to serve the stream.
    without_stream: WithoutStream,
}

/// Where a stack stands with a healthy engine that lacks the row stream. It
/// outlives a stopped subscription, so one engine is logged and respawned
/// once, not on every supervise pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum WithoutStream {
    /// The engine serves the stream, or none has answered yet.
    #[default]
    No,
    /// The engine lacks it, and that is logged.
    Seen,
    /// The gateway respawned the engine for it. It does not do so twice.
    Respawned,
}

impl Default for RowSubscription {
    fn default() -> Self {
        Self {
            task: None,
            device_unread: HashMap::new(),
            epoch: 0,
            refused: None,
            silence_limit: row_stream::SILENCE_LIMIT,
            without_stream: WithoutStream::No,
        }
    }
}

/// Why a row stream stopped being followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// The engine answered the request with this status.
    Refused(StatusCode),
    /// The engine sent nothing for the silence limit: no answer to the
    /// request, or no frame and no keepalive after it.
    Stalled,
    /// The stream closed, or its request never got an answer.
    Closed,
}

/// The task's handle. Dropping it ends the task, so a deleted stack leaves no
/// stream behind.
struct Task(tokio::task::JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl RowSubscription {
    /// A task follows the stream right now.
    pub fn is_live(&self) -> bool {
        self.task.as_ref().is_some_and(|t| !t.0.is_finished())
    }

    /// Follow `engine`'s row stream for the stack behind `stack`, replacing
    /// any earlier task.
    pub fn start(&mut self, stack: Weak<AsyncMutex<StackRuntime>>, engine: EngineAddress) {
        self.forget_devices();
        let silence_limit = self.silence_limit;
        self.task = Some(Task(tokio::spawn(run(stack, engine, silence_limit))));
    }

    /// End the task. The caller blanks the row.
    pub fn stop(&mut self) {
        self.task = None;
        self.forget_devices();
    }

    /// Record what a healthy probe said about the stream. Returns `true` the
    /// first time an engine is seen to lack it, so the caller logs it once.
    pub fn note_engine_stream(&mut self, offered: bool) -> bool {
        let first_sight = !offered && self.without_stream == WithoutStream::No;
        if offered {
            self.without_stream = WithoutStream::No;
        } else if first_sight {
            self.without_stream = WithoutStream::Seen;
        }
        first_sight
    }

    /// The engine last probed healthy lacks the stream, so nothing here is
    /// kept current for it.
    pub fn engine_lacks_stream(&self) -> bool {
        self.without_stream != WithoutStream::No
    }

    /// The gateway may still respawn this engine to get the stream.
    pub fn stream_respawn_owed(&self) -> bool {
        self.without_stream == WithoutStream::Seen
    }

    /// The gateway respawned the engine for the stream.
    pub fn note_stream_respawn(&mut self) {
        self.without_stream = WithoutStream::Respawned;
    }

    /// Drop every held device count.
    pub fn forget_devices(&mut self) {
        self.device_unread.clear();
        self.epoch += 1;
    }

    /// The held count for `device_id`, and the epoch a fresh read must name
    /// to store its answer. `None` with no live stream: nothing would ever
    /// drop a count held then.
    pub fn device_unread(&self, device_id: &str) -> Option<(Option<u64>, u64)> {
        self.is_live()
            .then(|| (self.device_unread.get(device_id).copied(), self.epoch))
    }

    /// Hold `count` for `device_id`, unless the counts were dropped since
    /// `epoch`.
    pub fn remember_device(&mut self, epoch: u64, device_id: &str, count: u64) {
        if self.is_live() && epoch == self.epoch {
            self.device_unread.insert(device_id.to_string(), count);
        }
    }

    /// Drop a silent stream after `limit`, so a test need not wait out the
    /// real one.
    #[cfg(test)]
    pub fn set_silence_limit_for_test(&mut self, limit: Duration) {
        self.silence_limit = limit;
    }

    /// A subscription that reads as live and follows nothing.
    #[cfg(test)]
    pub fn live_for_test() -> Self {
        Self {
            task: Some(Task(tokio::spawn(std::future::pending()))),
            ..Self::default()
        }
    }
}

async fn run(
    stack: Weak<AsyncMutex<StackRuntime>>,
    engine: EngineAddress,
    silence_limit: Duration,
) {
    let refresh = Notify::new();
    let end = tokio::select! {
        end = follow(&stack, &engine, &refresh, silence_limit) => end,
        () = refresh_on_poke(&stack, &engine, &refresh) => End::Closed,
    };
    let Some(stack) = stack.upgrade() else { return };
    let mut s = stack.lock().await;
    let id = private(s.ws.id.clone());
    let refused = match end {
        End::Refused(status) => Some(status),
        End::Stalled | End::Closed => None,
    };
    match end {
        End::Refused(_) if s.row_subscription.refused == refused => {}
        End::Refused(status) => crate::log!("[Gateway] row stream to '{}' refused: {}", id, status),
        End::Stalled => crate::log!("[Gateway] row stream to '{}' stalled, dropped", id),
        End::Closed => crate::log!("[Gateway] row stream to '{}' ended", id),
    }
    s.row_subscription.refused = refused;
    // No live stream, so nothing held is trusted: the row says nothing until
    // the next subscription's first snapshot.
    s.blank_row();
}

/// Read the stream to its end, and ask for a snapshot read on every poke.
///
/// Any bytes count as life, a keepalive included. A stream silent for
/// `silence_limit` is dropped, and the next healthy probe subscribes again.
/// The limit also bounds the wait for the engine's answer to the request.
async fn follow(
    stack: &Weak<AsyncMutex<StackRuntime>>,
    engine: &EngineAddress,
    refresh: &Notify,
    silence_limit: Duration,
) -> End {
    let Ok(req) = Request::get(row_stream::request_path()).body(Body::empty()) else {
        return End::Closed;
    };
    let sent = engine_hop::send(engine, req, Credential::None);
    let resp = match tokio::time::timeout(silence_limit, sent).await {
        Ok(Ok(resp)) => resp,
        Ok(Err(_)) => return End::Closed,
        Err(_) => return End::Stalled,
    };
    if !resp.status().is_success() {
        return End::Refused(resp.status());
    }
    let mut body = Body::new(resp.into_body()).into_data_stream();
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let chunk = match tokio::time::timeout(silence_limit, body.next()).await {
            Ok(Some(Ok(chunk))) => chunk,
            Ok(_) => return End::Closed,
            Err(_) => return End::Stalled,
        };
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let Some(poke) = poke_in(&line) else { continue };
            if poke != Poke::Backup {
                let Some(stack) = stack.upgrade() else {
                    return End::Closed;
                };
                stack.lock().await.row_subscription.forget_devices();
            }
            refresh.notify_one();
        }
        if pending.len() > MAX_LINE_BYTES {
            return End::Closed;
        }
    }
}

/// The poke in one SSE line, if it is a `data:` line carrying one.
fn poke_in(line: &[u8]) -> Option<Poke> {
    let json = line.strip_prefix(b"data:")?;
    serde_json::from_slice::<row_stream::Frame>(json)
        .ok()?
        .poke()
}

/// Read the two snapshot routes after each poke and write the row.
///
/// `Notify` holds one permit, so a poke during a read queues exactly one
/// more read, and a burst of frames costs two. A read that keeps failing is
/// logged once, not on every retry.
async fn refresh_on_poke(
    stack: &Weak<AsyncMutex<StackRuntime>>,
    engine: &EngineAddress,
    refresh: &Notify,
) {
    let mut failing = false;
    loop {
        refresh.notified().await;
        loop {
            let (unread, backup) = futures::future::join(
                stack::fetch_unread_count(engine),
                stack::fetch_last_successful_backup(engine),
            )
            .await;
            let Some(stack) = stack.upgrade() else { return };
            let complete = unread.is_some() && backup.is_some();
            {
                let mut s = stack.lock().await;
                // `None` for a read that failed: no badge, never a stale one.
                s.last_unread = unread;
                s.last_successful_backup = backup;
                if !complete && !failing {
                    crate::log!(
                        "[Gateway] snapshot read of '{}' failed (unread count read: {}, \
                         backup line read: {}), retrying every {}s",
                        private(s.ws.id.clone()),
                        unread.is_some(),
                        backup.is_some(),
                        SNAPSHOT_RETRY.as_secs()
                    );
                }
            }
            failing = !complete;
            if complete {
                break;
            }
            tokio::time::sleep(SNAPSHOT_RETRY).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_data_line_with_an_accepted_frame_pokes() {
        let poke = |line: &str| poke_in(line.as_bytes());
        assert_eq!(
            poke("data: {\"type\":\"StreamHello\",\"data\":{}}\n"),
            Some(Poke::Resync)
        );
        assert_eq!(
            poke("data: {\"type\":\"Lagged\",\"data\":{\"count\":3}}\n"),
            Some(Poke::Resync)
        );
        assert_eq!(
            poke("data: {\"type\":\"NotificationRead\",\"aggregate\":\"notification\"}\n"),
            Some(Poke::Unread)
        );
        assert_eq!(
            poke("data: {\"type\":\"BackupCompleted\",\"aggregate\":\"ops\"}\n"),
            Some(Poke::Backup)
        );
        assert_eq!(
            poke("data: {\"type\":\"ThreadsDeleted\",\"aggregate\":\"ops\"}\n"),
            Some(Poke::Unread)
        );
        for ignored in [
            "event: keepalive\n",
            "data:\n",
            "\n",
            "data: not json\n",
            "data: {\"type\":\"ThreadEvent\",\"data\":{}}\n",
            "data: {\"type\":\"NativePushRequested\",\"aggregate\":\"notification\"}\n",
        ] {
            assert_eq!(poke(ignored), None, "{ignored}");
        }
    }
}
