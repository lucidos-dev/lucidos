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

    #[test]
    fn every_test_that_reads_the_source_tree_is_a_guard_test() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src.clone()];
        let mut found = Vec::new();
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
                    found.push(format!("src/{rel}:{line}"));
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
