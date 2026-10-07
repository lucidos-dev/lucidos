//! *Memory views*: what a Tree turn preloads of the two summary trees.
//!
//! The workspace view is shared as a *view snapshot*: a fold frozen at one
//! log length, byte for byte the same for every thread of one budget (I5).
//! Workspace leaves newer than the snapshot ride in a short recent block, and
//! the snapshot rolls over once that block passes its cap. So another
//! thread's settled turn costs this thread a few recent lines, never its
//! cached prefix.
//!
//! The thread view is a fold over the thread's own tree, with each unbuilt
//! leaf read from its event. A turn never waits on the compactor (I7). Its
//! cache marks sit at fixed offsets, so consecutive turns read its stable
//! start from the prompt cache.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use sqlx::PgPool;
use uuid::Uuid;

use super::fold::{built_prefix_end, fold};
use super::log::{EntryKind, ThreadKind};
use super::store::{self, StoredNode};
use super::{NodeAddr, SummaryScope};

type StoreResult<T> = Result<T, sqlx::Error>;

/// The recent block's cap. A workspace view of `budget` bytes gives the
/// recent block at most half of it. A thread view keeps as much free for the
/// entries its built prefix does not reach yet.
pub(crate) const RECENT_BYTES: usize = 8 * 1024;

/// Where a thread view's cache marks fall, as fractions of its prefix budget:
/// the last line end at or before each. OptChat's marks at 50k and 80k of
/// 128k. The prefix budget is what a caught-up view fills.
const THREAD_MARKS: [(usize, usize); 2] = [(50, 128), (80, 128)];

pub(crate) const WORKSPACE_VIEW_OPEN: &str = "[WORKSPACE MEMORY VIEW]\n\
Summary lines over every thread turn and artifact write in this workspace, oldest first. \
Older lines cover more. Each line starts with its id: open one with the recall tool's zoom.\n";
pub(crate) const WORKSPACE_VIEW_CLOSE: &str = "[END WORKSPACE MEMORY VIEW]\n";
const RECENT_OPEN: &str = "[RECENT WORKSPACE ENTRIES]\n\
Workspace lines newer than the view above, oldest first.\n";
const RECENT_CLOSE: &str = "[END RECENT WORKSPACE ENTRIES]";
const THREAD_VIEW_OPEN: &str = "[THREAD MEMORY VIEW]\n\
This thread so far, oldest first: recent entries verbatim, older ones as summary lines. \
An id here names a node of this thread's tree.\n";
const THREAD_VIEW_CLOSE: &str = "[END THREAD MEMORY VIEW]";

/// A node's address across trees: `w/start+span` for the workspace tree,
/// `<thread id>/start+span` for a thread's. A bare `start+span` names the
/// calling thread's tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NodeId {
    pub(crate) scope: SummaryScope,
    pub(crate) addr: NodeAddr,
}

impl NodeId {
    pub(crate) fn parse(text: &str, caller: Option<Uuid>) -> Result<Self, String> {
        let text = text.trim().trim_start_matches('[').trim_end_matches(']');
        let (scope, addr) = match text.rsplit_once('/') {
            Some(("w", addr)) => (SummaryScope::Workspace, addr),
            Some((thread, addr)) => {
                let id = Uuid::parse_str(thread)
                    .map_err(|_| format!("'{thread}' is neither 'w' nor a thread id"))?;
                (SummaryScope::Thread(id), addr)
            }
            None => {
                let caller = caller.ok_or_else(|| {
                    format!("'{text}' names no tree. Write w/{text} or <thread id>/{text}")
                })?;
                (SummaryScope::Thread(caller), text)
            }
        };
        let (start, span) = addr
            .split_once('+')
            .and_then(|(s, n)| Some((s.parse::<u64>().ok()?, n.parse::<u64>().ok()?)))
            .ok_or_else(|| format!("'{addr}' is not start+span"))?;
        if span == 0
            || !span.is_power_of_two()
            || start % span != 0
            || start.checked_add(span).is_none()
        {
            return Err(format!(
                "'{addr}' is no node: span is a power of 2 and divides start"
            ));
        }
        Ok(Self {
            scope,
            addr: NodeAddr { start, span },
        })
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.scope {
            SummaryScope::Workspace => write!(f, "w/{}", self.addr),
            SummaryScope::Thread(id) => write!(f, "{id}/{}", self.addr),
        }
    }
}

/// One view line: `[id] text`, continuation lines indented by two spaces, so
/// every line of a view ends at a line end.
pub(crate) fn render_line(id: &str, text: &str) -> String {
    format!("[{id}] {}\n", text.trim_end().replace('\n', "\n  "))
}

/// What [`render_line`] costs, from the text's indented size.
fn line_bytes(id: &str, indented_text: usize) -> usize {
    id.len() + 3 + indented_text + 1
}

fn indented_len(text: &str) -> usize {
    let text = text.trim_end();
    text.len() + 2 * text.matches('\n').count()
}

/// One turn's workspace view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceView {
    /// The snapshot, framed. Shared byte for byte within its epoch.
    pub(crate) snapshot: String,
    /// Leaves newer than the snapshot, framed, or empty.
    pub(crate) recent: String,
}

struct Snapshot {
    epoch: u64,
    /// The log length the snapshot folds.
    end: u64,
    /// Leaf `end - 1`'s event. A delete before `end` moves it, which retires
    /// the snapshot.
    last_event: Option<Uuid>,
    text: String,
}

/// The live view snapshots, one per workspace view budget.
///
/// Held in memory as a cache: a restart folds afresh, which costs one cache
/// miss and loses nothing a turn needs.
#[derive(Default)]
pub(crate) struct ViewSnapshots {
    snapshots: tokio::sync::Mutex<HashMap<usize, Snapshot>>,
    epochs: AtomicU64,
}

impl ViewSnapshots {
    /// The workspace view at `budget` bytes, snapshot and recent block
    /// together. Only built leaves show, in order, so a line the compactor has
    /// not written yet waits for the next turn rather than holding this one.
    pub(crate) async fn workspace_view(
        &self,
        pool: &PgPool,
        budget: usize,
    ) -> StoreResult<WorkspaceView> {
        if budget == 0 {
            return Ok(WorkspaceView::default());
        }
        let recent_cap = RECENT_BYTES.min(budget / 2);
        let mut snapshots = self.snapshots.lock().await;

        if let Some(snapshot) = snapshots.get(&budget) {
            if self.still_holds(pool, snapshot).await? {
                let recent = frame_recent(&recent_lines(pool, snapshot.end).await?);
                if recent.len() <= recent_cap {
                    return Ok(WorkspaceView {
                        snapshot: snapshot.text.clone(),
                        recent,
                    });
                }
            }
        }

        let snapshot = self.fold_snapshot(pool, budget - recent_cap).await?;
        log!(
            "[SummaryTree] Workspace view snapshot epoch {} at {} leaves, {} bytes (budget {})",
            snapshot.epoch,
            snapshot.end,
            snapshot.text.len(),
            budget
        );
        let view = WorkspaceView {
            snapshot: snapshot.text.clone(),
            recent: String::new(),
        };
        snapshots.insert(budget, snapshot);
        Ok(view)
    }

    async fn still_holds(&self, pool: &PgPool, snapshot: &Snapshot) -> StoreResult<bool> {
        let Some(last) = snapshot.end.checked_sub(1) else {
            return Ok(true);
        };
        let leaf = store::nodes_at(pool, SummaryScope::Workspace, &[NodeAddr::leaf(last)]).await?;
        Ok(leaf
            .get(&NodeAddr::leaf(last))
            .and_then(|n| n.source_event_id)
            == snapshot.last_event)
    }

    async fn fold_snapshot(&self, pool: &PgPool, budget: usize) -> StoreResult<Snapshot> {
        let shapes = store::node_shapes(pool, SummaryScope::Workspace).await?;
        let end = (0..)
            .find(|i| !shapes.contains_key(&NodeAddr::leaf(*i)))
            .unwrap_or(0);
        let id = |a: NodeAddr| format!("w/{a}");
        let frame = WORKSPACE_VIEW_OPEN.len() + WORKSPACE_VIEW_CLOSE.len();
        let tiles = fold(
            end,
            built_prefix_end(end, |a| shapes.contains_key(&a)),
            |a| shapes.get(&a).map(|s| line_bytes(&id(a), s.bytes)),
            budget.saturating_sub(frame),
            0,
        );
        let nodes = store::nodes_at(pool, SummaryScope::Workspace, &tiles).await?;
        let mut text = String::new();
        if !tiles.is_empty() {
            text.push_str(WORKSPACE_VIEW_OPEN);
            for tile in &tiles {
                if let Some(node) = nodes.get(tile) {
                    text.push_str(&render_line(&id(*tile), &node.text));
                }
            }
            text.push_str(WORKSPACE_VIEW_CLOSE);
        }
        let last_event = end
            .checked_sub(1)
            .and_then(|i| shapes.get(&NodeAddr::leaf(i)))
            .and_then(|s| s.source_event_id);
        Ok(Snapshot {
            epoch: self.epochs.fetch_add(1, Ordering::Relaxed) + 1,
            end,
            last_event,
            text,
        })
    }
}

/// The built workspace leaves from `from` on, contiguous, rendered.
async fn recent_lines(pool: &PgPool, from: u64) -> StoreResult<String> {
    let mut out = String::new();
    let mut expect = from;
    for (addr, node) in store::leaves_from(pool, SummaryScope::Workspace, from).await? {
        if addr.start != expect {
            break;
        }
        out.push_str(&render_line(&format!("w/{addr}"), &node.text));
        expect += 1;
    }
    Ok(out)
}

/// A coding agent's view section: the workspace view, then how to open it
/// through the `lucidos` CLI. Empty when the view is.
pub(crate) fn coding_agent_section(view: &WorkspaceView) -> String {
    if view.snapshot.is_empty() && view.recent.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n");
    out.push_str(&view.snapshot);
    if !view.recent.is_empty() {
        out.push_str(&view.recent);
        out.push('\n');
    }
    out.push_str(CODING_AGENT_RECALL);
    out
}

const CODING_AGENT_RECALL: &str = "Open a line of the view with \
`lucidos recall zoom --id <id>`, down to the exact message. `lucidos recall find --query <what>` \
walks the tree for a topic, `lucidos recall search --text <words>` finds exact words, and \
`lucidos recall date --id <id>` says when a line's entries happened.\n";

fn frame_recent(lines: &str) -> String {
    if lines.is_empty() {
        return String::new();
    }
    format!("{RECENT_OPEN}{lines}{RECENT_CLOSE}")
}

/// A thread memory view and where its cache marks fall.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ThreadView {
    /// The view, framed, or empty.
    pub(crate) text: String,
    /// Byte offsets into `text`, ascending, each just past a line's newline.
    pub(crate) marks: Vec<usize>,
}

impl ThreadView {
    /// The text cut at its marks: one piece ending at each mark, then the rest.
    pub(crate) fn pieces(&self) -> Vec<&str> {
        let mut pieces = Vec::with_capacity(self.marks.len() + 1);
        let mut from = 0;
        for &mark in &self.marks {
            pieces.push(&self.text[from..mark]);
            from = mark;
        }
        pieces.push(&self.text[from..]);
        pieces
    }
}

/// A Lucidos Agent thread's memory view at `budget` bytes, or empty for a
/// thread with no past. `current_message` is the turn's own message, already
/// in the log, which the view leaves to the request line.
pub(crate) async fn thread_view(
    pool: &PgPool,
    thread_id: Uuid,
    budget: usize,
    current_message: &str,
) -> StoreResult<ThreadView> {
    if budget == 0 {
        return Ok(ThreadView::default());
    }
    let mut log = store::load_thread_log_now(pool, thread_id, ThreadKind::LucidosAgent).await?;
    // The turn's own message, with its image count, or a trigger's `[name
    // fired] prompt`. Matched whole, so a side question never hides an entry
    // it only resembles.
    let current = current_message.trim();
    if !current.is_empty()
        && log.entries.last().is_some_and(|e| {
            let text = e.text.trim();
            e.kind == EntryKind::User
                && (text == current
                    || text.starts_with(&format!("{current} ["))
                    || text.ends_with(&format!("] {current}")))
        })
    {
        log.entries.pop();
    }
    let nodes = store::load_nodes(pool, SummaryScope::Thread(thread_id)).await?;
    Ok(render_thread_view(&log.entries, &nodes, budget))
}

/// [`thread_view`] over a loaded log and tree.
pub(crate) fn render_thread_view(
    entries: &[super::log::LogEntry],
    nodes: &HashMap<NodeAddr, StoredNode>,
    budget: usize,
) -> ThreadView {
    let lines = thread_lines(entries, nodes);
    let trusted = trusted_end(entries, nodes);
    let len = entries.len() as u64;
    let (fold_budget, reserve) = thread_budgets(budget);
    let tiles = fold(
        len,
        built_prefix_end(len, |a| a.end() <= trusted && nodes.contains_key(&a)),
        |a| {
            lines
                .get(&a)
                .map(|text| line_bytes(&a.to_string(), indented_len(text)))
        },
        fold_budget,
        reserve,
    );
    if tiles.is_empty() {
        return ThreadView::default();
    }
    let mut text = String::from(THREAD_VIEW_OPEN);
    let mut line_ends = Vec::with_capacity(tiles.len());
    for tile in &tiles {
        text.push_str(&render_line(&tile.to_string(), &lines[tile]));
        line_ends.push(text.len());
    }
    text.push_str(THREAD_VIEW_CLOSE);
    let mut marks: Vec<usize> = thread_mark_offsets(budget)
        .into_iter()
        .filter(|&at| at < text.len())
        .filter_map(|at| {
            line_ends
                .iter()
                .take_while(|&&end| end <= at)
                .last()
                .copied()
        })
        .collect();
    marks.dedup();
    ThreadView { text, marks }
}

/// A thread view's fold budget and the reserve it keeps for entries past the built prefix.
fn thread_budgets(budget: usize) -> (usize, usize) {
    let fold_budget = budget.saturating_sub(THREAD_VIEW_OPEN.len() + THREAD_VIEW_CLOSE.len());
    (fold_budget, RECENT_BYTES.min(fold_budget / 2))
}

/// The offsets a thread view of `budget` bytes puts its cache marks at or
/// before, ascending.
pub(crate) fn thread_mark_offsets(budget: usize) -> Vec<usize> {
    let (fold_budget, reserve) = thread_budgets(budget);
    THREAD_MARKS
        .iter()
        .map(|(part, whole)| (fold_budget - reserve) * part / whole)
        .collect()
}

/// The lines a thread view may show: each built node whose leaves still
/// match the log, and every leaf, read from its event when no node matches.
///
/// A node built before a position shift is trusted only below the first leaf
/// whose event moved, since merges past it summarise the old order.
pub(crate) fn thread_lines(
    entries: &[super::log::LogEntry],
    nodes: &HashMap<NodeAddr, StoredNode>,
) -> HashMap<NodeAddr, String> {
    let trusted_end = trusted_end(entries, nodes);
    let mut lines: HashMap<NodeAddr, String> = nodes
        .iter()
        .filter(|(a, _)| a.end() <= trusted_end)
        .map(|(a, n)| (*a, n.text.clone()))
        .collect();
    for (i, entry) in entries.iter().enumerate() {
        lines
            .entry(NodeAddr::leaf(i as u64))
            .or_insert_with(|| entry.raw_line());
    }
    lines
}

/// The first leaf whose built node no longer matches its event, or the log's
/// length when every built leaf does.
fn trusted_end(entries: &[super::log::LogEntry], nodes: &HashMap<NodeAddr, StoredNode>) -> u64 {
    (0..entries.len())
        .find(|&i| {
            nodes
                .get(&NodeAddr::leaf(i as u64))
                .map(|n| n.source_event_id)
                != Some(Some(entries[i].event_id))
        })
        .unwrap_or(entries.len()) as u64
}
