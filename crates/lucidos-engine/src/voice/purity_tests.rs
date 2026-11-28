mod guard_tests {
    use crate::test_support::source_scan::{engine_source, production_sources};

    const PROMPT_PATH_PREFIXES: &[&str] = &[
        "engine/chat/",
        "engine/agent_session/prompts.rs",
        "engine/context.rs",
        "engine/tools/capabilities.rs",
    ];

    const PROMPT_ASSEMBLY_FILES: &[&str] = &[
        "engine/chat/process/context_build.rs",
        "engine/chat/process/context_mode.rs",
        "engine/chat/process/context_sections.rs",
        "engine/chat/process/history.rs",
        "engine/chat/process/system_prompt.rs",
        "engine/chat/process/working_understanding.rs",
        "engine/chat/process/workspace_prompt_footprint.rs",
        "engine/agent_session/prompts.rs",
        "engine/context.rs",
        "engine/tools/capabilities.rs",
    ];

    const ALLOWED_PHRASES: &[&str] = &["active voice", "passive voice"];

    fn scrubbed(text: &str) -> String {
        let mut haystack = text.to_lowercase();
        for phrase in ALLOWED_PHRASES {
            haystack = haystack.replace(phrase, "");
        }
        haystack
    }

    fn prompt_path_sources() -> Vec<(String, String)> {
        let sources: Vec<(String, String)> = production_sources()
            .into_iter()
            .filter(|(rel, _)| PROMPT_PATH_PREFIXES.iter().any(|p| rel.starts_with(p)))
            .collect();
        assert!(
            sources.len() > 10,
            "the prompt-path prefixes matched almost nothing, so this guard is \
         scanning the wrong tree: {:?}",
            sources.iter().map(|(rel, _)| rel).collect::<Vec<_>>()
        );
        sources
    }

    fn string_literals(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c != '"' {
                continue;
            }
            let mut body = String::new();
            for inner in chars.by_ref() {
                match inner {
                    '"' => break,
                    _ => body.push(inner),
                }
            }
            out.push(body);
        }
        out
    }

    #[test]
    fn nothing_the_doer_reads_mentions_voice() {
        let mut offenders = Vec::new();
        for (rel, text) in prompt_path_sources() {
            for literal in string_literals(&scrubbed(&text)) {
                if literal.contains("voice") {
                    offenders.push(format!("{}: {:?}", rel, literal));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "The doer must never be told a voice session is live (ADR 0149). \
         These prompt-path literals mention voice: {:?}",
            offenders
        );
    }

    fn passes_the_field_along(code: &str) -> bool {
        let line = code.trim();
        line == "voice_session_id," || line.starts_with("voice_session_id: Option<Uuid>")
    }

    #[test]
    fn the_prompt_path_only_passes_the_field_along() {
        let mut offenders = Vec::new();
        for (rel, text) in prompt_path_sources() {
            for (index, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if !scrubbed(code).contains("voice") {
                    continue;
                }
                if passes_the_field_along(code) {
                    continue;
                }
                offenders.push(format!("{}:{}", rel, index + 1));
            }
        }
        assert!(
            offenders.is_empty(),
            "A turn may pass `voice_session_id` along and nothing else (ADR 0149). \
         These lines do something else with voice: {:?}",
            offenders
        );
    }

    #[test]
    fn the_doers_prompt_assembly_never_mentions_voice() {
        let mut offenders = Vec::new();
        for (rel, text) in prompt_path_sources() {
            if !PROMPT_ASSEMBLY_FILES.contains(&rel.as_str()) {
                continue;
            }
            for (index, line) in scrubbed(&text).lines().enumerate() {
                if line.contains("voice") {
                    offenders.push(format!("{}:{}", rel, index + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "Prompt assembly must not know voice exists (ADR 0149). These lines \
         mention it: {:?}",
            offenders
        );
    }

    #[test]
    fn nothing_on_the_prompt_path_reads_the_live_session_registry() {
        for (rel, text) in prompt_path_sources() {
            assert!(
                !text.contains("voice_sessions"),
                "{} reads the live voice-session registry (ADR 0149)",
                rel
            );
        }
    }

    #[test]
    fn every_prompt_path_still_exists() {
        for prefix in PROMPT_PATH_PREFIXES {
            let path = engine_source(prefix.trim_end_matches('/'));
            assert!(path.exists(), "{} no longer exists", prefix);
        }
        for file in PROMPT_ASSEMBLY_FILES {
            let path = engine_source(file);
            assert!(path.exists(), "{} no longer exists", file);
        }
    }
}
