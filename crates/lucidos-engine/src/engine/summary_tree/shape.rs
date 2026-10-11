//! Pure tree arithmetic: which nodes a log of `len` entries holds, and which
//! may start now.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::NodeAddr;

/// Every address a fully built tree over `len` entries holds. Which nodes
/// exist is a function of `len` alone, which is what makes a rebuild land on
/// the same shape (I1).
pub(crate) fn complete_addresses(len: u64) -> Vec<NodeAddr> {
    let mut out = Vec::new();
    let mut span = 1;
    while span <= len {
        let mut start = 0;
        while start + span <= len {
            out.push(NodeAddr { start, span });
            start += span;
        }
        span *= 2;
    }
    out
}

/// How many unbuilt leaves of a thread may build at once (OptChat §4).
pub(crate) const LEAF_WINDOW: usize = 8;

/// Whether a tree's leaves wait on the leaves before them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeafOrder {
    /// A thread leaf reads the summaries before it as context, so it starts
    /// once fewer than [`LEAF_WINDOW`] leaves before it are unbuilt.
    Windowed,
    /// A workspace leaf reads nothing of its own tree.
    Any,
}

/// The unbuilt nodes of a tree over `len` entries. The build loop keeps this
/// set current, so it never rescans the whole tree to find what may start.
///
/// Merges are ordered by their end. The pump only looks at merges ending at or
/// before the first unbuilt leaf, so it walks a short prefix of this set.
pub(crate) struct Pending {
    len: u64,
    leaves: BTreeSet<u64>,
    merges: BTreeSet<(u64, u64)>,
}

impl Pending {
    pub(crate) fn new<V>(len: u64, built: &HashMap<NodeAddr, V>) -> Self {
        let mut pending = Self {
            len,
            leaves: BTreeSet::new(),
            merges: BTreeSet::new(),
        };
        for addr in complete_addresses(len) {
            if built.contains_key(&addr) {
                continue;
            }
            if addr.is_leaf() {
                pending.leaves.insert(addr.start);
            } else {
                pending.merges.insert((addr.end(), addr.span));
            }
        }
        pending
    }

    pub(crate) fn mark_built(&mut self, addr: NodeAddr) {
        if addr.is_leaf() {
            self.leaves.remove(&addr.start);
        } else {
            self.merges.remove(&(addr.end(), addr.span));
        }
    }

    /// How many nodes are still unbuilt.
    pub(crate) fn remaining(&self) -> u64 {
        (self.leaves.len() + self.merges.len()) as u64
    }

    /// Whether every node is built.
    pub(crate) fn is_done(&self) -> bool {
        self.leaves.is_empty() && self.merges.is_empty()
    }

    /// The first leaf not built yet, or `len` when every leaf is.
    pub(crate) fn first_unbuilt_leaf(&self) -> u64 {
        self.leaves.first().copied().unwrap_or(self.len)
    }

    /// [`super::fold::built_prefix_end`] without a scan: the prefix ends just
    /// before the first end an unbuilt node has.
    pub(crate) fn built_prefix_end(&self) -> u64 {
        let leaf_end = self.leaves.first().map(|&i| i + 1);
        let merge_end = self.merges.first().map(|&(end, _)| end);
        leaf_end
            .into_iter()
            .chain(merge_end)
            .min()
            .map_or(self.len, |end| end - 1)
    }

    /// Up to `room` nodes the compactor may start now (OptChat §4).
    ///
    /// In a tree whose leaves read the lines before them, only the first
    /// [`LEAF_WINDOW`] unbuilt leaves are eligible. A leaf's context stops at
    /// the first unbuilt one, so it misses at most the few lines still
    /// building. Leaves that read no line of their own tree may all build at
    /// once. A merge needs both children built and every leaf up to its end
    /// built, so it never waits on a line that could still change. Merges go
    /// first: each one moves the built prefix, which every view folds.
    pub(crate) fn buildable<V>(
        &self,
        built: &HashMap<NodeAddr, V>,
        busy: &HashSet<NodeAddr>,
        order: LeafOrder,
        room: usize,
    ) -> Vec<NodeAddr> {
        let first = self.first_unbuilt_leaf();
        let mut out = Vec::new();
        for &(end, span) in self.merges.range(..=(first, u64::MAX)) {
            if out.len() >= room {
                return out;
            }
            let addr = NodeAddr {
                start: end - span,
                span,
            };
            if busy.contains(&addr) {
                continue;
            }
            let (a, b) = addr.children().expect("a merge has children");
            if built.contains_key(&a) && built.contains_key(&b) {
                out.push(addr);
            }
        }
        let window = match order {
            LeafOrder::Windowed => LEAF_WINDOW,
            LeafOrder::Any => usize::MAX,
        };
        let room = room - out.len();
        out.extend(
            self.leaves
                .iter()
                .take(window)
                .map(|&i| NodeAddr::leaf(i))
                .filter(|a| !busy.contains(a))
                .take(room),
        );
        out
    }
}
