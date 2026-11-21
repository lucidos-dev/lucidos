//! Fault sources classify locally. No provider/network/error-text heuristic feeds
//! this signal. Pending pre-DB signals survive an engine exit in a bounded inbox.
use crate::engine::event_bus::EventBus;
pub use lucidos_diagnostic_log::fault::{FaultKind, Inbox};
use std::path::Path;
use std::sync::OnceLock;

type Error = Box<dyn std::error::Error + Send + Sync>;
static INBOX: OnceLock<Inbox> = OnceLock::new();
type Barrier = tokio::sync::oneshot::Sender<Result<(), String>>;
static WORKER: OnceLock<tokio::sync::mpsc::Sender<Barrier>> = OnceLock::new();

/// Install before constructing the engine, so pre-DB panics survive too.
pub fn install(workspace: &Path) {
    if INBOX.set(Inbox::new(workspace)).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record_pending(FaultKind::Panic);
        previous(info);
    }));
}

pub fn record_pending(kind: FaultKind) {
    if let Some(inbox) = INBOX.get() {
        if let Err(e) = inbox.record(kind) {
            crate::log_legacy!("[InternalFault] could not persist {}: {}", kind.as_str(), e);
        }
    }
}

/// A single worker retries failed emits. Never acknowledge before commit.
pub async fn drain(inbox: &Inbox, bus: &EventBus) -> Result<(), Error> {
    for (kind, id) in inbox.pending()? {
        bus.emit_internal_fault(id, kind).await?;
        inbox.acknowledge(kind)?;
    }
    Ok(())
}

/// Ask the sole drain consumer to commit pending markers before a snapshot or
/// successful-send boundary. A failed drain must never masquerade as no faults.
pub async fn synchronize() -> Result<(), Error> {
    let worker = WORKER.get().ok_or("Fault worker not started")?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (tx, rx) = tokio::sync::oneshot::channel();
        worker.send(tx).await.map_err(|_| "Fault worker stopped")?;
        rx.await
            .map_err(|_| "Fault worker stopped")?
            .map_err(Error::from)
    })
    .await
    .map_err(|_| "Fault worker timed out")?
}

pub fn start(workspace: &Path, bus: EventBus) {
    let inbox = Inbox::new(workspace);
    let (tx, rx) = tokio::sync::mpsc::channel::<Barrier>(8);
    if WORKER.set(tx).is_err() {
        return;
    }
    tokio::spawn(run_worker(inbox, bus, rx));
}

async fn run_worker(inbox: Inbox, bus: EventBus, mut rx: tokio::sync::mpsc::Receiver<Barrier>) {
    let mut ticks = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        let barrier = tokio::select! {
            _ = ticks.tick() => None,
            request = rx.recv() => match request { Some(request) => Some(request), None => break },
        };
        let result = drain(&inbox, &bus).await;
        if let Err(e) = &result {
            crate::log_legacy!("[InternalFault] inbox drain failed: {}", e);
        }
        if let Some(barrier) = barrier {
            // A timed-out requester may disappear; the durable drain still
            // completed and its result has already been logged above.
            let _ = barrier.send(result.map_err(|e| e.to_string()));
        }
    }
}

pub async fn count(bus: &EventBus) -> Result<i64, Error> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(DISTINCT payload->'data'->>'kind') FROM events
         WHERE event_type = 'InternalFaultRecorded' AND sequence >
         (SELECT COALESCE(MAX(sequence), 0) FROM events WHERE event_type = 'DiagnosticBundleSent')",
    )
    .fetch_one(bus.pool())
    .await?)
}

#[cfg(test)]
#[path = "internal_faults_tests.rs"]
mod tests;
