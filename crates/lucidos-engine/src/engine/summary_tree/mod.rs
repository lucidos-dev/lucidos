mod breaker;
mod compaction_view;
mod compactor;
pub(crate) mod compactor_models;
mod consumer;
pub(crate) mod estimate;
pub mod fold;
mod limit;
pub(crate) mod log;
pub mod module;
mod prompt;
pub mod recall;
mod shape;
pub mod spend;
pub mod store;
pub mod view;
pub(crate) mod workspace_log;

pub use compactor_models::CompactorSelection;
pub use compactor_models::COMPACTOR_DEFAULTS;
pub use consumer::spawn;
pub use estimate::{Bounds, EffortCost, ModelCost, TreeBackfillEstimate};
pub use module::{tree_ready, BackfillProgress, TreeBackfill};
pub use spend::StartEstimate;
pub use store::caught_up;
pub(crate) use {compactor_models::compactor_selection, compactor_models::default_effort_for};

pub(crate) const NODE_BYTES: usize = 512;

const LIVE_DRAINS: usize = 4;

/// INVARIANT: Drains reading the database at once. The engine's pool serves user
/// requests too, so the compactor's reads take a few connections at most.
const READS: usize = 8;

const READY_DAYS: i32 = 7;

const TRIES: usize = 5;

/// INVARIANT: Wait before a failed node is tried again. Short on purpose: OptChat warns
/// that a long backoff starves whatever waits on the summaries.
const RETRY: std::time::Duration = std::time::Duration::from_secs(10);

const CAP_CHARS: usize = 30_000;

const RAW_CONTEXT_BYTES: usize = 16_000;

/// INVARIANT: Events younger than this are left for the next drain. An event's sequence
/// is assigned at insert, so a slow transaction can commit a lower sequence
/// after a higher one. Waiting this long keeps positions from shifting.
const HORIZON_SECS: i64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SummaryScope {
    Workspace,
    Thread(uuid::Uuid),
}

impl SummaryScope {
    const WORKSPACE: &'static str = "workspace";

    pub(crate) fn as_db(&self) -> String {
        match self {
            Self::Workspace => Self::WORKSPACE.to_string(),
            Self::Thread(id) => id.to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeAddr {
    pub start: u64,
    pub span: u64,
}

impl NodeAddr {
    pub fn leaf(index: u64) -> Self {
        Self {
            start: index,
            span: 1,
        }
    }

    pub(crate) fn end(&self) -> u64 {
        self.start + self.span
    }

    pub fn is_leaf(&self) -> bool {
        self.span == 1
    }

    pub fn children(&self) -> Option<(NodeAddr, NodeAddr)> {
        if self.is_leaf() {
            return None;
        }
        let half = self.span / 2;
        Some((
            NodeAddr {
                start: self.start,
                span: half,
            },
            NodeAddr {
                start: self.start + half,
                span: half,
            },
        ))
    }
}

impl std::fmt::Display for NodeAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}+{}", self.start, self.span)
    }
}

pub async fn tree_backfill(
    engine: &crate::engine::LucidosEngine,
) -> Result<TreeBackfill, Box<dyn std::error::Error + Send + Sync>> {
    module::tree_backfill(engine.pool(), engine.summary_tree()).await
}

pub async fn tree_backfill_estimate(
    engine: &crate::engine::LucidosEngine,
) -> Result<TreeBackfillEstimate, sqlx::Error> {
    let pool = engine.pool();
    let (counts, measured) =
        tokio::try_join!(estimate::counts(pool), estimate::measured_for_catalog(pool),)?;
    let configured = engine.configured_provider_kinds();
    Ok(estimate::estimate(
        &counts,
        &measured,
        engine.model_registry(),
        &configured,
    ))
}

#[derive(Default)]
pub struct Runtime {
    compactor: std::sync::OnceLock<std::sync::Arc<compactor::Compactor>>,
}

impl Runtime {
    fn set(&self, compactor: std::sync::Arc<compactor::Compactor>) {
        if self.compactor.set(compactor).is_err() {
            log_legacy!(
                "[SummaryTree] A second compactor started; the first stays the one routes read"
            );
        }
    }

    pub fn backfill_progress(&self) -> Option<BackfillProgress> {
        self.compactor.get()?.backfill_progress()
    }
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
