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
use crate::engine::summary_tree::compaction_view::{CompactionView, MAX_BYTES, RUNGS};
use crate::engine::summary_tree::fold::built_prefix_end;
use crate::engine::summary_tree::shape::{complete_addresses, LeafOrder, Pending};
use crate::engine::summary_tree::view::{trailing_rungs, BLOCK_LINES, LOOKBACK_BLOCKS};
use crate::engine::summary_tree::NodeAddr;

/// Calls a drain runs at once on one thread.
const IN_FLIGHT: usize = 8;

/// One simulated call: what it read, and how many calls had answered when it
/// launched.
struct Launch {
    tiles: Vec<NodeAddr>,
    answered: usize,
}

/// What a call's context is, from the drain's state at launch.
trait Context {
    fn advance(&mut self, built_prefix: u64, built: &HashMap<NodeAddr, usize>);
    fn tiles(&self, end: u64, built: &HashMap<NodeAddr, usize>) -> Vec<NodeAddr>;
}

impl Context for CompactionView {
    fn advance(&mut self, built_prefix: u64, built: &HashMap<NodeAddr, usize>) {
        CompactionView::advance(self, built_prefix, &|a| built.get(&a).copied());
    }

    fn tiles(&self, end: u64, built: &HashMap<NodeAddr, usize>) -> Vec<NodeAddr> {
        CompactionView::tiles(self, end, &|a| built.get(&a).copied())
    }
}

/// Drain a tree over `entries` from nothing, as the build loop does, and
/// check each call's context as it launches.
fn drain(entries: u64, order: LeafOrder, context: &mut impl Context) -> Vec<Launch> {
    let mut built: HashMap<NodeAddr, usize> = HashMap::new();
    let mut pending = Pending::new(entries, &built);
    let (mut busy, mut inflight) = (HashSet::new(), VecDeque::new());
    let (mut launches, mut answered) = (Vec::new(), 0);
    loop {
        context.advance(pending.built_prefix_end(), &built);
        let room = IN_FLIGHT - inflight.len();
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
            launches.push(Launch { tiles, answered });
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
/// call writes an entry at each of its rungs. A call reads only entries that
/// calls answered before it launched wrote, since an entry becomes readable
/// once its writer's response starts.
fn cost(launches: &[Launch], cache: Cache) -> (f64, f64) {
    let mut written: HashMap<u64, usize> = HashMap::new();
    let (mut sent, mut uncached) = (0usize, 0usize);
    for (call, launch) in launches.iter().enumerate() {
        let tiles = &launch.tiles;
        let ours = trailing_rungs(tiles.len() / BLOCK_LINES, RUNGS);
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
