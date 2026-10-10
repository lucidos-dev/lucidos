//! The fold over synthetic logs: I4 (a stable fold), I6 (budgets hold), the
//! fold's half of I7, and the spec fold's own promises from the ADR 0362
//! amendments:
//! - it makes the rollback push's merges, in batches;
//! - a replay equals the view kept live;
//! - consecutive turns read most of the view from cache;
//! - each level keeps about as many lines.

use std::collections::{HashMap, HashSet};

use crate::engine::summary_tree::fold::{built_cover, built_prefix_end, fold, Budget, Fold};
use crate::engine::summary_tree::shape::complete_addresses;
use crate::engine::summary_tree::view::{rungs, BLOCK_LINES, LOOKBACK_BLOCKS, THREAD_RUNGS};
use crate::engine::summary_tree::NodeAddr;

fn addr(start: u64, span: u64) -> NodeAddr {
    NodeAddr { start, span }
}

/// Every node of a complete tree over `len`, each `line` bytes.
fn complete(len: u64, line: usize) -> HashMap<NodeAddr, usize> {
    complete_addresses(len)
        .into_iter()
        .map(|a| (a, line))
        .collect()
}

/// The view over whatever `sizes` holds, with no reserve.
fn view(len: u64, sizes: &HashMap<NodeAddr, usize>, budget: usize) -> Vec<NodeAddr> {
    let prefix = built_prefix_end(len, |a| sizes.contains_key(&a));
    fold(len, prefix, |a| sizes.get(&a).copied(), budget, 0)
}

fn bytes(tiles: &[NodeAddr], size: impl Fn(NodeAddr) -> Option<usize>) -> usize {
    tiles.iter().map(|t| size(*t).unwrap_or(0)).sum()
}

/// The view tiles its log: no gap, no overlap, oldest first.
fn assert_tiles(tiles: &[NodeAddr], from: u64, len: u64) {
    let mut at = from;
    for t in tiles {
        assert_eq!(t.start, at, "{tiles:?} leaves a gap or overlaps");
        at = t.end();
    }
    assert_eq!(at, len, "{tiles:?} stops short of {len}");
}

/// A rendered line's bytes for a fully built synthetic tree, a pure function
/// of the address. Leaves mix short verbatim messages with near-full
/// summaries; merges are dense summaries. The `[id] ` prefix is counted.
pub(super) fn synthetic(a: NodeAddr) -> usize {
    let mut h = a.start.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ a.span.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    let text = if a.is_leaf() && h % 10 < 4 {
        40 + h % 100
    } else if a.is_leaf() {
        200 + h % 313
    } else {
        300 + h % 213
    };
    a.to_string().len() + 4 + text as usize
}

const BUDGET: usize = 64 * 1024;
const RESERVE: usize = 8 * 1024;

#[test]
fn a_roomy_budget_shows_every_entry_verbatim() {
    let sizes = complete(6, 10);
    assert_eq!(
        view(6, &sizes, 10_000),
        (0..6).map(NodeAddr::leaf).collect::<Vec<_>>()
    );
}

/// The most due pair goes first: the longest ago for its size. Between pairs
/// as due as each other, the older one goes. So each level keeps about as
/// many lines, rather than the oldest lines all folding into one. Driven at an
/// exact budget, since the sawtooth changes when merges happen, not which.
#[test]
fn a_tight_budget_merges_the_most_due_pairs() {
    let sizes = complete(8, 10);
    let mut live = Fold::default();
    for _ in 0..8 {
        live.push(&|a| sizes.get(&a).copied(), Budget::exact(50));
    }
    assert_eq!(
        live.addrs(),
        vec![addr(0, 2), addr(2, 2), addr(4, 2), addr(6, 1), addr(7, 1)]
    );
}

/// Taelin's rollback push, quoted in OptChat §3.1, without `life`: under push
/// alone it stays 0. `list` is newest first, each state with its keep bit. A
/// 0 bit absorbs the pushed state, and a 1 bit carries the old one further.
fn rollback_push(list: &mut Vec<(bool, u64)>, mut state: u64) {
    for entry in list.iter_mut() {
        if !entry.0 {
            entry.0 = true;
            return;
        }
        (state, entry.1) = (entry.1, state);
        entry.0 = false;
    }
    list.push((false, state));
}

/// Push's list as view lines: each state runs to the next newer one, and the
/// newest runs to `len`.
fn rollback_lines(list: &[(bool, u64)], len: u64) -> Vec<NodeAddr> {
    let starts: Vec<u64> = list.iter().rev().map(|e| e.1).collect();
    let ends = starts.iter().skip(1).copied().chain([len]);
    starts
        .iter()
        .zip(ends)
        .map(|(&s, e)| addr(s, e - s))
        .collect()
}

/// With push's list length as the budget, the fold makes exactly push's
/// merges at every step (OptChat §3.2). Measuring a pair's age from its first
/// entry instead matches at 481 of these steps.
#[test]
fn the_fold_makes_the_rollback_push_merges() {
    let mut list = Vec::new();
    let mut live = Fold::default();
    for t in 0..=20_000u64 {
        rollback_push(&mut list, t);
        live.push(&|_| Some(1), Budget::exact(list.len()));
        assert_eq!(live.addrs(), rollback_lines(&list, t + 1), "t = {t}");
    }
}

/// I6: no view is over its budget, for every log length and budget tried.
#[test]
fn no_view_exceeds_its_budget() {
    for len in 0..140 {
        let sizes = complete(len, 37);
        for budget in [0, 36, 37, 100, 512, 2_000, 16_384] {
            let tiles = view(len, &sizes, budget);
            assert!(
                bytes(&tiles, |a| sizes.get(&a).copied()) <= budget,
                "len {len} budget {budget}"
            );
        }
    }
}

/// I4: as the log grows, a merged node never splits, and the view's first
/// difference from the shorter log's is a merge or the newest entry.
#[test]
fn the_fold_is_stable_as_the_log_grows() {
    let mut previous: Vec<NodeAddr> = Vec::new();
    for len in 1..400u64 {
        let sizes = complete(len, 37);
        let tiles = view(len, &sizes, 600);
        assert_tiles(&tiles, 0, len);

        for old in &previous {
            let holder = tiles
                .iter()
                .find(|t| t.start <= old.start && old.end() <= t.end());
            assert!(
                holder.is_some(),
                "len {len}: {old} split into smaller lines"
            );
        }

        let same = previous
            .iter()
            .zip(&tiles)
            .take_while(|(a, b)| a == b)
            .count();
        if let (Some(old), Some(new)) = (previous.get(same), tiles.get(same)) {
            assert!(
                new.span > old.span,
                "len {len}: {new} replaced {old} with no merge"
            );
        }
        previous = tiles;
    }
}

/// The sawtooth: a view only appends until it passes its budget, then one
/// batch merges it down to half. With every node built, a batch always gets
/// there.
#[test]
fn the_view_appends_until_a_batch_halves_it() {
    let budget = BUDGET - RESERVE;
    let size = |a: NodeAddr| Some(synthetic(a));
    let mut live = Fold::default();
    let mut previous: Vec<NodeAddr> = Vec::new();
    let mut batches = 0;
    for len in 1..20_000u64 {
        live.push(&size, Budget::sawtooth(budget));
        let tiles = live.addrs();
        let appended = tiles.len() == previous.len() + 1 && tiles.starts_with(&previous);
        let grown = bytes(&previous, size) + synthetic(NodeAddr::leaf(len - 1));
        if grown > budget {
            batches += 1;
            assert!(
                bytes(&tiles, size) <= budget / 2,
                "len {len}: a batch stopped short"
            );
        } else {
            assert!(
                appended,
                "len {len}: merged with {grown} bytes under {budget}"
            );
        }
        previous = tiles;
    }
    assert!(batches >= 5, "only {batches} batches");
}

/// A view kept live, one entry at a time, is byte for byte the view a replay
/// from entry 0 gives. Checked with the compactor caught up, and with it a
/// turn behind, where the unbuilt tail rides in the reserve.
#[test]
fn a_replay_from_zero_equals_the_live_view() {
    let mut live = Fold::default();
    for len in 1..3_000u64 {
        live.push(&|a| Some(synthetic(a)), Budget::sawtooth(BUDGET - RESERVE));
        let replay = fold(len, len, |a| Some(synthetic(a)), BUDGET, RESERVE);
        assert_eq!(live.addrs(), replay, "caught up, len {len}");
    }

    // Each turn adds `turn` entries. The compactor has not reached them yet:
    // their leaves are raw lines and no merge ending past them is built.
    let turn = 6;
    let mut live = Fold::default();
    let mut replayed = 0;
    for len in (turn..3_000u64).step_by(turn as usize) {
        let first_unbuilt = len - turn;
        let size = |a: NodeAddr| match (a.is_leaf(), a.start >= first_unbuilt) {
            (true, true) => Some(530),
            (false, _) if a.end() > first_unbuilt => None,
            _ => Some(synthetic(a)),
        };
        let now = built_prefix_end(len, |a| a.end() <= first_unbuilt);
        assert_eq!(now, first_unbuilt);
        while replayed < now {
            live.push(&|a| Some(synthetic(a)), Budget::sawtooth(BUDGET - RESERVE));
            replayed += 1;
        }
        let replay = fold(len, now, size, BUDGET, RESERVE);
        let mut expected = live.addrs();
        expected.extend((now..len).map(NodeAddr::leaf));
        assert_eq!(replay, expected, "a turn behind, len {len}");
    }
}

/// The share of a view's bytes a turn re-sends at full price, averaged over
/// `samples` turns of `turn` entries each, from a log of `entries`.
///
/// It models Anthropic's cache. The last turn wrote an entry at each of its
/// rungs. This turn reads the longest of those its view still starts with,
/// if one of its own rungs lies at most a lookback after it.
fn uncached_share(entries: u64, turn: u64, samples: u64) -> f64 {
    let size = |a: NodeAddr| Some(synthetic(a));
    let mut live = Fold::default();
    let mut view = |len: u64| {
        while live.entries() < len {
            live.push(&size, Budget::sawtooth(BUDGET - RESERVE));
        }
        live.addrs()
    };
    let rungs_of = |tiles: &[NodeAddr]| rungs(tiles.len() / BLOCK_LINES, THREAD_RUNGS);
    let mut previous = view(entries);
    let (mut uncached, mut sent) = (0, 0);
    for step in 1..=samples {
        let current = view(entries + step * turn);
        let same = previous
            .iter()
            .zip(&current)
            .take_while(|(a, b)| a == b)
            .count();
        let ours = rungs_of(&current);
        let read = rungs_of(&previous)
            .into_iter()
            .filter(|&r| r * BLOCK_LINES <= same)
            .filter(|&r| ours.iter().any(|&n| n >= r && n - r <= LOOKBACK_BLOCKS))
            .max()
            .unwrap_or(0);
        let total = bytes(&current, size);
        uncached += total - bytes(&current[..read * BLOCK_LINES], size);
        sent += total;
        previous = current;
    }
    uncached as f64 / sent as f64
}

/// Consecutive turns read most of the view from the prompt cache. Between
/// batches a view only appends, and a rung a few blocks on finds the last
/// turn's entry. A batch rewrites the view once. The table goes to the log;
/// the plan for OptChat revision 3c190e06 holds the numbers it replaced.
#[test]
fn consecutive_turns_read_most_of_the_view_from_cache() {
    let mut report = String::new();
    let mut shares = HashMap::new();
    for entries in [1_000u64, 10_000] {
        for turn in [1u64, 4, 10, 30] {
            let share = uncached_share(entries, turn, 300);
            report.push_str(&format!(
                "{entries} entries, turns of {turn}: {:.1}% uncached\n",
                share * 100.0
            ));
            shares.insert((entries, turn), share);
        }
    }
    log!("[SummaryTree] Uncached share of the thread view per turn:\n{report}");
    assert!(shares[&(10_000, 4)] <= 0.10, "{report}");
    assert!(shares[&(10_000, 30)] <= 0.40, "{report}");
}

/// No cliff: each level between the leaves and the coarsest holds within a
/// factor of two of the median level's lines, so detail fades with age in
/// steps. Read as the last batch before 10k entries left the view: after it,
/// every new entry is one more leaf. The leaves hold the newest pairs still
/// filling in, so they get a factor of three.
#[test]
fn each_level_keeps_about_as_many_lines() {
    let size = |a: NodeAddr| Some(synthetic(a));
    let mut live = Fold::default();
    let (mut tiles, mut len) = (Vec::new(), 0);
    for pushed in 1..=10_000u64 {
        let before = live.addrs().len();
        live.push(&size, Budget::sawtooth(BUDGET - RESERVE));
        if live.addrs().len() <= before {
            (tiles, len) = (live.addrs(), pushed);
        }
    }
    assert_tiles(&tiles, 0, len);
    let mut per_span: HashMap<u64, usize> = HashMap::new();
    for t in &tiles {
        *per_span.entry(t.span).or_default() += 1;
    }
    let mut spans: Vec<u64> = per_span.keys().copied().collect();
    spans.sort_unstable();
    let coarsest = spans.pop().expect("a level");
    let leaves = per_span[&spans.remove(0)];
    let mut counts: Vec<usize> = spans.iter().map(|s| per_span[s]).collect();
    counts.sort_unstable();
    let median = counts[counts.len() / 2];
    assert!(spans.len() >= 5, "{per_span:?}");
    assert!(leaves <= median * 3, "{leaves} leaves: {per_span:?}");
    for span in &spans {
        let lines = per_span[span];
        assert!(
            lines * 2 >= median && lines <= median * 2,
            "span {span} holds {lines} lines against a median of {median} \
             (coarsest {coarsest}): {per_span:?}"
        );
    }
}

/// The built prefix ends at the first entry some node covering it waits on.
#[test]
fn the_built_prefix_stops_at_the_first_unbuilt_node() {
    let mut sizes = complete(8, 10);
    assert_eq!(built_prefix_end(8, |a| sizes.contains_key(&a)), 8);
    sizes.remove(&addr(4, 2));
    assert_eq!(built_prefix_end(8, |a| sizes.contains_key(&a)), 5);
    sizes.remove(&addr(2, 1));
    assert_eq!(built_prefix_end(8, |a| sizes.contains_key(&a)), 2);
    assert_eq!(built_prefix_end(0, |_| false), 0);
}

/// I7: an unbuilt merge leaves its built children in the view. The view
/// still tiles the log, and it waits on nothing.
#[test]
fn an_unbuilt_merge_leaves_its_children_in_the_view() {
    let mut sizes = complete(8, 10);
    sizes.remove(&addr(0, 4));
    sizes.remove(&addr(0, 8));
    let tiles = view(8, &sizes, 50);
    assert_tiles(&tiles, 0, 8);
    assert!(tiles.contains(&addr(0, 2)) && tiles.contains(&addr(2, 2)));
    assert!(bytes(&tiles, |a| sizes.get(&a).copied()) <= 50);
}

/// With nothing above the leaves built, the oldest lines give way to keep the
/// budget, and the newest entries stay.
#[test]
fn a_stalled_compactor_drops_the_oldest_lines_to_keep_the_budget() {
    let sizes: HashMap<NodeAddr, usize> = (0..20).map(|i| (NodeAddr::leaf(i), 10)).collect();
    let tiles = view(20, &sizes, 55);
    assert_eq!(tiles, (15..20).map(NodeAddr::leaf).collect::<Vec<_>>());
}

#[test]
fn the_built_cover_reads_an_unbuilt_block_as_its_halves() {
    let built: HashSet<NodeAddr> = [addr(0, 1), addr(1, 1), addr(2, 1)].into();
    let size = |a: NodeAddr| built.contains(&a).then_some(1);
    assert_eq!(
        built_cover(3, size),
        vec![addr(0, 1), addr(1, 1), addr(2, 1)]
    );
    let sizes = complete(4, 1);
    assert_eq!(built_cover(4, |a| sizes.get(&a).copied()), vec![addr(0, 4)]);
}
