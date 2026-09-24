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
    /// calls with no prose between them adds one break, not one each.
    pub(crate) fn break_paragraph(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with(PARAGRAPH_BREAK) {
            self.text.push_str(PARAGRAPH_BREAK);
        }
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
        Text(&'static str),
        Tool,
    }

    /// Drive the buffer the way the loop does: flush after every text chunk,
    /// and flush then break the paragraph at every tool call. Returns every
    /// chunk that would have been stored, plus the buffer.
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
                Step::Text(text) => {
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
        let (chunks, _) = run(&[Text("A"), Tool, Tool, Tool, Tool, Text("B"), Tool, Tool]);
        assert_eq!(chunks, ["A", "\n\nB"]);
    }

    #[test]
    fn the_paragraph_break_between_two_blocks_survives() {
        use Step::*;
        let (chunks, _) = run(&[Text("A"), Tool, Tool, Text("B")]);
        assert_eq!(chunks.concat(), "A\n\nB");
    }

    #[test]
    fn prose_already_ending_in_a_break_gets_no_second_one() {
        use Step::*;
        let (chunks, _) = run(&[Text("A\n\n"), Tool, Tool, Text("B")]);
        assert_eq!(chunks.concat(), "A\n\nB");
        assert_no_whitespace_only(&chunks);
    }

    #[test]
    fn whitespace_alone_is_never_stored() {
        use Step::*;
        for steps in [
            vec![Tool, Tool],
            vec![Text("\n\n"), Tool, Text("  \n")],
            vec![Text("A"), Tool, Text("\n\n"), Tool],
        ] {
            let (chunks, _) = run(&steps);
            assert_no_whitespace_only(&chunks);
        }
    }

    #[test]
    fn a_whitespace_text_chunk_leads_the_next_prose_chunk() {
        use Step::*;
        let (chunks, _) = run(&[Text("A"), Text("\n\n"), Text("B")]);
        assert_eq!(chunks, ["A", "\n\nB"]);
    }

    #[test]
    fn stored_chunks_join_to_the_buffer_minus_a_whitespace_tail() {
        use Step::*;
        let (chunks, buf) = run(&[
            Text("One"),
            Tool,
            Text("Two\n"),
            Tool,
            Tool,
            Text("Three"),
            Tool,
        ]);
        let joined = chunks.concat();
        let tail = buf
            .as_str()
            .strip_prefix(joined.as_str())
            .expect("chunks are a prefix");
        assert!(tail.trim().is_empty());
        assert_eq!(joined, "One\n\nTwo\n\n\nThree");
    }

    #[test]
    fn multibyte_text_flushes_on_char_boundaries() {
        use Step::*;
        let (chunks, _) = run(&[Text("Blåbær"), Tool, Text("ørret")]);
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
