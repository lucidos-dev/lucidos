//! The fold: which nodes of a summary tree a *memory view* shows (OptChat
//! §5.2).
//!
//! A view appends each entry's line at its end. While it is over budget, it
//! merges the adjacent pair of sibling lines that is most due: the oldest for
//! its size. A merged line never splits. A line over `2^l` entries therefore
//! changes about once every `2^l` entries. A view changes near its end, which
//! keeps its start readable from the prompt cache.
//!
//! Every read replays the view from entry 0, so a restart loses nothing. The
//! replay covers only the *built prefix*, where every node is built. Those
//! nodes never change, so a longer built prefix replays to the shorter one's
//! view plus appends and merges. Newer entries follow as leaves, in a reserve
//! the prefix replay leaves free. Why: the ADR 0362 amendment.

use std::collections::HashMap;

use super::NodeAddr;

/// The nodes a view of `budget` bytes shows, oldest first.
///
/// `size` gives a node's rendered bytes, or `None` while it is unbuilt. Every
/// leaf below `len` must answer. An unbuilt leaf shows as whatever line the
/// caller gives it, so a view never waits on the compactor (I7).
/// `built_prefix` is [`built_prefix_end`]. The prefix replay keeps `reserve`
/// bytes free for the newer entries. When no built pair is left to merge, the oldest lines go, so no
/// view exceeds its budget (I6).
pub(crate) fn fold(
    len: u64,
    built_prefix: u64,
    size: impl Fn(NodeAddr) -> Option<usize>,
    budget: usize,
    reserve: usize,
) -> Vec<NodeAddr> {
    let mut view = Fold::default();
    let prefix_budget = budget.saturating_sub(reserve);
    while view.len < built_prefix {
        view.push(&size, prefix_budget);
    }
    while view.len < len {
        view.push(&size, budget);
    }
    let mut drop = 0;
    while view.bytes > budget && drop < view.parts.len() {
        view.bytes -= view.parts[drop].bytes;
        drop += 1;
    }
    view.parts[drop..].iter().map(|p| p.addr).collect()
}

/// The longest prefix `[0, f)` of a log of `len` entries over which every node
/// is built. Nothing in it changes until a delete moves the log.
pub(crate) fn built_prefix_end(len: u64, built: impl Fn(NodeAddr) -> bool) -> u64 {
    for end in 1..=len {
        let mut span = 1;
        while end % span == 0 {
            if !built(NodeAddr {
                start: end - span,
                span,
            }) {
                return end - 1;
            }
            span *= 2;
        }
    }
    len
}

/// A view as it folds, one entry at a time.
#[derive(Default)]
pub(crate) struct Fold {
    parts: Vec<Part>,
    bytes: usize,
    /// Entries appended so far, the `T` of OptChat's age rule.
    len: u64,
}

#[derive(Clone, Copy)]
struct Part {
    addr: NodeAddr,
    bytes: usize,
}

impl Fold {
    /// Append the next entry's leaf, then merge the most due built pairs until
    /// the view fits `budget`, or no pair has a built parent.
    pub(crate) fn push(&mut self, size: &impl Fn(NodeAddr) -> Option<usize>, budget: usize) {
        let leaf = NodeAddr::leaf(self.len);
        let bytes = size(leaf).unwrap_or(0);
        self.parts.push(Part { addr: leaf, bytes });
        self.bytes += bytes;
        self.len += 1;
        while self.bytes > budget {
            let Some((at, parent)) = self.most_due(size) else {
                break;
            };
            self.bytes -= self.parts[at].bytes + self.parts[at + 1].bytes;
            self.bytes += parent.bytes;
            self.parts.splice(at..at + 2, [parent]);
        }
    }

    #[cfg(test)]
    pub(crate) fn addrs(&self) -> Vec<NodeAddr> {
        self.parts.iter().map(|p| p.addr).collect()
    }

    /// Where the sibling pair with the largest `due = age / 2^(l+2)` and a
    /// built parent starts, and that parent. Ties go to the oldest pair.
    fn most_due(&self, size: &impl Fn(NodeAddr) -> Option<usize>) -> Option<(usize, Part)> {
        let mut best: Option<(usize, Part)> = None;
        for (at, pair) in self.parts.windows(2).enumerate() {
            let (a, b) = (pair[0].addr, pair[1].addr);
            if a.span != b.span || a.start % (2 * a.span) != 0 || b.start != a.end() {
                continue;
            }
            let parent = NodeAddr {
                start: a.start,
                span: 2 * a.span,
            };
            // Cross-multiplied, so no division rounds two dues together.
            let beats = best.is_none_or(|(_, held)| {
                u128::from(self.len - parent.start) * u128::from(held.addr.span)
                    > u128::from(self.len - held.addr.start) * u128::from(parent.span)
            });
            if !beats {
                continue;
            }
            if let Some(bytes) = size(parent) {
                best = Some((
                    at,
                    Part {
                        addr: parent,
                        bytes,
                    },
                ));
            }
        }
        best
    }
}

/// The compactor's context: the coarsest aligned cover of a prefix `[0, k)`,
/// then every leaf from `k` on, for the smallest `k` that fits `budget`.
///
/// It rebuilds from scratch for each node, which OptChat §5.3 rules out for a
/// turn's view, so no view uses it. `size` is as for [`fold`]. An unbuilt
/// block reads as its built children. When even the finest built cover is over
/// budget, the oldest lines go.
pub(crate) fn context_tiling(
    len: u64,
    size: impl Fn(NodeAddr) -> Option<usize>,
    budget: usize,
) -> Vec<NodeAddr> {
    let mut cover = Cover {
        size: &size,
        bytes: HashMap::new(),
    };
    // Leaf bytes from the newest back, only as far as the budget reaches. A
    // `k` whose leaves alone overflow cannot fit, so the scan starts at `k0`.
    let mut suffix = vec![0usize];
    let mut k0 = len;
    while k0 > 0 {
        let grown = suffix[suffix.len() - 1] + size(NodeAddr::leaf(k0 - 1)).unwrap_or(0);
        if grown > budget {
            break;
        }
        suffix.push(grown);
        k0 -= 1;
    }
    let leaves_from = |k: u64| suffix[(len - k) as usize];

    let fits = (k0..=len).find(|&k| {
        let prefix: usize = blocks(k).into_iter().map(|b| cover.bytes(b)).sum();
        prefix + leaves_from(k) <= budget
    });
    let k = fits.unwrap_or(len);

    let mut tiles = Vec::new();
    for block in blocks(k) {
        cover.push(block, &mut tiles);
    }
    tiles.extend((k..len).map(NodeAddr::leaf));

    let mut total: usize = tiles.iter().map(|t| size(*t).unwrap_or(0)).sum();
    let mut drop = 0;
    while total > budget && drop < tiles.len() {
        total -= size(tiles[drop]).unwrap_or(0);
        drop += 1;
    }
    tiles.split_off(drop)
}

/// The coarsest built nodes covering `[0, end)`, oldest first: the aligned
/// blocks of [`blocks`], each unbuilt one replaced by its children.
pub(crate) fn built_cover(end: u64, size: impl Fn(NodeAddr) -> Option<usize>) -> Vec<NodeAddr> {
    let cover = Cover {
        size: &size,
        bytes: HashMap::new(),
    };
    let mut tiles = Vec::new();
    for block in blocks(end) {
        cover.push(block, &mut tiles);
    }
    tiles
}

/// The maximal aligned blocks covering `[0, end)`: the binary digits of `end`,
/// highest first.
fn blocks(end: u64) -> Vec<NodeAddr> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut bit = if end == 0 {
        0
    } else {
        1u64 << (63 - end.leading_zeros())
    };
    while bit > 0 {
        if end & bit != 0 {
            out.push(NodeAddr { start, span: bit });
            start += bit;
        }
        bit >>= 1;
    }
    out
}

/// A block's built cover and its bytes, memoised so a stalled compactor costs
/// one pass over the tree rather than one per candidate `k`.
struct Cover<'a, F: Fn(NodeAddr) -> Option<usize>> {
    size: &'a F,
    bytes: HashMap<NodeAddr, usize>,
}

impl<F: Fn(NodeAddr) -> Option<usize>> Cover<'_, F> {
    fn bytes(&mut self, addr: NodeAddr) -> usize {
        if let Some(&known) = self.bytes.get(&addr) {
            return known;
        }
        let bytes = match ((self.size)(addr), addr.children()) {
            (Some(own), _) => own,
            (None, Some((a, b))) => self.bytes(a) + self.bytes(b),
            (None, None) => 0,
        };
        self.bytes.insert(addr, bytes);
        bytes
    }

    fn push(&self, addr: NodeAddr, out: &mut Vec<NodeAddr>) {
        if (self.size)(addr).is_some() {
            out.push(addr);
        } else if let Some((a, b)) = addr.children() {
            self.push(a, out);
            self.push(b, out);
        }
    }
}
