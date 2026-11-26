use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use super::archive_all::completed_preflight;
use super::facts::{load, TriageScope};
use crate::engine::event_bus::EventBus;

pub(crate) async fn count_archivable_sub_threads(
    pool: &PgPool,
    parent: Uuid,
) -> Result<i64, sqlx::Error> {
    let rows = load(pool, TriageScope::Descendants(parent)).await?;
    Ok(completed_preflight(&rows).safe_thread_count as i64)
}

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

pub struct ArchivableCountWorker;

impl ArchivableCountWorker {
    pub fn spawn(pool: PgPool, bus: Arc<EventBus>) -> tokio::task::JoinHandle<()> {
        let requests = bus
            .take_archive_recounts()
            .expect("one archivable count worker per event bus");
        tokio::spawn(run(pool, bus, requests))
    }
}

const RETRY_FLOOR: Duration = Duration::from_secs(5);
const RETRY_CEILING: Duration = Duration::from_secs(300);

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
