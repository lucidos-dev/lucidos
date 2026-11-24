//! The guard tests' own guard (ADR 0694).
//!
//! A *guard test* reads the source tree at run time: it scans the codebase,
//! the docs or the scripts. Every `/harden` round runs the guard tests, by the
//! filter `guard_tests`, while the full engine suite runs only in the main
//! check. So a test that reads the tree must live in a module named
//! `guard_tests`, or a round would never run it.

use std::path::Path;

/// What marks a line that reads the source tree at run time.
const READS_THE_TREE: &[&str] = &["CARGO_MANIFEST_DIR", "source_scan::", "route_scan::"];

/// The checkout-root helpers in `paths`, matched as whole names.
const REPO_ROOT_CALLS: &[&str] = &["repo_root(", "repo_or_workspace_root("];

fn reads_the_tree(line: &str) -> bool {
    READS_THE_TREE.iter().any(|m| line.contains(m))
        || REPO_ROOT_CALLS.iter().any(|call| {
            // A `.repo_root()` method is the workspace repo, not the checkout.
            line.match_indices(call).any(|(at, _)| {
                !line[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
            })
        })
}

/// A path whose segment is `tests`, `*_tests` or `*_test` holds test code
/// throughout.
fn is_test_path(rel: &str) -> bool {
    rel.split('/').any(|part| {
        let base = part.strip_suffix(".rs").unwrap_or(part);
        base == "tests" || base.ends_with("_tests") || base.ends_with("_test")
    })
}

/// Whether a trimmed line is a `#[cfg(..)]` whose predicate names `test`,
/// such as `#[cfg(test)]` or `#[cfg(all(test, unix))]`. A quoted value like
/// `feature = "e2e-test-hooks"` does not count.
fn is_test_cfg(trimmed: &str) -> bool {
    let Some(predicate) = trimmed.strip_prefix("#[cfg(") else {
        return false;
    };
    predicate
        .split('"')
        .step_by(2)
        .flat_map(|unquoted| unquoted.split(|c: char| !(c.is_alphanumeric() || c == '_')))
        .any(|word| word == "test")
}

/// Which lines are test code: all of a test path, else each item a test
/// `#[cfg]` covers. A declaration ends at its `;`, a block at the next `}` at
/// the attribute's indent, as rustfmt lays it out.
fn test_lines(rel: &str, lines: &[&str]) -> Vec<bool> {
    if is_test_path(rel) {
        return vec![true; lines.len()];
    }
    let mut test = vec![false; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !is_test_cfg(trimmed) {
            continue;
        }
        let close = format!("{}}}", &line[..line.len() - trimmed.len()]);
        let mut item = i + 1;
        while lines.get(item).is_some_and(|l| {
            let l = l.trim_start();
            l.starts_with("#[") || l.starts_with("///")
        }) {
            item += 1;
        }
        let Some(first) = lines.get(item) else { break };
        let end = if first.trim_end().ends_with(';') {
            item
        } else {
            (item + 1..lines.len())
                .find(|&k| lines[k] == close)
                .unwrap_or(lines.len() - 1)
        };
        test[i..=end].iter_mut().for_each(|t| *t = true);
    }
    test
}

/// The 1-based lines of `text` where test code reads the tree outside a
/// `guard_tests` module. `rel` is the path under `src/`. A module ends at its
/// own indent's `}`, as rustfmt lays it out.
fn tree_reads_outside_guard_tests(rel: &str, text: &str) -> Vec<usize> {
    if rel
        .split('/')
        .any(|part| part.trim_end_matches(".rs") == "guard_tests")
    {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let test = test_lines(rel, &lines);
    let mut guard_end: Option<String> = None;
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(end) = &guard_end {
            if *line == end {
                guard_end = None;
            }
            continue;
        }
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        if trimmed.strip_prefix("pub(crate) ").unwrap_or(trimmed) == "mod guard_tests {" {
            guard_end = Some(format!("{indent}}}"));
            continue;
        }
        if test[i] && !trimmed.starts_with("//") && reads_the_tree(line) {
            found.push(i + 1);
        }
    }
    found
}

mod tests {
    use super::*;

    fn collect_price_keys(
        value: &serde_json::Value,
        price_key: &dyn Fn(&str) -> bool,
        rel: &str,
        hits: &mut Vec<String>,
    ) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, inner) in map {
                    let numeric = inner.is_number()
                        || inner
                            .as_object()
                            .is_some_and(|m| m.values().any(serde_json::Value::is_number));
                    if price_key(key) && numeric {
                        hits.push(format!("{rel}: \"{key}\""));
                    }
                    collect_price_keys(inner, price_key, rel, hits);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_price_keys(item, price_key, rel, hits);
                }
            }
            _ => {}
        }
    }

    /// The price table defines each price once. A second copy shows as a rate literal
    /// divided by a million, a `ModelPrice` literal, or a JSON price key.
    /// `lucidos-eval` pins its own run prices on purpose (its `prices.toml`).
    #[test]
    fn no_rate_literal_lives_outside_the_price_table() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let per_million =
            regex::Regex::new(r"\d+\.\d+\s*/\s*(1e6|1_000_000(\.0)?)\b").expect("a regex");
        let price_key = |key: &str| {
            key.to_ascii_lowercase().contains("price")
                || ["uncached_in", "cache_read", "cache_write", "cache_write_1h"].contains(&key)
        };
        let mut hits = Vec::new();
        let mut stack = vec![crates.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("a readable dir").flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if path.is_dir() {
                    let skipped = ["target", "node_modules", "dist", "lucidos-eval"];
                    if !skipped.contains(&name.as_str()) && !name.starts_with('.') {
                        stack.push(path);
                    }
                    continue;
                }
                let rel = path
                    .strip_prefix(&crates)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                if rel.ends_with("llm/model_prices.json") || rel.ends_with("llm/model_prices.rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                if name.ends_with(".json") {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                        collect_price_keys(&value, &price_key, &rel, &mut hits);
                    }
                } else if [".rs", ".ts", ".tsx"].iter().any(|ext| name.ends_with(ext)) {
                    for (n, line) in text.lines().enumerate() {
                        let price_literal = line.contains("ModelPrice {") && !line.contains("->");
                        if per_million.is_match(line) || price_literal {
                            hits.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                        }
                    }
                }
            }
        }
        assert!(
            hits.is_empty(),
            "rates outside the price table:\n{}",
            hits.join("\n")
        );
    }

    /// The one-definition guard (ADR 0368): no Rust source outside this
    /// file spells a preference key. A reader names a key only through its
    /// handle, so its default can only come from here.
    ///
    /// A key with a `_` or `-` is flagged anywhere it appears as a whole string
    /// literal. A plain-word key such as `language` is also an ordinary word
    /// and a wire field. It is flagged only on a line that names
    /// `PreferenceStore` or a `key`. No allowlist: a hit is fixed at its
    /// source.
    #[test]
    fn no_preference_key_is_spelled_outside_the_catalog() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let catalog = crates.join("lucidos-core/src/core/preference_catalog.rs");
        let literal = regex::Regex::new(r#""([a-z][a-z0-9_\-]*)""#).unwrap();
        let context = regex::Regex::new(r"PreferenceStore|\bkey\b").unwrap();
        let mut scanned = 0;
        let mut offenders = Vec::new();
        let mut stack = vec![crates];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name();
                if path.is_dir() {
                    if !matches!(name.to_str(), Some("target" | "node_modules" | "dist")) {
                        stack.push(path);
                    }
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") || path == catalog {
                    continue;
                }
                scanned += 1;
                let source = std::fs::read_to_string(&path).unwrap();
                for (n, line) in source.lines().enumerate() {
                    for capture in literal.captures_iter(line) {
                        let word = &capture[1];
                        let Some(spec) = crate::core::preference_catalog::spec(word) else {
                            continue;
                        };
                        let distinctive = spec.key.contains(['_', '-']);
                        if distinctive || context.is_match(line) {
                            offenders.push(format!("{}:{}: {word}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        assert!(scanned > 500, "the walk found only {scanned} Rust files");
        assert!(
            offenders.is_empty(),
            "a preference key is spelled outside the catalog; read it through its \
         `prefs::` handle instead:\n{}",
            offenders.join("\n")
        );
    }

    #[test]
    fn every_test_that_reads_the_source_tree_is_a_guard_test() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut found = Vec::new();
        for crate_name in ["lucidos-core", "lucidos-engine", "lucidos-agents-api"] {
            let src = crates.join(crate_name).join("src");
            let mut stack = vec![src.clone()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                        continue;
                    }
                    if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                        continue;
                    }
                    let rel = path
                        .strip_prefix(&src)
                        .expect("under src")
                        .to_string_lossy()
                        .replace('\\', "/");
                    let text = std::fs::read_to_string(&path).expect("read source");
                    for line in tree_reads_outside_guard_tests(&rel, &text) {
                        found.push(format!("{crate_name}/src/{rel}:{line}"));
                    }
                }
            }
        }
        found.sort();
        assert!(
            found.is_empty(),
            "these tests read the source tree but sit outside a `mod guard_tests`, \
             so no /harden round runs them. Move each into one:\n  {}",
            found.join("\n  ")
        );
    }

    #[test]
    fn a_test_reading_the_tree_outside_guard_tests_is_caught() {
        let reader = "    let dir = Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"src\");";
        let inline = format!(
            "fn production() {{}}\n\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn scans() {{\n{reader}\n    }}\n}}\n"
        );
        assert_eq!(tree_reads_outside_guard_tests("engine/x.rs", &inline), [7]);

        let guarded = format!(
            "#[cfg(test)]\nmod tests {{\n    mod guard_tests {{\n        fn scans() {{\n{reader}\n        }}\n    }}\n\n    fn after() {{\n{reader}\n    }}\n}}\n"
        );
        assert_eq!(
            tree_reads_outside_guard_tests("engine/x.rs", &guarded),
            [10],
            "inside guard_tests is fine, after it is not"
        );

        let test_file = format!("fn scans() {{\n{reader}\n}}\n");
        assert_eq!(
            tree_reads_outside_guard_tests("engine/x_tests.rs", &test_file),
            [2]
        );
        assert!(tree_reads_outside_guard_tests("engine/guard_tests.rs", &test_file).is_empty());

        let declared = format!(
            "#[cfg(test)]\nmod tests;\n\npub fn root() {{\n{reader}\n    canonical_repo_root();\n}}\n"
        );
        assert!(
            tree_reads_outside_guard_tests("engine/x.rs", &declared).is_empty(),
            "a `#[cfg(test)] mod tests;` declaration makes nothing after it test code"
        );
        let helper = "#[cfg(test)]\nmod tests {\n    fn a() {\n        crate::paths::repo_root();\n        canonical_repo_root();\n    }\n}\n";
        assert_eq!(
            tree_reads_outside_guard_tests("engine/x.rs", helper),
            [4],
            "repo_root( counts as a whole name only"
        );
        let method =
            "#[cfg(test)]\nmod tests {\n    fn a() {\n        engine.repo_root();\n    }\n}\n";
        assert!(
            tree_reads_outside_guard_tests("engine/x.rs", method).is_empty(),
            "a .repo_root() method is not the checkout"
        );

        let combined = format!(
            "#[cfg(all(test, unix))]\nmod tests {{\n    fn scans() {{\n{reader}\n    }}\n}}\n"
        );
        assert_eq!(
            tree_reads_outside_guard_tests("engine/x.rs", &combined),
            [4]
        );
        let nested = format!(
            "mod inner {{\n    #[cfg(test)]\n    mod tests {{\n        fn scans() {{\n{reader}\n        }}\n    }}\n}}\n"
        );
        assert_eq!(tree_reads_outside_guard_tests("engine/x.rs", &nested), [5]);
        let feature = format!("#[cfg(feature = \"e2e-test-hooks\")]\nfn hook() {{\n{reader}\n}}\n");
        assert!(
            tree_reads_outside_guard_tests("engine/x.rs", &feature).is_empty(),
            "a quoted feature name holding `test` is not a test cfg"
        );

        let production = format!("pub fn root() {{\n{reader}\n}}\n");
        assert!(
            tree_reads_outside_guard_tests("paths.rs", &production).is_empty(),
            "production code reading the tree is not a test"
        );
    }
}
