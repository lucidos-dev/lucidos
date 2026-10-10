//! The fold: which nodes of a summary tree a *memory view* shows (OptChat
//! §3.2).
//!
//! A view appends each entry's line at its end, and nothing else changes.
//! Once it passes its budget, one batch merges the most due pairs of sibling
//! lines until the view is half its budget: a sawtooth. A merged line never
//! splits. Between batches each view is a prefix of the next, so a turn reads
//! the last turn's whole view from the prompt cache. Holding merges for a
//! batch rewrites far fewer lines than merging at every entry.
//!
//! Every read replays the view from entry 0, so a restart loses nothing. The
//! replay covers only the *built prefix*, where every node is built. Those
//! nodes never change, so a longer built prefix replays to the shorter one's
//! view plus appends and merges. Newer entries follow as leaves, in a reserve
//! the prefix replay leaves free. Why: the ADR 0362 amendments.

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
    view.advance(
        built_prefix,
        &size,
        Budget::sawtooth(budget.saturating_sub(reserve)),
    );
    view.into_tiles(len, &size, budget)
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

/// When a view merges: a batch starts once it passes `high` bytes, and merges
/// until it is at most `low`.
#[derive(Clone, Copy)]
pub(crate) struct Budget {
    high: usize,
    low: usize,
}

impl Budget {
    /// A view that grows to `high` bytes, then drops to half of it at once.
    pub(crate) fn sawtooth(high: usize) -> Self {
        Self {
            high,
            low: high / 2,
        }
    }

    /// A view that merges at every entry to stay at `bytes`.
    pub(crate) fn exact(bytes: usize) -> Self {
        Self {
            high: bytes,
            low: bytes,
        }
    }
}

/// A view as it folds, one entry at a time.
#[derive(Clone, Default)]
pub(crate) struct Fold {
    parts: Vec<Part>,
    bytes: usize,
    /// Entries appended so far, the `T` of OptChat's age rule.
    len: u64,
    /// A batch has started and not yet reached its low mark, because some
    /// pair's parent was unbuilt. It goes on at the next entry.
    batching: bool,
}

#[derive(Clone, Copy)]
struct Part {
    addr: NodeAddr,
    bytes: usize,
}

impl Fold {
    /// Append the next entry's leaf. Past `budget.high`, or in a batch not
    /// yet done, merge the most due built pairs down to `budget.low`.
    pub(crate) fn push(&mut self, size: &impl Fn(NodeAddr) -> Option<usize>, budget: Budget) {
        let leaf = NodeAddr::leaf(self.len);
        let bytes = size(leaf).unwrap_or(0);
        self.parts.push(Part { addr: leaf, bytes });
        self.bytes += bytes;
        self.len += 1;
        self.batching |= self.bytes > budget.high;
        while self.batching && self.bytes > budget.low {
            let Some((at, parent)) = self.most_due(size) else {
                return;
            };
            self.bytes -= self.parts[at].bytes + self.parts[at + 1].bytes;
            self.bytes += parent.bytes;
            self.parts.splice(at..at + 2, [parent]);
        }
        self.batching = false;
    }

    /// Push entries until `len` of them are in.
    pub(crate) fn advance(
        &mut self,
        len: u64,
        size: &impl Fn(NodeAddr) -> Option<usize>,
        budget: Budget,
    ) {
        while self.len < len {
            self.push(size, budget);
        }
    }

    /// The view at `len` entries: the entries past this fold join it within
    /// `budget`, merging at each one as needed. When no built pair is left to
    /// merge, the oldest lines go, so no view exceeds its budget (I6).
    pub(crate) fn into_tiles(
        mut self,
        len: u64,
        size: &impl Fn(NodeAddr) -> Option<usize>,
        budget: usize,
    ) -> Vec<NodeAddr> {
        self.advance(len, size, Budget::exact(budget));
        let mut drop = 0;
        while self.bytes > budget && drop < self.parts.len() {
            self.bytes -= self.parts[drop].bytes;
            drop += 1;
        }
        self.parts[drop..].iter().map(|p| p.addr).collect()
    }

    /// Entries appended so far.
    pub(crate) fn entries(&self) -> u64 {
        self.len
    }

    #[cfg(test)]
    pub(crate) fn addrs(&self) -> Vec<NodeAddr> {
        self.parts.iter().map(|p| p.addr).collect()
    }

    /// Where the sibling pair with the largest `due` and a built parent
    /// starts, and that parent. Ties go to the oldest pair.
    ///
    /// `due = (T - last) / 2^l`: how long ago the pair's last entry was, in
    /// the pair's own line size. Measured from the pair's first entry
    /// instead, near ties it merges old pairs and rewrites old lines.
    fn most_due(&self, size: &impl Fn(NodeAddr) -> Option<usize>) -> Option<(usize, Part)> {
        let due = |parent: NodeAddr| (self.len + 1 - parent.end(), parent.span / 2);
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
                let ((age, line), (held_age, held_line)) = (due(parent), due(held.addr));
                u128::from(age) * u128::from(held_line) > u128::from(held_age) * u128::from(line)
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

/// The coarsest built nodes covering `[0, end)`, oldest first: the aligned
/// blocks of [`blocks`], each unbuilt one replaced by its children.
pub(crate) fn built_cover(end: u64, size: impl Fn(NodeAddr) -> Option<usize>) -> Vec<NodeAddr> {
    let mut tiles = Vec::new();
    for block in blocks(end) {
        push_built(block, &size, &mut tiles);
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

/// `addr` if it is built, else its built children, oldest first.
fn push_built(addr: NodeAddr, size: &impl Fn(NodeAddr) -> Option<usize>, out: &mut Vec<NodeAddr>) {
    if size(addr).is_some() {
        out.push(addr);
    } else if let Some((a, b)) = addr.children() {
        push_built(a, size, out);
        push_built(b, size, out);
    }
}
