//! The buffer behind a coding-agent turn's `CodingAgentTextStreamed` events.

use uuid::Uuid;

const PARAGRAPH_BREAK: &str = "\n\n";

/// A turn's assistant text, and how much of it is already persisted.
///
/// A tail holding only whitespace is never flushed. It stays in the buffer and
/// leads the next chunk that holds prose. So a paragraph break between two text
/// blocks survives, without a stored event of its own.
#[derive(Default)]
pub(crate) struct CodingAgentTextBuffer {
    text: String,
    persisted_len: usize,
}

impl CodingAgentTextBuffer {
    pub(crate) fn push(&mut self, text: &str) {
        self.text.push_str(text);
    }

    /// Make the next text start a new paragraph. Idempotent, so a run of tool
    /// calls with no prose between them adds one break, not one each. Text
    /// ending in one newline gets the second only.
    pub(crate) fn break_paragraph(&mut self) {
        if self.text.is_empty() || self.text.ends_with(PARAGRAPH_BREAK) {
            return;
        }
        let missing = if self.text.ends_with('\n') {
            "\n"
        } else {
            PARAGRAPH_BREAK
        };
        self.text.push_str(missing);
    }

    /// The tail not yet persisted, or `None` while it holds only whitespace.
    fn unflushed(&self) -> Option<&str> {
        let tail = &self.text[self.text.floor_char_boundary(self.persisted_len)..];
        (!tail.trim().is_empty()).then_some(tail)
    }

    fn mark_flushed(&mut self) {
        self.persisted_len = self.text.len();
    }

    /// Emit the unflushed tail as one `CodingAgentTextStreamed` and mark it
    /// persisted. Does nothing while the tail holds only whitespace.
    pub(crate) async fn flush(
        &mut self,
        event_bus: &crate::engine::event_bus::EventBus,
        thread_id: Uuid,
        coding_agent: crate::runtime::CodingAgent,
        meta: &crate::engine::thread_events::EventMeta,
        label: &'static str,
    ) {
        let Some(tail) = self.unflushed() else {
            return;
        };
        event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentTextStreamed {
                        text: tail.to_string(),
                        coding_agent,
                    },
                    meta: meta.clone(),
                },
                label,
            )
            .await;
        self.mark_flushed();
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.persisted_len = 0;
    }

    pub(crate) fn into_text(self) -> String {
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::CodingAgentTextBuffer;

    /// One step of a turn, as the agent loop sees it.
    enum Step {
        /// Text that opens a new content block.
        Block(&'static str),
        /// More text for the block already open.
        Delta(&'static str),
        Tool,
    }

    /// Drive the buffer the way the loop does: break the paragraph where a
    /// block opens, flush after every text chunk, and flush then break the
    /// paragraph at every tool call. Returns every chunk that would have been
    /// stored, plus the buffer.
    fn run(steps: &[Step]) -> (Vec<String>, CodingAgentTextBuffer) {
        let mut buf = CodingAgentTextBuffer::default();
        let mut chunks = Vec::new();
        let mut flush = |buf: &mut CodingAgentTextBuffer| {
            if let Some(tail) = buf.unflushed() {
                chunks.push(tail.to_string());
                buf.mark_flushed();
            }
        };
        for step in steps {
            match step {
                Step::Block(text) => {
                    buf.break_paragraph();
                    buf.push(text);
                    flush(&mut buf);
                }
                Step::Delta(text) => {
                    buf.push(text);
                    flush(&mut buf);
                }
                Step::Tool => {
                    flush(&mut buf);
                    buf.break_paragraph();
                }
            }
        }
        flush(&mut buf);
        (chunks, buf)
    }

    fn assert_no_whitespace_only(chunks: &[String]) {
        for chunk in chunks {
            assert!(
                !chunk.trim().is_empty(),
                "stored a whitespace-only chunk: {chunk:?}"
            );
        }
    }

    #[test]
    fn a_run_of_silent_tool_calls_stores_no_bare_paragraph_breaks() {
        use Step::*;
        let (chunks, _) = run(&[Block("A"), Tool, Tool, Tool, Tool, Block("B"), Tool, Tool]);
        assert_eq!(chunks, ["A", "\n\nB"]);
    }

    #[test]
    fn the_paragraph_break_between_two_blocks_survives() {
        use Step::*;
        let (chunks, _) = run(&[Block("A"), Tool, Tool, Block("B")]);
        assert_eq!(chunks.concat(), "A\n\nB");
    }

    #[test]
    fn two_blocks_with_no_tool_call_between_are_two_paragraphs() {
        use Step::*;
        let (chunks, _) = run(&[Block("then hardening."), Block("Not done yet.")]);
        assert_eq!(chunks.concat(), "then hardening.\n\nNot done yet.");
    }

    #[test]
    fn a_block_ending_in_a_newline_is_completed_not_doubled() {
        use Step::*;
        let (chunks, _) = run(&[Block("A\n"), Block("B")]);
        assert_eq!(chunks.concat(), "A\n\nB");
    }

    #[test]
    fn the_first_block_of_a_turn_gets_no_leading_break() {
        use Step::*;
        let (chunks, _) = run(&[Block("A")]);
        assert_eq!(chunks, ["A"]);
    }

    #[test]
    fn deltas_within_one_block_join_with_no_separator() {
        use Step::*;
        let (chunks, _) = run(&[Block("Hel"), Delta("lo"), Delta(" world."), Block("Next")]);
        assert_eq!(chunks.concat(), "Hello world.\n\nNext");
    }

    #[test]
    fn prose_already_ending_in_a_break_gets_no_second_one() {
        use Step::*;
        for steps in [
            vec![Block("A\n\n"), Tool, Tool, Block("B")],
            vec![Block("A\n\n"), Block("B")],
        ] {
            let (chunks, _) = run(&steps);
            assert_eq!(chunks.concat(), "A\n\nB");
            assert_no_whitespace_only(&chunks);
        }
    }

    #[test]
    fn whitespace_alone_is_never_stored() {
        use Step::*;
        for steps in [
            vec![Tool, Tool],
            vec![Block("\n\n"), Tool, Block("  \n")],
            vec![Block("A"), Tool, Block("\n\n"), Tool],
            vec![Block("A"), Block("\n"), Block("  ")],
        ] {
            let (chunks, _) = run(&steps);
            assert_no_whitespace_only(&chunks);
        }
    }

    #[test]
    fn a_whitespace_text_chunk_leads_the_next_prose_chunk() {
        use Step::*;
        let (chunks, _) = run(&[Block("A"), Delta("\n\n"), Delta("B")]);
        assert_eq!(chunks, ["A", "\n\nB"]);
    }

    #[test]
    fn stored_chunks_join_to_the_buffer_minus_a_whitespace_tail() {
        use Step::*;
        let (chunks, buf) = run(&[
            Block("One"),
            Tool,
            Block("Two\n"),
            Tool,
            Tool,
            Block("Three"),
            Block("Four"),
            Tool,
        ]);
        let joined = chunks.concat();
        let tail = buf
            .as_str()
            .strip_prefix(joined.as_str())
            .expect("chunks are a prefix");
        assert!(tail.trim().is_empty());
        assert_eq!(joined, "One\n\nTwo\n\nThree\n\nFour");
    }

    #[test]
    fn multibyte_text_flushes_on_char_boundaries() {
        use Step::*;
        let (chunks, _) = run(&[Block("Blåbær"), Tool, Block("ørret")]);
        assert_eq!(chunks, ["Blåbær", "\n\nørret"]);
    }

    #[test]
    fn clear_starts_the_next_turn_empty() {
        let mut buf = CodingAgentTextBuffer::default();
        buf.push("A");
        buf.mark_flushed();
        buf.clear();
        buf.break_paragraph();
        assert!(buf.is_empty());
        buf.push("B");
        assert_eq!(buf.unflushed(), Some("B"));
    }
}
