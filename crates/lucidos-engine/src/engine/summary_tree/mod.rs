//! *Summary trees* and the *compactor* (ADR 0362).
//!
//! A summary tree is a binary tree of one-line summaries over a log. The log is
//! the events table, read through the projection in [`log`], so the nodes in
//! `summary_tree_nodes` are the only new storage. Every node is a projection
//! the compactor can rebuild from events (invariant I1).
//!
//! Each in-scope thread has a tree over its own entries. The workspace has one
//! tree whose leaves are settled thread turns and artifact writes
//! ([`workspace_log`]). The design follows the OptChat spec: 512-byte lines, a
//! short message is its own node, a thread's leaves build a few at a time, and
//! a merge starts only once both children are built. Each compactor call reads
//! a *compaction view* ([`compaction_view`]). Phase decisions live in
//! `docs/plans/2026-10-04-tree-memory-module-and-the-home-thread.md`, the
//! backfill's speed in `docs/plans/2026-10-06-tree-backfill-in-hours.md`, and
//! the compactor's cache and prompt in
//! `docs/plans/2026-10-08-tree-compactor-follows-optchat-revision-3c190e06.md`.
//!
//! A workspace on the Tree *memory module* reads them ([`module`]): each turn
//! preloads two *memory views* ([`view`]), and the recall tools open any line
//! down to its event ([`recall`]). The compactor runs for exactly those
//! workspaces ([`consumer`]).

mod compaction_view;
mod compactor;
pub(crate) mod compactor_models;
mod consumer;
pub(crate) mod estimate;
pub(crate) mod fold;
mod limit;
pub(crate) mod log;
pub(crate) mod module;
mod prompt;
pub(crate) mod recall;
mod shape;
pub(crate) mod store;
pub(crate) mod view;
pub(crate) mod workspace_log;

pub use compactor_models::CompactorSelection;
pub(crate) use compactor_models::{compactor_selection, default_effort_for, COMPACTOR_DEFAULTS};
pub use consumer::spawn;
pub use estimate::{Bounds, EffortCost, ModelCost, TreeBackfillEstimate};
pub use module::{tree_ready, BackfillProgress, TreeBackfill};
pub use store::caught_up;

/// Target size of one summary line, in UTF-8 bytes.
pub(crate) const NODE_BYTES: usize = 512;

/// Workers that drain only scopes a live event marked, so a live event never
/// waits for a backfill drain to end.
const LIVE_DRAINS: usize = 4;

/// Drains reading the database at once. The engine's pool serves user
/// requests too, so the compactor's reads take a few connections at most.
const READS: usize = 8;

/// A thread active in this many days must have its tree before the workspace
/// is ready. Older threads fill in after.
const READY_DAYS: i32 = 7;

/// Attempts per node to get a line under [`NODE_BYTES`].
const TRIES: usize = 5;

/// Wait before a failed node is tried again. Short on purpose: OptChat warns
/// that a long backoff starves whatever waits on the summaries.
const RETRY: std::time::Duration = std::time::Duration::from_secs(10);

/// Longest tool call or tool result an entry carries, head and tail kept.
const CAP_CHARS: usize = 30_000;

/// Most bytes of its thread's raw entries a workspace turn leaf reads as
/// context.
const RAW_CONTEXT_BYTES: usize = 16_000;

/// Events younger than this are left for the next drain. An event's sequence
/// is assigned at insert, so a slow transaction can commit a lower sequence
/// after a higher one. Waiting this long keeps positions from shifting.
const HORIZON_SECS: i64 = 2;

/// Which log a tree summarizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SummaryScope {
    Workspace,
    Thread(uuid::Uuid),
}

impl SummaryScope {
    const WORKSPACE: &'static str = "workspace";

    /// The `scope` column's spelling.
    pub(crate) fn as_db(&self) -> String {
        match self {
            Self::Workspace => Self::WORKSPACE.to_string(),
            Self::Thread(id) => id.to_string(),
        }
    }
}

/// A node's address: the entries `[start, start + span)`, `span` a power of 2
/// and `start` a multiple of it. Rendered `start+span`, as OptChat names lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct NodeAddr {
    pub(crate) start: u64,
    pub(crate) span: u64,
}

impl NodeAddr {
    pub(crate) fn leaf(index: u64) -> Self {
        Self {
            start: index,
            span: 1,
        }
    }

    pub(crate) fn end(&self) -> u64 {
        self.start + self.span
    }

    pub(crate) fn is_leaf(&self) -> bool {
        self.span == 1
    }

    /// The two halves a merge is built from. `None` for a leaf.
    pub(crate) fn children(&self) -> Option<(NodeAddr, NodeAddr)> {
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

/// The engine's Tree backfill state, as `GET /api/v1/memory/tree-backfill`
/// serves it.
pub(crate) async fn tree_backfill(
    engine: &crate::engine::LucidosEngine,
) -> Result<TreeBackfill, Box<dyn std::error::Error + Send + Sync>> {
    module::tree_backfill(engine.pool(), engine.summary_tree()).await
}

/// What a Tree backfill of this workspace would cost, as
/// `GET /api/v1/memory/tree-backfill/estimate` serves it.
pub(crate) async fn tree_backfill_estimate(
    engine: &crate::engine::LucidosEngine,
) -> Result<TreeBackfillEstimate, sqlx::Error> {
    let pool = engine.pool();
    // Independent reads against the same pool. The counts scan dominates,
    // and the index-served measured-usage read hides beside it.
    let (counts, measured) =
        tokio::try_join!(estimate::counts(pool), estimate::measured_for_catalog(pool),)?;
    let configured = consumer::configured_providers(engine);
    Ok(estimate::estimate(
        &counts,
        &measured,
        engine.model_registry(),
        &configured,
    ))
}

/// The engine's handle on its one compactor, set the first time the workspace
/// goes on Tree. Routes read the backfill count through it.
#[derive(Default)]
pub(crate) struct Runtime {
    compactor: std::sync::OnceLock<std::sync::Arc<compactor::Compactor>>,
}

impl Runtime {
    fn set(&self, compactor: std::sync::Arc<compactor::Compactor>) {
        if self.compactor.set(compactor).is_err() {
            log!("[SummaryTree] A second compactor started; the first stays the one routes read");
        }
    }

    /// The running backfill's count, or `None` when no backfill runs.
    pub(crate) fn backfill_progress(&self) -> Option<BackfillProgress> {
        self.compactor.get()?.backfill_progress()
    }
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
