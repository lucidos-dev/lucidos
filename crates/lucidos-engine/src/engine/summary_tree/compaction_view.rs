//! The *compaction view*: the context a compactor call reads (OptChat §4).
//!
//! It is a memory view's fold at a smaller budget. It grows one line per
//! built entry, and once past [`COMPACTION_VIEW_BYTES`] one batch merges it
//! down to half. So consecutive calls share all of it but its end, and read
//! that from the prompt cache.
//!
//! It holds only built lines. A drain keeps one per tree and advances it as
//! the built prefix grows. Built nodes never change, so that equals a replay
//! from entry 0. A node reads it up to its own end, and never past the first
//! unbuilt leaf.
//!
//! **The tail never rewrites the prefix.** Lines past the built prefix fold
//! among themselves within their own room, and a mark sits at the prefix's
//! end. So a call reads the prefix from the cache however long the tail
//! grows. A tree whose leaves build in any order grows a long one.

use super::fold::{Budget, Fold};
use super::prompt::flatten;
use super::view::{trailing_rungs, ViewBlocks};
use super::NodeAddr;

/// Where the sawtooth of the built prefix peaks; it drops to half.
pub(crate) const COMPACTION_VIEW_BYTES: usize = 32 * 1024;

/// Room for built lines past the built prefix. Every call's tail differs, so
/// it is what a call sends uncached; it stays this small however much room
/// the prefix leaves.
const TAIL_BYTES: usize = 8 * 1024;

/// The most a compaction view holds.
pub(crate) const MAX_BYTES: usize = COMPACTION_VIEW_BYTES + TAIL_BYTES;

/// Marks on a compaction view's last block ends: what an explicit-mode GPT
/// request takes, less the one at its prefix's end. GPT reads an entry only
/// at a mark in the very place an earlier request wrote it.
const TRAILING_RUNGS: usize = crate::llm::openai::MAX_EXPLICIT_BREAKPOINTS - 1;

const OPEN: &str = "<chat>\n";
const CLOSE: &str = "</chat>\n";

/// One tree's compaction view, kept for a drain.
#[derive(Default)]
pub(crate) struct CompactionView {
    prefix: Fold,
}

impl CompactionView {
    /// Fold in the entries up to `built_prefix`, over which every node is
    /// built.
    pub(crate) fn advance(&mut self, built_prefix: u64, size: &impl Fn(NodeAddr) -> Option<usize>) {
        self.prefix
            .advance(built_prefix, size, Budget::sawtooth(COMPACTION_VIEW_BYTES));
    }

    /// The nodes a call reads whose context ends at `end`. Every leaf below
    /// `end` is built, so the view holds no placeholder.
    pub(crate) fn tiles(
        &self,
        end: u64,
        size: &impl Fn(NodeAddr) -> Option<usize>,
    ) -> Vec<NodeAddr> {
        debug_assert!(
            end >= self.prefix.entries(),
            "a node ends inside the built prefix"
        );
        let tail = TAIL_BYTES.min(MAX_BYTES.saturating_sub(self.prefix.bytes()));
        self.prefix.with_tail(end, size, tail)
    }

    /// Lines of the built prefix, which lead every call's tiles unchanged.
    pub(crate) fn prefix_lines(&self) -> usize {
        self.prefix.lines()
    }
}

/// Which prefixes of a view of `blocks` whole blocks take a mark, ascending,
/// as block counts: the last whole block of its first `prefix_lines` lines,
/// then its last block ends.
pub(crate) fn mark_blocks(blocks: usize, prefix_lines: usize) -> Vec<usize> {
    let mut marks = trailing_rungs(blocks, TRAILING_RUNGS);
    let prefix = prefix_lines / super::view::BLOCK_LINES;
    if prefix > 0 && !marks.contains(&prefix) {
        marks.insert(0, prefix);
    }
    marks
}

/// A stored line as a compaction view shows it.
fn line(text: &str) -> String {
    flatten(text) + "\n"
}

/// A line's bytes in a compaction view, from its stored text.
pub(crate) fn line_bytes(text: &str) -> usize {
    line(text).len()
}

/// `lines` as a compaction view, cut into blocks, marked by [`mark_blocks`]
/// for `view`'s prefix.
pub(crate) fn render<'a>(
    lines: impl IntoIterator<Item = &'a str>,
    view: &CompactionView,
) -> ViewBlocks {
    let prefix = view.prefix_lines();
    ViewBlocks::render(OPEN, lines.into_iter().map(line), CLOSE, |blocks| {
        mark_blocks(blocks, prefix)
    })
}

/// `lines` as context no other call shares, so it takes no mark: a mark
/// there would write an entry nothing reads.
pub(crate) fn render_unshared<'a>(lines: impl IntoIterator<Item = &'a str>) -> ViewBlocks {
    ViewBlocks::render(OPEN, lines.into_iter().map(line), CLOSE, |_| Vec::new())
}
