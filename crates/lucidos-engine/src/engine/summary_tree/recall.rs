//! The recall tools' reads: `zoom`, `date` and `search` (ADR 0362). `find`
//! asks a judgment provider, so it lives with the engine
//! (`engine/tools/recall.rs`) and walks the tree through [`zoom`].
//!
//! Zoom runs straight through the tree of trees. A workspace node opens into
//! its halves, and a turn leaf into its thread's entries for that turn. A
//! thread node opens into its halves, and a thread leaf into the exact event
//! text (I3). An artifact leaf reads the file at that commit.

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::log::LogEntry;
use super::store::{self, StoredNode};
use super::view::{thread_lines, NodeId};
use super::{NodeAddr, SummaryScope};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Deepest a single zoom opens: 2^6 lines.
pub(crate) const MAX_ZOOM_LEVELS: u32 = 6;

/// Longest artifact text a zoom returns, head and tail kept.
const ARTIFACT_CHARS: usize = 30_000;

/// One line a recall tool returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RecallLine {
    pub(crate) id: String,
    pub(crate) text: String,
}

/// Open `id` `levels` levels down. A leaf opens into its source.
pub(crate) async fn zoom(
    pool: &PgPool,
    id: NodeId,
    levels: u32,
    read_artifact: &(dyn Fn(&str, &str) -> Option<String> + Sync),
) -> Result<Vec<RecallLine>, BoxError> {
    let levels = levels.clamp(1, MAX_ZOOM_LEVELS);
    match id.scope {
        SummaryScope::Thread(thread) => {
            let tree = ThreadTree::load(pool, thread).await?;
            tree.check(id.addr)?;
            if id.addr.is_leaf() {
                let entry = &tree.entries[id.addr.start as usize];
                return Ok(vec![RecallLine {
                    id: id.to_string(),
                    text: entry.message(),
                }]);
            }
            Ok(tree.lines(descendants(id.addr, levels)))
        }
        SummaryScope::Workspace => {
            check_workspace(pool, id).await?;
            if id.addr.is_leaf() {
                let nodes = store::nodes_at(pool, SummaryScope::Workspace, &[id.addr]).await?;
                let node = nodes
                    .get(&id.addr)
                    .ok_or_else(|| format!("{id} is not summarised yet. search finds its words"))?;
                return open_workspace_leaf(pool, id, node, read_artifact).await;
            }
            Ok(workspace_lines(pool, descendants(id.addr, levels)).await?)
        }
    }
}

/// The top of a tree: how many entries it holds, and the lines tiling them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TreeTop {
    pub(crate) entries: u64,
    /// The largest blocks tiling `[0, entries)`, oldest first. An unbuilt
    /// block shows as its built halves, as in [`zoom`].
    pub(crate) lines: Vec<RecallLine>,
}

/// The top of `scope`'s tree, where a browser starts before it zooms.
pub(crate) async fn top(pool: &PgPool, scope: SummaryScope) -> Result<TreeTop, BoxError> {
    match scope {
        SummaryScope::Thread(thread) => {
            let tree = ThreadTree::load(pool, thread).await?;
            let entries = tree.entries.len() as u64;
            Ok(TreeTop {
                entries,
                lines: tree.lines(aligned_blocks(0, entries)),
            })
        }
        SummaryScope::Workspace => {
            let entries = store::built_leaf_count(pool, SummaryScope::Workspace).await?;
            Ok(TreeTop {
                entries,
                lines: workspace_lines(pool, aligned_blocks(0, entries)).await?,
            })
        }
    }
}

/// Refuse a workspace node reaching past the built leaves, so an open never
/// walks a range that holds nothing.
async fn check_workspace(pool: &PgPool, id: NodeId) -> Result<(), BoxError> {
    let len = store::built_leaf_count(pool, SummaryScope::Workspace).await?;
    if id.addr.end() > len {
        return Err(
            format!("{id} is past the end of the workspace tree, which holds {len} lines").into(),
        );
    }
    Ok(())
}

/// The time range `id` covers, first entry to last.
pub(crate) async fn date(pool: &PgPool, id: NodeId) -> Result<serde_json::Value, BoxError> {
    let (from, to) = match id.scope {
        SummaryScope::Thread(thread) => {
            let tree = ThreadTree::load(pool, thread).await?;
            tree.check(id.addr)?;
            let event_ids: Vec<Uuid> = tree.entries[id.addr.start as usize..id.addr.end() as usize]
                .iter()
                .map(|e| e.event_id)
                .collect();
            store::event_span(pool, &event_ids).await?
        }
        SummaryScope::Workspace => {
            check_workspace(pool, id).await?;
            store::leaf_event_span(pool, SummaryScope::Workspace, id.addr.start, id.addr.end())
                .await?
        }
    };
    let (Some(from), Some(to)) = (from, to) else {
        return Err(format!("{id} covers no entry yet").into());
    };
    Ok(serde_json::json!({
        "id": id.to_string(),
        "from": from.to_rfc3339(),
        "to": to.to_rfc3339(),
    }))
}

/// One `search` result: a message, its address in its thread's tree, and the
/// workspace leaf of its turn once that is built.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct SearchHit {
    pub(crate) id: String,
    pub(crate) workspace_id: Option<String>,
    pub(crate) thread_id: Uuid,
    pub(crate) title: Option<String>,
    pub(crate) at: String,
    pub(crate) text: String,
}

/// Text search over the log, through the trigram index on message text,
/// returning tree addresses.
pub(crate) async fn search(
    pool: &PgPool,
    events: &crate::core::EventStore,
    text: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, BoxError> {
    let matches = events.search_message_events(text, limit as i64).await?;
    let mut trees: HashMap<Uuid, Option<ThreadTree>> = HashMap::new();
    let mut hits = Vec::with_capacity(matches.len());
    for found in matches {
        let tree = match trees.entry(found.thread_id) {
            Entry::Occupied(known) => known.into_mut(),
            Entry::Vacant(slot) => {
                slot.insert(ThreadTree::load_if_present(pool, found.thread_id).await?)
            }
        };
        let Some(tree) = tree.as_ref() else {
            continue;
        };
        let Some(index) = tree
            .entries
            .iter()
            .position(|e| e.event_id == found.event_id)
        else {
            continue;
        };
        let workspace_id = match tree.turn_of(index) {
            Some(settle) => store::turn_leaf(pool, settle)
                .await?
                .map(|start| format!("w/{}", NodeAddr::leaf(start))),
            None => None,
        };
        hits.push(SearchHit {
            id: NodeId {
                scope: SummaryScope::Thread(found.thread_id),
                addr: NodeAddr::leaf(index as u64),
            }
            .to_string(),
            workspace_id,
            thread_id: found.thread_id,
            title: tree.title.clone(),
            at: found.created.to_rfc3339(),
            text: snippet(&found.text, text),
        });
    }
    Ok(hits)
}

/// The message around the query's first word, so a long reply stays short.
fn snippet(text: &str, query: &str) -> String {
    const AROUND: usize = 160;
    let lower = text.to_lowercase();
    let at = query
        .split_whitespace()
        .find_map(|w| lower.find(&w.to_lowercase()))
        .unwrap_or(0);
    let start = text.floor_char_boundary(at.saturating_sub(AROUND));
    let end = text.floor_char_boundary((at + AROUND).min(text.len()));
    let mut out = text[start..end].trim().to_string();
    if start > 0 {
        out.insert_str(0, "… ");
    }
    if end < text.len() {
        out.push_str(" …");
    }
    out
}

/// One thread's log and the lines its tree can show.
struct ThreadTree {
    thread: Uuid,
    title: Option<String>,
    entries: Vec<LogEntry>,
    turns: Vec<super::log::Turn>,
    lines: HashMap<NodeAddr, String>,
}

impl ThreadTree {
    async fn load(pool: &PgPool, thread: Uuid) -> Result<Self, BoxError> {
        Self::load_if_present(pool, thread)
            .await?
            .ok_or_else(|| format!("no thread {thread}").into())
    }

    /// `None` for a thread that no longer exists; a failed read is an error.
    async fn load_if_present(pool: &PgPool, thread: Uuid) -> Result<Option<Self>, BoxError> {
        let Some(info) = store::thread_info(pool, thread).await? else {
            return Ok(None);
        };
        let log = store::load_thread_log_now(pool, thread, info.kind).await?;
        let nodes = store::load_nodes(pool, SummaryScope::Thread(thread)).await?;
        let lines = thread_lines(&log.entries, &nodes);
        Ok(Some(Self {
            thread,
            title: info.title,
            entries: log.entries,
            turns: log.turns,
            lines,
        }))
    }

    fn check(&self, addr: NodeAddr) -> Result<(), BoxError> {
        if addr.end() > self.entries.len() as u64 {
            return Err(format!(
                "{addr} is past the end of thread {}, which holds {} entries",
                self.thread,
                self.entries.len()
            )
            .into());
        }
        Ok(())
    }

    /// The settle event of the turn holding entry `index`.
    fn turn_of(&self, index: usize) -> Option<Uuid> {
        self.turns
            .iter()
            .find(|t| t.entries.contains(&index))
            .map(|t| t.settle_event_id)
    }

    /// Each address as its own line, or its built halves when unbuilt.
    fn lines(&self, addrs: Vec<NodeAddr>) -> Vec<RecallLine> {
        let mut out = Vec::new();
        for addr in addrs {
            self.push(addr, &mut out);
        }
        out
    }

    fn push(&self, addr: NodeAddr, out: &mut Vec<RecallLine>) {
        if let Some(text) = self.lines.get(&addr) {
            out.push(RecallLine {
                id: NodeId {
                    scope: SummaryScope::Thread(self.thread),
                    addr,
                }
                .to_string(),
                text: text.clone(),
            });
        } else if let Some((a, b)) = addr.children() {
            self.push(a, out);
            self.push(b, out);
        }
    }
}

/// The nodes `levels` levels below `addr`.
fn descendants(addr: NodeAddr, levels: u32) -> Vec<NodeAddr> {
    let span = (addr.span >> levels).max(1);
    (addr.start..addr.end())
        .step_by(span as usize)
        .map(|start| NodeAddr { start, span })
        .collect()
}

/// The aligned blocks tiling `[start, end)`, each as large as fits.
fn aligned_blocks(start: u64, end: u64) -> Vec<NodeAddr> {
    let mut out = Vec::new();
    let mut at = start;
    while at < end {
        let mut span = if at == 0 {
            1 << 62
        } else {
            at & at.wrapping_neg()
        };
        while at + span > end {
            span /= 2;
        }
        out.push(NodeAddr { start: at, span });
        at += span;
    }
    out
}

/// Each workspace address as its own line, or its built halves when unbuilt.
/// It reads one tree level per query, so it loads only the nodes it shows,
/// never the whole range under a top line.
async fn workspace_lines(
    pool: &PgPool,
    addrs: Vec<NodeAddr>,
) -> Result<Vec<RecallLine>, sqlx::Error> {
    enum Slot {
        Line(RecallLine),
        Open(NodeAddr),
    }
    let mut slots: Vec<Slot> = addrs.into_iter().map(Slot::Open).collect();
    let mut missing = 0;
    loop {
        let open: Vec<NodeAddr> = slots
            .iter()
            .filter_map(|s| match s {
                Slot::Open(addr) => Some(*addr),
                Slot::Line(_) => None,
            })
            .collect();
        if open.is_empty() {
            break;
        }
        let mut nodes = store::nodes_at(pool, SummaryScope::Workspace, &open).await?;
        let mut next = Vec::with_capacity(slots.len());
        for slot in slots {
            let Slot::Open(addr) = slot else {
                next.push(slot);
                continue;
            };
            if let Some(node) = nodes.remove(&addr) {
                next.push(Slot::Line(RecallLine {
                    id: format!("w/{addr}"),
                    text: node.text,
                }));
            } else if let Some((a, b)) = addr.children() {
                next.extend([Slot::Open(a), Slot::Open(b)]);
            } else {
                missing += 1;
            }
        }
        slots = next;
    }
    let mut out: Vec<RecallLine> = slots
        .into_iter()
        .filter_map(|s| match s {
            Slot::Line(line) => Some(line),
            Slot::Open(_) => None,
        })
        .collect();
    if missing > 0 {
        out.push(RecallLine {
            id: "w/pending".to_string(),
            text: format!(
                "{missing} newer entries are not summarised yet. search finds their words"
            ),
        });
    }
    Ok(out)
}

async fn open_workspace_leaf(
    pool: &PgPool,
    id: NodeId,
    node: &StoredNode,
    read_artifact: &(dyn Fn(&str, &str) -> Option<String> + Sync),
) -> Result<Vec<RecallLine>, BoxError> {
    let source = node
        .source_event_id
        .ok_or_else(|| format!("{id} names no source"))?;
    if let Some(thread) = node.source_thread_id {
        let Some(tree) = ThreadTree::load_if_present(pool, thread).await? else {
            return Ok(vec![RecallLine {
                id: id.to_string(),
                text: "(a thread no longer here)".to_string(),
            }]);
        };
        let Some(turn) = tree.turns.iter().find(|t| t.settle_event_id == source) else {
            return Err(format!("{id}: its turn is no longer in thread {thread}").into());
        };
        let range = turn.entries.clone();
        return Ok(tree.lines(aligned_blocks(range.start as u64, range.end as u64)));
    }
    let (path, commit) = store::artifact_write(pool, source)
        .await?
        .ok_or_else(|| format!("{id}: its artifact write is gone"))?;
    let text = read_artifact(&path, &commit)
        .map(|content| crate::engine::context::truncate_head_tail(&content, ARTIFACT_CHARS))
        .unwrap_or_else(|| format!("({path} at {commit} can no longer be read)"));
    Ok(vec![RecallLine {
        id: id.to_string(),
        text: format!("artifact: {path}\n{text}"),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(start: u64, span: u64) -> NodeAddr {
        NodeAddr { start, span }
    }

    #[test]
    fn descendants_halve_per_level_and_stop_at_leaves() {
        assert_eq!(descendants(addr(8, 8), 1), vec![addr(8, 4), addr(12, 4)]);
        assert_eq!(descendants(addr(0, 2), 3), vec![addr(0, 1), addr(1, 1)]);
    }

    #[test]
    fn aligned_blocks_tile_a_turns_range() {
        assert_eq!(
            aligned_blocks(3, 9),
            vec![addr(3, 1), addr(4, 4), addr(8, 1)]
        );
        assert_eq!(aligned_blocks(0, 6), vec![addr(0, 4), addr(4, 2)]);
        assert!(aligned_blocks(5, 5).is_empty());
    }

    #[test]
    fn node_ids_round_trip_and_bare_ids_name_the_caller() {
        let thread = Uuid::new_v4();
        let parsed = NodeId::parse("w/8+4", None).unwrap();
        assert_eq!(parsed.to_string(), "w/8+4");
        let bare = NodeId::parse("[3+1]", Some(thread)).unwrap();
        assert_eq!(bare.to_string(), format!("{thread}/3+1"));
        assert!(NodeId::parse("3+1", None).is_err());
        assert!(
            NodeId::parse("w/3+2", None).is_err(),
            "3 is not a multiple of 2"
        );
        assert!(
            NodeId::parse("w/18446744073709551615+1", None).is_err(),
            "start+span overflows"
        );
        assert!(
            NodeId::parse("w/0+3", None).is_err(),
            "3 is not a power of 2"
        );
    }

    #[test]
    fn a_snippet_centres_on_the_query() {
        let text = format!("{}needle{}", "a".repeat(400), "b".repeat(400));
        let cut = snippet(&text, "NEEDLE");
        assert!(cut.contains("needle") && cut.starts_with('…') && cut.ends_with('…'));
    }
}
