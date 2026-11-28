use std::collections::{HashMap, HashSet};

use crate::engine::summary_tree::shape::{complete_addresses, LeafOrder, Pending, LEAF_WINDOW};
use crate::engine::summary_tree::NodeAddr;

fn addr(start: u64, span: u64) -> NodeAddr {
    NodeAddr { start, span }
}

fn built(addrs: &[NodeAddr]) -> HashMap<NodeAddr, String> {
    addrs.iter().map(|a| (*a, format!("line {a}"))).collect()
}

fn buildable(
    len: u64,
    built: &HashMap<NodeAddr, String>,
    busy: &HashSet<NodeAddr>,
) -> Vec<NodeAddr> {
    Pending::new(len, built).buildable(built, busy, LeafOrder::Windowed, usize::MAX)
}

#[test]
fn a_complete_tree_holds_every_aligned_power_of_two() {
    let all: HashSet<_> = complete_addresses(5).into_iter().collect();
    let expected: HashSet<_> = [
        addr(0, 1),
        addr(1, 1),
        addr(2, 1),
        addr(3, 1),
        addr(4, 1),
        addr(0, 2),
        addr(2, 2),
        addr(0, 4),
    ]
    .into_iter()
    .collect();
    assert_eq!(all, expected);
}

#[test]
fn a_thread_leaf_starts_while_few_leaves_before_it_are_unbuilt() {
    let window = LEAF_WINDOW as u64;
    let none = built(&[]);
    let first: Vec<_> = (0..window).map(|i| addr(i, 1)).collect();
    assert_eq!(buildable(2 * window, &none, &HashSet::new()), first);
    let busy: HashSet<_> = [addr(0, 1)].into_iter().collect();
    assert_eq!(buildable(2 * window, &none, &busy), first[1..]);
}

#[test]
fn a_merge_waits_for_both_children() {
    let two = built(&[addr(0, 1), addr(1, 1)]);
    let next = buildable(4, &two, &HashSet::new());
    assert_eq!(next, vec![addr(0, 2), addr(2, 1), addr(3, 1)]);

    let one = built(&[addr(0, 1)]);
    assert_eq!(
        buildable(4, &one, &HashSet::new()),
        vec![addr(1, 1), addr(2, 1), addr(3, 1)]
    );
}

#[test]
fn a_ready_merge_goes_before_the_leaves() {
    let two = built(&[addr(0, 1), addr(1, 1)]);
    let pending = Pending::new(16, &two);
    assert_eq!(
        pending.buildable(&two, &HashSet::new(), LeafOrder::Windowed, 1),
        vec![addr(0, 2)]
    );
}

#[test]
fn a_merge_never_reaches_past_the_first_unbuilt_leaf() {
    let holes = built(&[addr(0, 1), addr(2, 1), addr(3, 1), addr(2, 2)]);
    let next = buildable(4, &holes, &HashSet::new());
    assert_eq!(next, vec![addr(1, 1)]);
}

#[test]
fn a_built_node_leaves_the_pending_set() {
    let mut nodes = built(&[]);
    let mut pending = Pending::new(4, &nodes);
    for leaf in [addr(0, 1), addr(1, 1)] {
        nodes.insert(leaf, String::new());
        pending.mark_built(leaf);
    }
    assert_eq!(pending.first_unbuilt_leaf(), 2);
    assert_eq!(
        pending.buildable(&nodes, &HashSet::new(), LeafOrder::Windowed, usize::MAX),
        vec![addr(0, 2), addr(2, 1), addr(3, 1)]
    );
}

#[test]
fn unordered_leaves_all_start_within_the_room() {
    let none = built(&[]);
    let pending = Pending::new(6, &none);
    let all = pending.buildable(&none, &HashSet::new(), LeafOrder::Any, usize::MAX);
    assert_eq!(all, (0..6).map(|i| addr(i, 1)).collect::<Vec<_>>());

    let busy: HashSet<_> = [addr(0, 1), addr(1, 1)].into_iter().collect();
    assert_eq!(
        pending.buildable(&none, &busy, LeafOrder::Any, 3),
        vec![addr(2, 1), addr(3, 1), addr(4, 1)]
    );
}

#[test]
fn an_unordered_merge_still_waits_for_every_leaf_before_its_end() {
    let holes = built(&[addr(0, 1), addr(2, 1), addr(3, 1)]);
    let pending = Pending::new(4, &holes);
    let next = pending.buildable(&holes, &HashSet::new(), LeafOrder::Any, usize::MAX);
    assert_eq!(next, vec![addr(1, 1)]);
}
