//! What Lucidos stores of a coding agent's tool output.
//!
//! The agent reads its own output inside its own process. Lucidos parses a
//! copy of the stream to draw the steps, and this copy is what the step modal
//! shows. So it is stored whole: the snapshot and the live stream both leave
//! it out, and the modal fetches it on demand.

use std::collections::HashMap;

/// The text a `CodingAgentToolResult` stores for one tool's output.
///
/// Whole, with two cleanups. A NUL byte would fail the jsonb insert and lose
/// the event. A Postgres password gets the mask the call's args already get.
pub(super) fn stored_tool_output(output: &str) -> String {
    crate::core::redact_postgres_secrets(&crate::core::sanitize_for_jsonb(output))
}

/// Each open tool call's name, by `tool_use_id`.
///
/// A coding agent's result frame carries only the id, so the name comes from
/// the call it answers. An id never seen, such as a straggler, yields an empty
/// name, as every row did before.
#[derive(Default)]
pub(super) struct ToolNamesById(HashMap<String, String>);

impl ToolNamesById {
    pub(super) fn record(&mut self, id: &str, name: &str) {
        self.0.insert(id.to_string(), name.to_string());
    }

    /// The call's name, forgotten once taken. Each call has one result.
    pub(super) fn take(&mut self, id: &str) -> String {
        self.0.remove(id).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_output_is_stored_whole() {
        let output = "line of cargo test output\n".repeat(200);
        assert!(output.chars().count() > 5_000);
        assert_eq!(stored_tool_output(&output), output);
    }

    #[test]
    fn a_short_output_is_stored_unchanged() {
        assert_eq!(stored_tool_output("ok"), "ok");
    }

    #[test]
    fn multibyte_output_survives_whole() {
        let output = "æøå 🦀 ".repeat(100);
        assert_eq!(stored_tool_output(&output), output);
    }

    #[test]
    fn nul_bytes_are_removed_so_the_insert_cannot_fail() {
        let stored = stored_tool_output("before\u{0}after");
        assert!(!stored.contains('\u{0}'), "got {stored:?}");
        assert!(stored.contains("before") && stored.contains("after"));
    }

    #[test]
    fn a_postgres_password_is_masked() {
        let stored =
            stored_tool_output("DATABASE_URL=postgres://lucidos:hunter2@localhost:5432/db");
        assert!(!stored.contains("hunter2"), "got {stored:?}");
        assert!(stored.contains("postgres://lucidos:***@localhost"));
    }

    #[test]
    fn a_result_takes_the_name_of_the_call_it_answers() {
        let mut names = ToolNamesById::default();
        names.record("toolu_1", "Bash");
        names.record("toolu_2", "Read");
        assert_eq!(names.take("toolu_2"), "Read");
        assert_eq!(names.take("toolu_1"), "Bash");
    }

    #[test]
    fn an_unseen_or_already_answered_id_has_no_name() {
        let mut names = ToolNamesById::default();
        assert_eq!(names.take("toolu_never"), "");
        names.record("toolu_1", "Bash");
        names.take("toolu_1");
        assert_eq!(names.take("toolu_1"), "");
    }
}
