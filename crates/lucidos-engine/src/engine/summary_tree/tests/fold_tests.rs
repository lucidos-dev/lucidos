use std::collections::{HashMap, HashSet};

use crate::engine::summary_tree::fold::{built_cover, built_prefix_end, fold, Budget, Fold};
use crate::engine::summary_tree::shape::complete_addresses;
use crate::engine::summary_tree::view::{
    cache_rungs, BLOCK_LINES, LOOKBACK_BLOCKS, THREAD_CACHE_RUNGS,
};
use crate::engine::summary_tree::NodeAddr;

fn addr(start: u64, span: u64) -> NodeAddr {
    NodeAddr { start, span }
}

fn complete(len: u64, line: usize) -> HashMap<NodeAddr, usize> {
    complete_addresses(len)
        .into_iter()
        .map(|a| (a, line))
        .collect()
}

fn view(len: u64, sizes: &HashMap<NodeAddr, usize>, budget: usize) -> Vec<NodeAddr> {
    let prefix = built_prefix_end(len, |a| sizes.contains_key(&a));
    fold(len, prefix, |a| sizes.get(&a).copied(), budget, 0)
}

fn bytes(tiles: &[NodeAddr], size: impl Fn(NodeAddr) -> Option<usize>) -> usize {
    tiles.iter().map(|t| size(*t).unwrap_or(0)).sum()
}

fn assert_tiles(tiles: &[NodeAddr], from: u64, len: u64) {
    let mut at = from;
    for t in tiles {
        assert_eq!(t.start, at, "{tiles:?} leaves a gap or overlaps");
        at = t.end();
    }
    assert_eq!(at, len, "{tiles:?} stops short of {len}");
}

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

fn rollback_lines(list: &[(bool, u64)], len: u64) -> Vec<NodeAddr> {
    let starts: Vec<u64> = list.iter().rev().map(|e| e.1).collect();
    let ends = starts.iter().skip(1).copied().chain([len]);
    starts
        .iter()
        .zip(ends)
        .map(|(&s, e)| addr(s, e - s))
        .collect()
}

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

#[test]
fn a_replay_from_zero_equals_the_live_view() {
    let mut live = Fold::default();
    for len in 1..3_000u64 {
        live.push(&|a| Some(synthetic(a)), Budget::sawtooth(BUDGET - RESERVE));
        let replay = fold(len, len, |a| Some(synthetic(a)), BUDGET, RESERVE);
        assert_eq!(live.addrs(), replay, "caught up, len {len}");
    }

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

fn uncached_share(entries: u64, turn: u64, samples: u64) -> f64 {
    let size = |a: NodeAddr| Some(synthetic(a));
    let mut live = Fold::default();
    let mut view = |len: u64| {
        while live.entries() < len {
            live.push(&size, Budget::sawtooth(BUDGET - RESERVE));
        }
        live.addrs()
    };
    let cache_rungs_of =
        |tiles: &[NodeAddr]| cache_rungs(tiles.len() / BLOCK_LINES, THREAD_CACHE_RUNGS);
    let mut previous = view(entries);
    let (mut uncached, mut sent) = (0, 0);
    for step in 1..=samples {
        let current = view(entries + step * turn);
        let same = previous
            .iter()
            .zip(&current)
            .take_while(|(a, b)| a == b)
            .count();
        let ours = cache_rungs_of(&current);
        let read = cache_rungs_of(&previous)
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
    log_legacy!("[SummaryTree] Uncached share of the thread view per turn:\n{report}");
    assert!(shares[&(10_000, 4)] <= 0.10, "{report}");
    assert!(shares[&(10_000, 30)] <= 0.40, "{report}");
}

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
