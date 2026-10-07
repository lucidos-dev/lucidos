//! A Tree turn's *memory views* (ADR 0362), in the order the request sends
//! them: the view snapshot, the thread memory view, then the recent workspace
//! entries. A Classic turn never reaches this file, which is what keeps its
//! request byte-identical (I2).
//!
//! The snapshot and each marked thread view piece lead the message as memory
//! view blocks. Each ends a prefix the prompt cache can keep.

use uuid::Uuid;

use crate::engine::summary_tree::module::{Surface, ViewBudgets};
use crate::engine::summary_tree::view::{self, ThreadView};
use crate::engine::LucidosEngine;
use crate::llm::provider::{ContentBlock, MessageContent};

/// One turn's views. Each is framed, or empty.
#[derive(Debug, Default)]
pub(crate) struct TurnMemoryViews {
    /// Shared byte for byte by every thread of this view budget (I5).
    pub(crate) snapshot: String,
    pub(crate) thread: ThreadView,
    pub(crate) recent: String,
}

impl TurnMemoryViews {
    pub(crate) fn bytes(&self) -> usize {
        self.snapshot.len() + self.thread.text.len() + self.recent.len()
    }

    /// The workspace half, snapshot then recent block, for the context view.
    pub(crate) fn workspace(&self) -> String {
        [self.snapshot.as_str(), self.recent.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(super::context_mode::PART_SEPARATOR)
    }

    /// Put the views in front of the message. The snapshot, whose end other
    /// threads read through, and each thread view piece ending at a mark are
    /// memory view blocks. The thread view's rest is plain text.
    pub(crate) fn lead(&self, content: MessageContent) -> MessageContent {
        let mut blocks: Vec<ContentBlock> = Vec::new();
        if !self.snapshot.is_empty() {
            blocks.push(ContentBlock::MemoryView {
                text: self.snapshot.clone(),
            });
        }
        if !self.thread.text.is_empty() {
            let pieces = self.thread.pieces();
            let (rest, marked) = pieces.split_last().expect("pieces end with the rest");
            blocks.extend(marked.iter().map(|piece| ContentBlock::MemoryView {
                text: piece.to_string(),
            }));
            blocks.push(ContentBlock::Text {
                text: rest.to_string(),
            });
        }
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
                ThreadView::default()
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

    #[test]
    fn the_snapshot_leads_as_its_own_memory_view_block() {
        let views = TurnMemoryViews {
            snapshot: "[WORKSPACE MEMORY VIEW]\n".into(),
            ..Default::default()
        };
        let MessageContent::Blocks(blocks) = views.lead(MessageContent::Text("rest".into())) else {
            panic!("a led message is blocks");
        };
        assert!(
            matches!(&blocks[0], ContentBlock::MemoryView { text } if text.starts_with("[WORKSPACE"))
        );
        assert!(matches!(&blocks[1], ContentBlock::Text { text } if text == "rest"));
    }

    #[test]
    fn no_views_leave_the_message_alone() {
        let led = TurnMemoryViews::default().lead(MessageContent::Text("rest".into()));
        assert!(matches!(led, MessageContent::Text(text) if text == "rest"));
    }

    /// Each thread view piece ending at a mark is a memory view block, after
    /// the snapshot. The rest of the view is plain text, so its end, which
    /// moves every turn, is never a breakpoint.
    #[test]
    fn each_marked_thread_piece_leads_as_a_memory_view_block() {
        let text = "[THREAD MEMORY VIEW]\n[0+2] a\n[2+1] b\n[3+1] c\n[END THREAD MEMORY VIEW]";
        let first = text.find("[2+1]").unwrap();
        let second = text.find("[3+1]").unwrap();
        let views = TurnMemoryViews {
            snapshot: "[WORKSPACE MEMORY VIEW]\n".into(),
            thread: ThreadView {
                text: text.into(),
                marks: vec![first, second],
            },
            recent: String::new(),
        };
        let MessageContent::Blocks(blocks) = views.lead(MessageContent::Text("rest".into())) else {
            panic!("a led message is blocks");
        };
        let texts: Vec<(bool, &str)> = blocks
            .iter()
            .map(|b| match b {
                ContentBlock::MemoryView { text } => (true, text.as_str()),
                ContentBlock::Text { text } => (false, text.as_str()),
                other => panic!("unexpected block {other:?}"),
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                (true, "[WORKSPACE MEMORY VIEW]\n"),
                (true, &text[..first]),
                (true, &text[first..second]),
                (false, &text[second..]),
                (false, "rest"),
            ]
        );
    }
}
