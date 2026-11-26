use std::collections::HashMap;

pub(super) fn stored_tool_output(output: &str, secrets: &[String]) -> String {
    crate::core::redact_subprocess_output(&crate::core::sanitize_for_jsonb(output), secrets)
}

#[derive(Default)]
pub(super) struct ToolNamesById(HashMap<String, String>);

impl ToolNamesById {
    pub(super) fn record(&mut self, id: &str, name: &str) {
        self.0.insert(id.to_string(), name.to_string());
    }

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
        assert_eq!(stored_tool_output(&output, &[]), output);
    }

    #[test]
    fn a_short_output_is_stored_unchanged() {
        assert_eq!(stored_tool_output("ok", &[]), "ok");
    }

    #[test]
    fn multibyte_output_survives_whole() {
        let output = "æøå 🦀 ".repeat(100);
        assert_eq!(stored_tool_output(&output, &[]), output);
    }

    #[test]
    fn nul_bytes_are_removed_so_the_insert_cannot_fail() {
        let stored = stored_tool_output("before\u{0}after", &[]);
        assert!(!stored.contains('\u{0}'), "got {stored:?}");
        assert!(stored.contains("before") && stored.contains("after"));
    }

    #[test]
    fn a_postgres_password_is_masked() {
        let stored = stored_tool_output(
            "DATABASE_URL=postgres://lucidos:hunter2@localhost:5432/db",
            &[],
        );
        assert!(!stored.contains("hunter2"), "got {stored:?}");
        assert!(stored.contains("postgres://lucidos:***@localhost"));
    }

    #[test]
    fn a_printed_token_and_a_session_secret_are_masked() {
        let secrets = vec!["session-origin-token-value".to_string()];
        let stored = stored_tool_output(
            "GH_TOKEN=ghp_Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7d\n\
             LUCIDOS_AGENT_ORIGIN_TOKEN=session-origin-token-value",
            &secrets,
        );
        assert_eq!(
            stored,
            "GH_TOKEN=ghp_[REDACTED]\nLUCIDOS_AGENT_ORIGIN_TOKEN=[REDACTED]"
        );
    }

    #[test]
    fn a_token_in_a_tool_call_input_is_masked() {
        let secrets = vec!["session-origin-token-value".to_string()];
        let mut input = serde_json::json!({
            "command": "curl -H 'Authorization: Bearer ghp_Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7d'",
            "nested": ["psql postgres://lucidos:hunter2@localhost/db", "session-origin-token-value"],
            "session-origin-token-value": { "env": { "session-origin-token-value": 1 } },
        });
        crate::core::redact_subprocess_output_in_json(&mut input, &secrets);
        assert_eq!(
            input,
            serde_json::json!({
                "command": "curl -H 'Authorization: Bearer ghp_[REDACTED]'",
                "nested": ["psql postgres://lucidos:***@localhost/db", "[REDACTED]"],
                "[REDACTED]": { "env": { "[REDACTED]": 1 } },
            })
        );
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
