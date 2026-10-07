//! The fold over synthetic logs: I4 (a stable fold), I6 (budgets hold), the
//! fold's half of I7, and the spec fold's own promises from the ADR 0362
//! amendment. A replay equals the view kept live, consecutive views share
//! most of their bytes, and each level keeps about as many lines.

use std::collections::{HashMap, HashSet};

use crate::engine::summary_tree::fold::{
    built_cover, built_prefix_end, context_tiling, fold, Fold,
};
use crate::engine::summary_tree::shape::complete_addresses;
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
fn synthetic(a: NodeAddr) -> usize {
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

/// The most due pair goes first: the oldest for its size. Between pairs as
/// due as each other, the older one goes.
#[test]
fn a_tight_budget_merges_the_most_due_pairs() {
    let sizes = complete(8, 10);
    assert_eq!(
        view(8, &sizes, 50),
        vec![addr(0, 4), addr(4, 1), addr(5, 1), addr(6, 1), addr(7, 1)]
    );
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

/// A view kept live, one entry at a time, is byte for byte the view a replay
/// from entry 0 gives. Checked with the compactor caught up, and with it a
/// turn behind, where the unbuilt tail rides in the reserve.
#[test]
fn a_replay_from_zero_equals_the_live_view() {
    let mut live = Fold::default();
    for len in 1..3_000u64 {
        live.push(&|a| Some(synthetic(a)), BUDGET - RESERVE);
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
            live.push(&|a| Some(synthetic(a)), BUDGET - RESERVE);
            replayed += 1;
        }
        let replay = fold(len, now, size, BUDGET, RESERVE);
        let mut expected = live.addrs();
        expected.extend((now..len).map(NodeAddr::leaf));
        assert_eq!(replay, expected, "a turn behind, len {len}");
    }
}

/// The share of the newer view's bytes two consecutive views have in common
/// from their start, averaged over `samples` turns of `turn` entries each.
fn mean_share(
    entries: u64,
    turn: u64,
    samples: u64,
    view: &mut dyn FnMut(u64) -> Vec<NodeAddr>,
) -> f64 {
    let mut previous = view(entries);
    let mut total = 0.0;
    for step in 1..=samples {
        let current = view(entries + step * turn);
        let shared: usize = previous
            .iter()
            .zip(&current)
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| synthetic(*a))
            .sum();
        total += shared as f64 / bytes(&current, |a| Some(synthetic(a))) as f64;
        previous = current;
    }
    total / samples as f64
}

/// Consecutive turns share most of the view, where the old fold shared
/// little. Measured at 1k, 10k and 100k entries with turns of 1, 4 and 10
/// entries; the numbers are in the ADR 0362 amendment.
///
/// The spec fold is driven live, which the replay test above shows is the
/// same view. A view's levels bound its share, so the majority is asserted
/// from 10k entries for turns of up to four. The table reports the rest.
#[test]
fn consecutive_turns_share_most_of_the_view() {
    let mut report = String::new();
    let mut spec_shares = HashMap::new();
    for entries in [1_000u64, 10_000, 100_000] {
        for turn in [1u64, 4, 10] {
            let mut live = Fold::default();
            let mut pushed = 0;
            let mut spec = |len: u64| {
                while pushed < len {
                    live.push(&|a| Some(synthetic(a)), BUDGET - RESERVE);
                    pushed += 1;
                }
                live.addrs()
            };
            let new = mean_share(entries, turn, 40, &mut spec);
            let mut old = |len: u64| context_tiling(len, |a| Some(synthetic(a)), BUDGET);
            let old = mean_share(entries, turn, 40, &mut old);
            report.push_str(&format!(
                "{entries} entries, turns of {turn}: old {:.0}%, spec {:.0}%\n",
                old * 100.0,
                new * 100.0
            ));
            spec_shares.insert((entries, turn), new);
        }
    }
    log!("[SummaryTree] Consecutive-view prefix share:\n{report}");
    for entries in [10_000, 100_000] {
        for turn in [1, 4] {
            assert!(spec_shares[&(entries, turn)] > 0.5, "{report}");
        }
    }
    assert!(spec_shares[&(1_000, 1)] > 0.5, "{report}");
}

/// No cliff: each level between the leaves and the coarsest holds within a
/// factor of two of the median level's lines. Detail fades with age in steps. The leaves hold the newest pairs still filling in, so they get a
/// factor of three.
#[test]
fn each_level_keeps_about_as_many_lines() {
    let len = 10_000;
    let tiles = fold(len, len, |a| Some(synthetic(a)), BUDGET, RESERVE);
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

/// The compactor's context keeps its own shape: the coarsest cover of an old
/// prefix, then the recent entries verbatim.
#[test]
fn the_context_tiling_covers_an_old_prefix_then_recent_leaves() {
    let sizes = complete(8, 10);
    assert_eq!(
        context_tiling(8, |a| sizes.get(&a).copied(), 50),
        vec![addr(0, 4), addr(4, 1), addr(5, 1), addr(6, 1), addr(7, 1)]
    );
    let leaves: HashMap<NodeAddr, usize> = (0..20).map(|i| (NodeAddr::leaf(i), 10)).collect();
    assert_eq!(
        context_tiling(20, |a| leaves.get(&a).copied(), 55),
        (15..20).map(NodeAddr::leaf).collect::<Vec<_>>()
    );
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
