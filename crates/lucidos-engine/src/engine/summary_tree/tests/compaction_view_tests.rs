//! The compaction view over simulated drains of a synthetic thread, with
//! `IN_FLIGHT` calls at once and answers in launch order. Plan:
//! `docs/plans/2026-10-08-tree-compactor-follows-optchat-revision-3c190e06.md`.
//! - a call reads only built lines, up to its own end, within its budget;
//! - the view kept for a drain equals a replay;
//! - consecutive calls read most of their view from the prompt cache.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

use super::fold_tests::synthetic;
use crate::engine::summary_tree::compaction_view::{mark_blocks, CompactionView, MAX_BYTES};
use crate::engine::summary_tree::fold::built_prefix_end;
use crate::engine::summary_tree::shape::{complete_addresses, LeafOrder, Pending};
use crate::engine::summary_tree::view::{BLOCK_LINES, LOOKBACK_BLOCKS};
use crate::engine::summary_tree::NodeAddr;

/// Calls a drain runs at once on one thread.
const IN_FLIGHT: usize = 8;

/// Calls a workspace drain runs at once: every lane slot, since its leaves
/// read no other line and build in any order.
const WORKSPACE_IN_FLIGHT: usize = 128;

/// One simulated call: what it read, how many of those lines were the
/// built prefix, and how many calls had answered when it launched.
struct Launch {
    tiles: Vec<NodeAddr>,
    prefix_lines: usize,
    answered: usize,
}

/// What a call's context is, from the drain's state at launch.
trait Context {
    fn advance(&mut self, built_prefix: u64, built: &HashMap<NodeAddr, usize>);
    fn tiles(&self, end: u64, built: &HashMap<NodeAddr, usize>) -> Vec<NodeAddr>;
    fn prefix_lines(&self) -> usize;
}

impl Context for CompactionView {
    fn advance(&mut self, built_prefix: u64, built: &HashMap<NodeAddr, usize>) {
        CompactionView::advance(self, built_prefix, &|a| built.get(&a).copied());
    }

    fn tiles(&self, end: u64, built: &HashMap<NodeAddr, usize>) -> Vec<NodeAddr> {
        CompactionView::tiles(self, end, &|a| built.get(&a).copied())
    }

    fn prefix_lines(&self) -> usize {
        CompactionView::prefix_lines(self)
    }
}

/// Drain a tree over `entries` from nothing, as the build loop does, and
/// check each call's context as it launches.
fn drain(entries: u64, order: LeafOrder, context: &mut impl Context) -> Vec<Launch> {
    drain_with(entries, order, IN_FLIGHT, context)
}

/// [`drain`] with `in_flight` calls at once.
fn drain_with(
    entries: u64,
    order: LeafOrder,
    in_flight: usize,
    context: &mut impl Context,
) -> Vec<Launch> {
    let mut built: HashMap<NodeAddr, usize> = HashMap::new();
    let mut pending = Pending::new(entries, &built);
    let (mut busy, mut inflight) = (HashSet::new(), VecDeque::new());
    let (mut launches, mut answered) = (Vec::new(), 0);
    loop {
        context.advance(pending.built_prefix_end(), &built);
        let room = in_flight - inflight.len();
        for addr in pending.buildable(&built, &busy, order, room) {
            let end = if addr.is_leaf() {
                addr.start.min(pending.first_unbuilt_leaf())
            } else {
                addr.end()
            };
            let tiles = context.tiles(end, &built);
            assert!(
                tiles
                    .iter()
                    .all(|t| built.contains_key(t) && t.end() <= end),
                "{addr} reads a line that is unbuilt or past its end"
            );
            launches.push(Launch {
                tiles,
                prefix_lines: context.prefix_lines(),
                answered,
            });
            busy.insert(addr);
            inflight.push_back(addr);
        }
        let Some(addr) = inflight.pop_front() else {
            break;
        };
        busy.remove(&addr);
        pending.mark_built(addr);
        built.insert(addr, synthetic(addr));
        answered += 1;
    }
    assert!(pending.is_done());
    launches
}

/// How a provider finds an entry an earlier request wrote.
#[derive(Clone, Copy, Debug)]
enum Cache {
    /// Claude: at most [`LOOKBACK_BLOCKS`] block ends back from a mark.
    Lookback,
    /// GPT: only at a mark in this request, in the very same place.
    ExactMark,
}

fn prefix_key(tiles: &[NodeAddr]) -> u64 {
    let mut h = DefaultHasher::new();
    tiles.hash(&mut h);
    h.finish()
}

fn bytes(tiles: &[NodeAddr]) -> usize {
    tiles.iter().map(|t| synthetic(*t)).sum()
}

/// Mean view bytes per call, and the mean a call sends at full price. Each
/// call writes an entry at each of its cache rungs. A call reads only entries that
/// calls answered before it launched wrote, since an entry becomes readable
/// once its writer's response starts.
fn cost(launches: &[Launch], cache: Cache) -> (f64, f64) {
    let mut written: HashMap<u64, usize> = HashMap::new();
    let (mut sent, mut uncached) = (0usize, 0usize);
    for (call, launch) in launches.iter().enumerate() {
        let tiles = &launch.tiles;
        let ours = mark_blocks(tiles.len() / BLOCK_LINES, launch.prefix_lines);
        let readable = |blocks: usize| {
            written
                .get(&prefix_key(&tiles[..blocks * BLOCK_LINES]))
                .is_some_and(|&writer| writer < launch.answered)
        };
        let read = match cache {
            Cache::ExactMark => ours.iter().copied().filter(|&r| readable(r)).max(),
            Cache::Lookback => ours
                .iter()
                .flat_map(|&r| (r.saturating_sub(LOOKBACK_BLOCKS - 1)..=r).rev())
                .filter(|&b| b > 0 && readable(b))
                .max(),
        }
        .unwrap_or(0);
        for &r in &ours {
            written
                .entry(prefix_key(&tiles[..r * BLOCK_LINES]))
                .or_insert(call);
        }
        let total = bytes(tiles);
        sent += total;
        uncached += total - bytes(&tiles[..read * BLOCK_LINES]);
    }
    let calls = launches.len() as f64;
    (sent as f64 / calls, uncached as f64 / calls)
}

#[test]
fn a_call_reads_built_lines_within_its_budget() {
    let launches = drain(3_000, LeafOrder::Windowed, &mut CompactionView::default());
    for launch in &launches {
        assert!(bytes(&launch.tiles) <= MAX_BYTES);
    }
}

/// The view a drain keeps live is the view a fresh replay over the same built
/// nodes reads, at every launch.
#[test]
fn a_live_compaction_view_equals_a_replay() {
    struct Checked(CompactionView);
    impl Context for Checked {
        fn advance(&mut self, built_prefix: u64, built: &HashMap<NodeAddr, usize>) {
            self.0.advance(built_prefix, &|a| built.get(&a).copied());
        }

        fn tiles(&self, end: u64, built: &HashMap<NodeAddr, usize>) -> Vec<NodeAddr> {
            let size = |a| built.get(&a).copied();
            let live = self.0.tiles(end, &size);
            let mut replay = CompactionView::default();
            replay.advance(built_prefix_end(end, |a| built.contains_key(&a)), &size);
            assert_eq!(live, replay.tiles(end, &size), "at end {end}");
            live
        }

        fn prefix_lines(&self) -> usize {
            self.0.prefix_lines()
        }
    }
    drain(
        1_500,
        LeafOrder::Windowed,
        &mut Checked(CompactionView::default()),
    );
}

/// The built prefix the build loop tracks is the one a scan finds.
#[test]
fn the_pending_set_tracks_the_built_prefix() {
    let len = 37;
    let mut built: HashMap<NodeAddr, ()> = HashMap::new();
    let mut pending = Pending::new(len, &built);
    let mut order = complete_addresses(len);
    order.sort_by_key(|a| (a.end().wrapping_mul(2_654_435_761) % 97, a.span));
    for addr in order {
        built.insert(addr, ());
        pending.mark_built(addr);
        assert_eq!(
            pending.built_prefix_end(),
            built_prefix_end(len, |a| built.contains_key(&a))
        );
    }
}

/// Consecutive compactions read most of their view from the prompt cache,
/// under both providers' rules. The table goes to the log.
#[test]
fn consecutive_compactions_read_most_of_their_view_from_cache() {
    let entries = 10_000;
    let view = drain(entries, LeafOrder::Windowed, &mut CompactionView::default());
    let mut report = String::new();
    for cache in [Cache::Lookback, Cache::ExactMark] {
        let (sent, uncached) = cost(&view, cache);
        report.push_str(&format!(
            "{cache:?}: {sent:.0} bytes per call, {uncached:.0} uncached ({:.1}%)\n",
            100.0 * uncached / sent
        ));
    }
    log!("[SummaryTree] Compaction view cache, {entries} entries:\n{report}");
    for cache in [Cache::Lookback, Cache::ExactMark] {
        let (sent, uncached) = cost(&view, cache);
        assert!(uncached <= 0.15 * sent, "{cache:?}\n{report}");
    }
}

/// The workspace tree builds leaves in any order, every lane slot busy. So its
/// built prefix stalls behind one slow leaf while the tail grows. Merges there
/// still read their prefix from the cache, and the tail keeps to its room.
///
/// The bound stays loose by nature. A call reads only what calls answered
/// before it launched wrote, about [`WORKSPACE_IN_FLIGHT`] launches back. The
/// prefix rolls over more often than that.
#[test]
fn a_stalled_prefix_keeps_merges_reading_from_cache() {
    let entries = 6_000;
    let view = drain_with(
        entries,
        LeafOrder::Any,
        WORKSPACE_IN_FLIGHT,
        &mut CompactionView::default(),
    );
    let (sent, uncached) = cost(&view, Cache::ExactMark);
    log!(
        "[SummaryTree] Workspace-shaped drain, {entries} entries: {sent:.0} bytes per call, \
         {uncached:.0} uncached ({:.1}%)",
        100.0 * uncached / sent
    );
    assert!(uncached <= 0.6 * sent, "{uncached:.0} of {sent:.0}");
    assert!(
        sent <= 0.8 * MAX_BYTES as f64,
        "the tail keeps to its room: {sent:.0}"
    );
}

/// The tail folds among itself, so the built prefix leads every call's
/// tiles line for line, however long the tail behind it grows.
#[test]
fn the_tail_never_rewrites_the_prefix() {
    let built: HashMap<NodeAddr, usize> = (0..600).map(|i| (NodeAddr::leaf(i), 512)).collect();
    let size = |a: NodeAddr| built.get(&a).copied();
    let mut view = CompactionView::default();
    view.advance(40, &size);
    let prefix = view.tiles(40, &size);
    assert_eq!(prefix.len(), view.prefix_lines());
    for end in [41, 80, 200, 600] {
        let tiles = view.tiles(end, &size);
        assert_eq!(&tiles[..prefix.len()], &prefix[..], "at end {end}");
        assert!(bytes_of(&tiles, &size) <= MAX_BYTES, "at end {end}");
    }
}

fn bytes_of(tiles: &[NodeAddr], size: &impl Fn(NodeAddr) -> Option<usize>) -> usize {
    tiles.iter().filter_map(|t| size(*t)).sum()
}
