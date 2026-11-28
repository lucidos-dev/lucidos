mod guard_tests {
    use std::path::{Path, PathBuf};

    use crate::test_support::source_scan::engine_source;

    const CACHE_RUNG: &[&str] = &["cache_rung", "CACHE_RUNG"];
    const INTELLIGENCE_RUNG: &[&str] = &[
        "intelligence_rung",
        "IntelligenceRung",
        "INTELLIGENCE_RUNG",
        "rung_estimates",
        "RungEstimates",
        "RUNG_ESTIMATES",
    ];

    const AREAS: &[(&str, &[&str])] = &[
        ("engine/summary_tree", CACHE_RUNG),
        ("runtime/chat_ladder_policy.rs", INTELLIGENCE_RUNG),
        ("runtime/intelligence_ladder.rs", INTELLIGENCE_RUNG),
        ("runtime/intelligence_ladder_policy.rs", INTELLIGENCE_RUNG),
        ("runtime/intelligence_ladder_tests.rs", INTELLIGENCE_RUNG),
        ("api/intelligence_ladder.rs", INTELLIGENCE_RUNG),
        (
            "../../lucidos-cli/src/intelligence_ladder.rs",
            INTELLIGENCE_RUNG,
        ),
    ];

    fn rust_files(area: &Path) -> Vec<PathBuf> {
        if area.is_file() {
            return vec![area.to_path_buf()];
        }
        let mut out = Vec::new();
        let mut stack = vec![area.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }
        out
    }

    fn code_of(text: &str) -> String {
        let mut code = String::new();
        let (mut in_string, mut in_comment, mut escaped) = (false, false, false);
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\n' {
                in_comment = false;
                code.push(c);
                continue;
            }
            if in_comment {
                continue;
            }
            if in_string {
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '"' => in_string = false,
                    _ => {}
                }
                code.push(' ');
                continue;
            }
            match c {
                '"' => in_string = true,
                '/' if chars.peek() == Some(&'/') => {
                    in_comment = true;
                    continue;
                }
                _ => {}
            }
            code.push(c);
        }
        code
    }

    fn bare_rungs(text: &str, qualifiers: &[&str]) -> Vec<String> {
        code_of(text)
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|word| word.to_lowercase().contains("rung"))
            .filter(|word| !qualifiers.iter().any(|q| word.contains(q)))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn every_rung_identifier_names_its_kind() {
        let mut found = Vec::new();
        for (area, qualifiers) in AREAS {
            for path in rust_files(&engine_source(area)) {
                let text = std::fs::read_to_string(&path).expect("read source");
                for word in bare_rungs(&text, qualifiers) {
                    found.push(format!("{}: {word}", path.display()));
                }
            }
        }
        assert!(
            found.is_empty(),
            "bare rung identifiers:\n{}",
            found.join("\n")
        );
    }

    #[test]
    fn the_scan_flags_a_bare_rung_and_skips_comments_and_strings() {
        let text = "fn rungs() {}\n// the rungs of a view\nlet cache_rungs = THREAD_CACHE_RUNGS;\n\
                #[serde(rename = \"rungs\")] intelligence_rungs: \"a \\\" \\\n rung\", rung";
        assert_eq!(
            bare_rungs(text, CACHE_RUNG),
            vec!["rungs", "intelligence_rungs", "rung"]
        );
        assert_eq!(
            bare_rungs(text, INTELLIGENCE_RUNG),
            vec!["rungs", "cache_rungs", "THREAD_CACHE_RUNGS", "rung"]
        );
    }
}
