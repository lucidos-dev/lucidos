use super::NodeAddr;

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

/// INVARIANT: The longest prefix `[0, f)` of a log of `len` entries over which every node
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

#[derive(Clone, Copy)]
pub(crate) struct Budget {
    high: usize,
    low: usize,
}

impl Budget {
    pub(crate) fn sawtooth(high: usize) -> Self {
        Self {
            high,
            low: high / 2,
        }
    }

    pub(crate) fn exact(bytes: usize) -> Self {
        Self {
            high: bytes,
            low: bytes,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Fold {
    parts: Vec<Part>,
    bytes: usize,
    len: u64,
    batching: bool,
}

#[derive(Clone, Copy)]
struct Part {
    addr: NodeAddr,
    bytes: usize,
}

impl Fold {
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

    /// INVARIANT: The view at `len` entries with this fold's lines left as they are.
    /// The entries past it fold among themselves within `tail_budget`
    /// bytes. So a view that reads this fold as its prefix never rewrites it,
    /// however long the rest grows.
    pub(crate) fn with_tail(
        &self,
        len: u64,
        size: &impl Fn(NodeAddr) -> Option<usize>,
        tail_budget: usize,
    ) -> Vec<NodeAddr> {
        let tail = Fold {
            len: self.len,
            ..Fold::default()
        };
        self.parts
            .iter()
            .map(|p| p.addr)
            .chain(tail.into_tiles(len, size, tail_budget))
            .collect()
    }

    pub(crate) fn entries(&self) -> u64 {
        self.len
    }

    pub(crate) fn lines(&self) -> usize {
        self.parts.len()
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub(crate) fn addrs(&self) -> Vec<NodeAddr> {
        self.parts.iter().map(|p| p.addr).collect()
    }

    /// INVARIANT: Where the sibling pair with the largest `due` and a built parent
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

pub fn built_cover(end: u64, size: impl Fn(NodeAddr) -> Option<usize>) -> Vec<NodeAddr> {
    let mut tiles = Vec::new();
    for block in blocks(end) {
        push_built(block, &size, &mut tiles);
    }
    tiles
}

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

fn push_built(addr: NodeAddr, size: &impl Fn(NodeAddr) -> Option<usize>, out: &mut Vec<NodeAddr>) {
    if size(addr).is_some() {
        out.push(addr);
    } else if let Some((a, b)) = addr.children() {
        push_built(a, size, out);
        push_built(b, size, out);
    }
}
