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

const SNAPSHOT_RETRY: Duration = Duration::from_secs(2);

const MAX_LINE_BYTES: usize = 64 * 1024;

pub struct RowSubscription {
    task: Option<Task>,
    device_unread: HashMap<String, u64>,
    epoch: u64,
    refused: Option<StatusCode>,
    silence_limit: Duration,
    without_stream: WithoutStream,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum WithoutStream {
    #[default]
    No,
    Seen,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Refused(StatusCode),
    Stalled,
    Closed,
}

struct Task(tokio::task::JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl RowSubscription {
    pub fn is_live(&self) -> bool {
        self.task.as_ref().is_some_and(|t| !t.0.is_finished())
    }

    pub fn start(&mut self, stack: Weak<AsyncMutex<StackRuntime>>, engine: EngineAddress) {
        self.forget_devices();
        let silence_limit = self.silence_limit;
        self.task = Some(Task(tokio::spawn(run(stack, engine, silence_limit))));
    }

    pub fn stop(&mut self) {
        self.task = None;
        self.forget_devices();
    }

    pub fn note_engine_stream(&mut self, offered: bool) -> bool {
        let first_sight = !offered && self.without_stream == WithoutStream::No;
        if offered {
            self.without_stream = WithoutStream::No;
        } else if first_sight {
            self.without_stream = WithoutStream::Seen;
        }
        first_sight
    }

    pub fn engine_lacks_stream(&self) -> bool {
        self.without_stream != WithoutStream::No
    }

    pub fn stream_respawn_owed(&self) -> bool {
        self.without_stream == WithoutStream::Seen
    }

    pub fn note_stream_respawn(&mut self) {
        self.without_stream = WithoutStream::Respawned;
    }

    pub fn forget_devices(&mut self) {
        self.device_unread.clear();
        self.epoch += 1;
    }

    pub fn device_unread(&self, device_id: &str) -> Option<(Option<u64>, u64)> {
        self.is_live()
            .then(|| (self.device_unread.get(device_id).copied(), self.epoch))
    }

    pub fn remember_device(&mut self, epoch: u64, device_id: &str, count: u64) {
        if self.is_live() && epoch == self.epoch {
            self.device_unread.insert(device_id.to_string(), count);
        }
    }

    #[cfg(test)]
    pub fn set_silence_limit_for_test(&mut self, limit: Duration) {
        self.silence_limit = limit;
    }

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
    s.blank_row();
}

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

fn poke_in(line: &[u8]) -> Option<Poke> {
    let json = line.strip_prefix(b"data:")?;
    serde_json::from_slice::<row_stream::Frame>(json)
        .ok()?
        .poke()
}

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
