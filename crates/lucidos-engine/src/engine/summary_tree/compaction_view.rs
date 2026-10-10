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

use super::fold::{Budget, Fold};
use super::prompt::flatten;
use super::view::{trailing_rungs, ViewBlocks};
use super::NodeAddr;

/// Where the sawtooth of the built prefix peaks; it drops to half.
pub(crate) const COMPACTION_VIEW_BYTES: usize = 32 * 1024;

/// Room for built leaves past the built prefix, whose merges are not built
/// yet.
const TAIL_BYTES: usize = 8 * 1024;

/// The most a compaction view holds.
pub(crate) const MAX_BYTES: usize = COMPACTION_VIEW_BYTES + TAIL_BYTES;

/// Marks per compaction view, on its last block ends: as many as a GPT
/// request takes. GPT reads an entry only at a mark in the very place an
/// earlier request wrote it.
pub(crate) const RUNGS: usize = crate::llm::openai::MAX_BREAKPOINTS;

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
        self.prefix.clone().into_tiles(end, size, MAX_BYTES)
    }
}

/// A stored line as a compaction view shows it.
fn line(text: &str) -> String {
    flatten(text) + "\n"
}

/// A line's bytes in a compaction view, from its stored text.
pub(crate) fn line_bytes(text: &str) -> usize {
    line(text).len()
}

/// `lines` as a compaction view, cut into blocks, with marks on its last
/// `rungs` block ends. Context no other call shares takes none.
pub(crate) fn render<'a>(lines: impl IntoIterator<Item = &'a str>, rungs: usize) -> ViewBlocks {
    ViewBlocks::render(OPEN, lines.into_iter().map(line), CLOSE, |blocks| {
        trailing_rungs(blocks, rungs)
    })
}
