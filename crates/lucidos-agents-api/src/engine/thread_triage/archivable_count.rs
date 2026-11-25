//! The *archivable sub-thread count* on each thread summary: how many
//! sub-threads the overflow menu's "Archive N completed sub-threads" puts
//! away.
//!
//! The count is exactly the press's `safe_thread_count`
//! ([`completed_preflight`] over [`TriageScope::Descendants`]), so the menu
//! and the press cannot disagree. It is a family property, so every recount
//! is a full recount, never a delta.
//!
//! The projection samples the archive inputs before and after each event.
//! It sends the rows whose count may have moved on a directed channel
//! ([`EventBus::take_archive_recounts`]). This worker drains it after commit,
//! recounts each row and its ancestors, and stores the counts through the bus,
//! which rebroadcasts every row that moved. It first recounts every family
//! once, which fills the column on upgrade and heals any drift a crash left.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use super::archive_all::completed_preflight;
use super::facts::{load, TriageScope};
use crate::engine::event_bus::EventBus;

/// `parent`'s archivable sub-thread count, by the press's own rule.
pub(crate) async fn count_archivable_sub_threads(
    pool: &PgPool,
    parent: Uuid,
) -> Result<i64, sqlx::Error> {
    let rows = load(pool, TriageScope::Descendants(parent)).await?;
    Ok(completed_preflight(&rows).safe_thread_count as i64)
}

/// `anchors` and every ancestor of each. Plain `UNION`, so a cycle in
/// `parent_thread_id` terminates.
async fn with_ancestors(pool: &PgPool, anchors: &[Uuid]) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "WITH RECURSIVE up AS ( \
             SELECT thread_id, parent_thread_id FROM thread_summaries \
             WHERE thread_id = ANY($1) \
             UNION \
             SELECT p.thread_id, p.parent_thread_id FROM thread_summaries p \
             JOIN up ON p.thread_id = up.parent_thread_id \
         ) SELECT thread_id FROM up",
    )
    .bind(anchors)
    .fetch_all(pool)
    .await
}

/// Recount `anchors` and their ancestors, store the counts, and return the
/// rows that moved.
pub(crate) async fn recount_from(
    pool: &PgPool,
    bus: &EventBus,
    anchors: &[Uuid],
) -> Result<Vec<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    let mut counts = Vec::new();
    for id in with_ancestors(pool, anchors).await? {
        counts.push((id, count_archivable_sub_threads(pool, id).await?));
    }
    bus.store_archivable_sub_thread_counts(&counts).await
}

/// Recount every thread that has sub-threads or holds a nonzero count.
pub(crate) async fn recount_all(
    pool: &PgPool,
    bus: &EventBus,
) -> Result<Vec<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    let parents: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT parent_thread_id FROM thread_summaries \
         WHERE parent_thread_id IS NOT NULL \
         UNION SELECT thread_id FROM thread_summaries WHERE archivable_sub_thread_count <> 0",
    )
    .fetch_all(pool)
    .await?;
    let mut counts = Vec::with_capacity(parents.len());
    for id in parents {
        counts.push((id, count_archivable_sub_threads(pool, id).await?));
    }
    bus.store_archivable_sub_thread_counts(&counts).await
}

/// The one consumer of the bus's archive recount requests.
pub struct ArchivableCountWorker;

impl ArchivableCountWorker {
    /// Take the bus's recount requests and run the worker on a tokio task.
    /// Boot does not wait for it: the first full recount runs on the task.
    ///
    /// Panics when the requests were already taken: two workers would store
    /// counts in no defined order.
    pub fn spawn(pool: PgPool, bus: Arc<EventBus>) -> tokio::task::JoinHandle<()> {
        let requests = bus
            .take_archive_recounts()
            .expect("one archivable count worker per event bus");
        tokio::spawn(run(pool, bus, requests))
    }
}

const RETRY_FLOOR: Duration = Duration::from_secs(5);
const RETRY_CEILING: Duration = Duration::from_secs(300);

/// Recount every family once, then each batch of requests in arrival order.
/// One task stores every count, so a later recount never loses to an earlier.
///
/// A failed recount keeps its work owed and retries with backoff. No later
/// request is guaranteed to cover a settled family, so dropping it would
/// leave the count wrong until the next restart.
async fn run(pool: PgPool, bus: Arc<EventBus>, mut requests: UnboundedReceiver<Uuid>) {
    let mut full_recount_owed = true;
    let mut pending: HashSet<Uuid> = HashSet::new();
    let mut retry_delay = RETRY_FLOOR;
    loop {
        if !full_recount_owed && pending.is_empty() {
            let Some(first) = requests.recv().await else {
                return;
            };
            pending.insert(first);
        }
        while let Ok(more) = requests.try_recv() {
            pending.insert(more);
        }
        let result = if full_recount_owed {
            recount_all(&pool, &bus).await
        } else {
            let anchors: Vec<Uuid> = pending.iter().copied().collect();
            recount_from(&pool, &bus, &anchors).await
        };
        match result {
            Ok(_) if full_recount_owed => full_recount_owed = false,
            Ok(_) => pending.clear(),
            Err(e) => {
                log_legacy!(
                    "[ArchivableCount] Recount failed, retrying in {:?}: {}",
                    retry_delay,
                    e
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(RETRY_CEILING);
                continue;
            }
        }
        retry_delay = RETRY_FLOOR;
    }
}

#[cfg(test)]
#[path = "archivable_count_tests.rs"]
mod tests;
