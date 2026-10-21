//! A Tree turn's *memory views* (ADR 0362), in the order the request sends
//! them: the view snapshot, the thread memory view, then the recent workspace
//! entries. A Classic turn has no views, so only [`TurnMemoryViews::lead`]
//! reaches it.
//!
//! Both views lead the message in blocks of a few lines. A later request can
//! read the prompt cache up to any block end. Each block that takes a cache
//! mark is a memory view block. The stable context block sits between the two
//! views.

use uuid::Uuid;

use crate::engine::summary_tree::module::{Surface, ViewBudgets};
use crate::engine::summary_tree::view::{self, ViewBlocks};
use crate::engine::LucidosEngine;
use crate::llm::provider::{ContentBlock, MessageContent};

/// One turn's views. Each is framed, or empty.
#[derive(Debug, Default)]
pub(crate) struct TurnMemoryViews {
    /// Shared byte for byte by every thread of this view budget (I5).
    pub(crate) snapshot: ViewBlocks,
    pub(crate) thread: ViewBlocks,
    pub(crate) recent: String,
}

impl TurnMemoryViews {
    pub(crate) fn bytes(&self) -> usize {
        self.snapshot.text.len() + self.thread.text.len() + self.recent.len()
    }

    /// The workspace half, snapshot then recent block, for the context view.
    pub(crate) fn workspace(&self) -> String {
        [self.snapshot.text.as_str(), self.recent.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(super::context_mode::PART_SEPARATOR)
    }

    /// Put the views and the stable context block in front of the message:
    /// snapshot, stable block, thread view, one content block per view block.
    /// A marked view block is a memory view block, and every other block is
    /// plain text.
    ///
    /// The one place that orders a message's leading blocks, for Classic too.
    /// A Classic turn has no views, so its stable block leads alone.
    pub(crate) fn lead(&self, stable: String, content: MessageContent) -> MessageContent {
        let stable = (!stable.is_empty()).then_some(ContentBlock::StableContext { text: stable });
        let mut blocks: Vec<ContentBlock> = self
            .snapshot
            .blocks()
            .chain(stable)
            .chain(self.thread.blocks())
            .collect();
        if blocks.is_empty() {
            return content;
        }
        match content {
            MessageContent::Text(text) => blocks.push(ContentBlock::Text { text }),
            MessageContent::Blocks(rest) => blocks.extend(rest),
        }
        MessageContent::Blocks(blocks)
    }
}

impl LucidosEngine {
    /// The surface a chat thread's turn reads its workspace view at.
    pub(crate) async fn chat_surface(&self, thread_id: Uuid, is_trigger: bool) -> Surface {
        if is_trigger {
            return Surface::Trigger;
        }
        match crate::engine::home_thread::is_home_thread(&self.pool, thread_id).await {
            Ok(true) => Surface::Home,
            Ok(false) => Surface::Chat,
            Err(e) => {
                log!(
                    "[Chat] Could not tell whether {} is the home thread: {}",
                    thread_id,
                    e
                );
                Surface::Chat
            }
        }
    }

    /// The views for one turn, together at most `limit` bytes. A read that
    /// fails leaves its view empty and says so in the log: a turn never fails
    /// over its memory.
    pub(crate) async fn turn_memory_views(
        &self,
        thread_id: Uuid,
        surface: Surface,
        model: &str,
        user_message: &str,
        limit: usize,
    ) -> TurnMemoryViews {
        let budgets = ViewBudgets::resolve(&self.pool, surface, Some(model))
            .await
            .within(limit);
        let workspace = match self
            .view_snapshots
            .workspace_view(&self.pool, budgets.workspace)
            .await
        {
            Ok(workspace) => workspace,
            Err(e) => {
                log!("[Chat] The workspace memory view could not be read: {}", e);
                Default::default()
            }
        };
        let thread = view::thread_view(&self.pool, thread_id, budgets.thread, user_message)
            .await
            .unwrap_or_else(|e| {
                log!(
                    "[Chat] The memory view of thread {} could not be read: {}",
                    thread_id,
                    e
                );
                ViewBlocks::default()
            });
        TurnMemoryViews {
            snapshot: workspace.snapshot,
            thread,
            recent: workspace.recent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A view of `text`, cut where the `ends` lines start, marked where the `marks` lines start.
    fn blocks(text: &str, ends: &[&str], marks: &[&str]) -> ViewBlocks {
        let at = |line: &&str| text.find(line).expect("the line is in the text");
        ViewBlocks {
            text: text.into(),
            ends: ends.iter().map(at).collect(),
            marks: marks.iter().map(at).collect(),
        }
    }

    #[derive(Debug, PartialEq)]
    enum Kind {
        View,
        Stable,
        Text,
    }

    fn texts(content: MessageContent) -> Vec<(Kind, String)> {
        let MessageContent::Blocks(blocks) = content else {
            panic!("a led message is blocks");
        };
        blocks
            .into_iter()
            .map(|b| match b {
                ContentBlock::MemoryView { text } => (Kind::View, text),
                ContentBlock::StableContext { text } => (Kind::Stable, text),
                ContentBlock::Text { text } => (Kind::Text, text),
                other => panic!("unexpected block {other:?}"),
            })
            .collect()
    }

    #[test]
    fn no_views_and_no_stable_text_leave_the_message_alone() {
        let led =
            TurnMemoryViews::default().lead(String::new(), MessageContent::Text("rest".into()));
        assert!(matches!(led, MessageContent::Text(text) if text == "rest"));
    }

    /// Classic: no views, so the stable block alone leads the message.
    #[test]
    fn a_classic_message_leads_with_the_stable_block() {
        let led =
            TurnMemoryViews::default().lead("stable".into(), MessageContent::Text("rest".into()));
        assert_eq!(
            texts(led),
            vec![
                (Kind::Stable, "stable".to_string()),
                (Kind::Text, "rest".to_string())
            ]
        );
    }

    /// Every view block is its own content block, snapshot first, so each
    /// block end is a boundary the cache lookback can find. Only the marked
    /// ones are memory view blocks; the rest, which moves every turn, is text.
    /// The stable block sits between the two views: the snapshot stays first so
    /// every thread shares it (I3), and the thread view's rungs read the stable
    /// block through their own prefix.
    #[test]
    fn each_view_block_leads_as_its_own_block_and_the_marked_ones_are_memory_views() {
        let snapshot =
            "[WORKSPACE MEMORY VIEW]\n[w/0+4] a\n[w/4+1] b\n[END WORKSPACE MEMORY VIEW]\n";
        let thread = "[THREAD MEMORY VIEW]\n[0+2] a\n[2+1] b\n[3+1] c\n[END THREAD MEMORY VIEW]";
        let views = TurnMemoryViews {
            snapshot: blocks(snapshot, &["[w/4+1]"], &["[w/4+1]"]),
            thread: blocks(thread, &["[2+1]", "[3+1]"], &["[3+1]"]),
            recent: String::new(),
        };
        let (s, t) = (
            snapshot.find("[w/4+1]").unwrap(),
            [thread.find("[2+1]").unwrap(), thread.find("[3+1]").unwrap()],
        );
        assert_eq!(
            texts(views.lead("stable".into(), MessageContent::Text("rest".into()))),
            vec![
                (Kind::View, snapshot[..s].to_string()),
                (Kind::Text, snapshot[s..].to_string()),
                (Kind::Stable, "stable".to_string()),
                (Kind::Text, thread[..t[0]].to_string()),
                (Kind::View, thread[t[0]..t[1]].to_string()),
                (Kind::Text, thread[t[1]..].to_string()),
                (Kind::Text, "rest".to_string()),
            ]
        );
    }

    /// A new thread has an empty thread view: the stable block follows the
    /// snapshot directly.
    #[test]
    fn an_empty_thread_view_leaves_the_stable_block_behind_the_snapshot() {
        let snapshot = "[WORKSPACE MEMORY VIEW]\n[w/0+4] a\n[END WORKSPACE MEMORY VIEW]\n";
        let views = TurnMemoryViews {
            snapshot: blocks(snapshot, &[], &[]),
            ..TurnMemoryViews::default()
        };
        let led = texts(views.lead("stable".into(), MessageContent::Text("rest".into())));
        assert_eq!(led.last(), Some(&(Kind::Text, "rest".to_string())));
        let order: Vec<&Kind> = led.iter().map(|(kind, _)| kind).collect();
        let stable = order.iter().position(|k| **k == Kind::Stable).unwrap();
        assert!(order[..stable].iter().all(|k| **k != Kind::Stable));
        assert_eq!(led[stable - 1].1, snapshot);
    }
}
