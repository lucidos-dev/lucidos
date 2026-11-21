//! Tests for the shared unified-diff parser. Moved here from
//! `repositories_tests.rs` when the parser was lifted out of
//! `api::repositories` to be shared with the command-checkpoint diff; the
//! assertions are unchanged.

use super::*;

#[test]
fn parse_modified_file_diff() {
    let input = concat!(
        "diff --git a/src/main.rs b/src/main.rs\n",
        "--- a/src/main.rs\n",
        "+++ b/src/main.rs\n",
        "@@ -1,3 +1,4 @@\n",
        " fn main() {\n",
        "-    println!(\"old\");\n",
        "+    println!(\"new\");\n",
        "+    println!(\"extra\");\n",
        " }\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/main.rs");
    assert_eq!(files[0].status, "modified");
    assert_eq!(files[0].hunks.len(), 1);
    assert_eq!(files[0].hunks[0].old_start, 1);
    assert_eq!(files[0].hunks[0].old_count, 3);
    assert_eq!(files[0].hunks[0].new_start, 1);
    assert_eq!(files[0].hunks[0].new_count, 4);
    // 2 context + 1 deletion + 2 additions = 5
    assert_eq!(files[0].hunks[0].lines.len(), 5);
}

#[test]
fn parse_added_file_diff() {
    let input = concat!(
        "diff --git a/new.rs b/new.rs\n",
        "new file mode 100644\n",
        "--- /dev/null\n",
        "+++ b/new.rs\n",
        "@@ -0,0 +1,2 @@\n",
        "+fn new_fn() {\n",
        "+}\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, "added");
    assert_eq!(files[0].path, "new.rs");
}

#[test]
fn parse_deleted_file_diff() {
    let input = concat!(
        "diff --git a/old.rs b/old.rs\n",
        "deleted file mode 100644\n",
        "--- a/old.rs\n",
        "+++ /dev/null\n",
        "@@ -1,2 +0,0 @@\n",
        "-fn old_fn() {\n",
        "-}\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, "deleted");
    assert_eq!(files[0].path, "old.rs");
}

#[test]
fn parse_multiple_files() {
    let input = concat!(
        "diff --git a/a.rs b/a.rs\n",
        "--- a/a.rs\n",
        "+++ b/a.rs\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "+new\n",
        "diff --git a/b.rs b/b.rs\n",
        "--- a/b.rs\n",
        "+++ b/b.rs\n",
        "@@ -1 +1 @@\n",
        "-x\n",
        "+y\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "a.rs");
    assert_eq!(files[1].path, "b.rs");
}

#[test]
fn parse_empty_diff() {
    let files = parse_diff_output("");
    assert!(files.is_empty());
}

#[test]
fn parse_diff_output_filters_engine_injected_paths() {
    let input = concat!(
        "diff --git a/.lucidos-workspace b/.lucidos-workspace\n",
        "new file mode 100644\n",
        "--- /dev/null\n",
        "+++ b/.lucidos-workspace\n",
        "@@ -0,0 +1,2 @@\n",
        "+/Users/me/workspaces/dev\n",
        "+abc-uuid\n",
        "diff --git a/.lucidos/bin/lucidos b/.lucidos/bin/lucidos\n",
        "new file mode 120000\n",
        "--- /dev/null\n",
        "+++ b/.lucidos/bin/lucidos\n",
        "@@ -0,0 +1 @@\n",
        "+/usr/local/bin/lucidos\n",
        "diff --git a/.claude/skills/lucidos-cli/SKILL.md b/.claude/skills/lucidos-cli/SKILL.md\n",
        "new file mode 100644\n",
        "--- /dev/null\n",
        "+++ b/.claude/skills/lucidos-cli/SKILL.md\n",
        "@@ -0,0 +1,2 @@\n",
        "+# lucidos-cli skill\n",
        "+content\n",
        "diff --git a/src/real.rs b/src/real.rs\n",
        "--- a/src/real.rs\n",
        "+++ b/src/real.rs\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "+new\n",
    );
    let files = parse_diff_output(input);
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["src/real.rs"],
        "engine-injected paths must be filtered from parse_diff_output, got: {:?}",
        paths
    );
}

#[test]
fn parse_binary_file_skipped() {
    // Binary files have no hunks, so the parser should produce a file with
    // empty hunks.
    let input = concat!(
        "diff --git a/image.png b/image.png\n",
        "new file mode 100644\n",
        "Binary files /dev/null and b/image.png differ\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert!(files[0].hunks.is_empty());
}

#[test]
fn parse_rename_diff() {
    let input = concat!(
        "diff --git a/old_name.rs b/new_name.rs\n",
        "similarity index 100%\n",
        "rename from old_name.rs\n",
        "rename to new_name.rs\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    // Path extracted from "diff --git a/old b/new" header
    assert_eq!(files[0].path, "new_name.rs");
    assert!(files[0].hunks.is_empty());
}

#[test]
fn parse_multiple_hunks() {
    let input = concat!(
        "diff --git a/lib.rs b/lib.rs\n",
        "--- a/lib.rs\n",
        "+++ b/lib.rs\n",
        "@@ -1,2 +1,2 @@\n",
        "-old1\n",
        "+new1\n",
        " same\n",
        "@@ -10,3 +10,4 @@\n",
        " ctx\n",
        "-old2\n",
        "+new2\n",
        "+extra\n",
        " ctx\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].hunks.len(), 2);
    assert_eq!(files[0].hunks[0].old_start, 1);
    assert_eq!(files[0].hunks[0].lines.len(), 3); // 1 del + 1 add + 1 ctx
    assert_eq!(files[0].hunks[1].old_start, 10);
    // 1 ctx + 1 del + 2 add + 1 ctx = 5
    assert_eq!(files[0].hunks[1].lines.len(), 5);
}

#[test]
fn parse_no_newline_at_eof() {
    // git diff outputs "\ No newline at end of file", which the parser skips.
    let input = concat!(
        "diff --git a/f.rs b/f.rs\n",
        "--- a/f.rs\n",
        "+++ b/f.rs\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "\\ No newline at end of file\n",
        "+new\n",
        "\\ No newline at end of file\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    // Only the -old and +new lines, not the backslash lines
    assert_eq!(files[0].hunks[0].lines.len(), 2);
}

#[test]
fn parse_hunk_header_no_count() {
    // Single-line hunk: @@ -5 +5 @@ (no comma count)
    let hunk = parse_hunk_header("@@ -5 +5 @@").unwrap();
    assert_eq!(hunk.old_start, 5);
    assert_eq!(hunk.old_count, 1);
    assert_eq!(hunk.new_start, 5);
    assert_eq!(hunk.new_count, 1);
}

#[test]
fn parse_hunk_header_with_function_context() {
    // Real git often appends function name: @@ -10,3 +10,4 @@ fn main()
    let hunk = parse_hunk_header("@@ -10,3 +10,4 @@ fn main()").unwrap();
    assert_eq!(hunk.old_start, 10);
    assert_eq!(hunk.old_count, 3);
    assert_eq!(hunk.new_start, 10);
    assert_eq!(hunk.new_count, 4);
}

#[test]
fn parse_hunk_header_malformed() {
    // Too few tokens: need at least @@ -x +y @@
    assert!(parse_hunk_header("@@").is_none());
    assert!(parse_hunk_header("@@ -1").is_none());
}

#[test]
fn parse_range_basic() {
    assert_eq!(parse_range("5,3"), (5, 3));
    assert_eq!(parse_range("1"), (1, 1));
    assert_eq!(parse_range("0,0"), (0, 0));
}

// Defense-in-depth: even after we set `-c core.quotepath=false` on the
// git invocations, the parser must never let the raw `diff --git` header
// line bleed into the user-facing filename if a quoted path ever slips
// through (e.g. paths with literal quotes, control bytes, or a future
// git version that quotes for some other reason).
#[test]
fn parse_added_file_with_quoted_unicode_path() {
    let input = concat!(
        r#"diff --git "a/dir/7_\360\237\247\256_HLL.py" "b/dir/7_\360\237\247\256_HLL.py""#,
        "\n",
        "new file mode 100644\n",
        "--- /dev/null\n",
        r#"+++ "b/dir/7_\360\237\247\256_HLL.py""#,
        "\n",
        "@@ -0,0 +1 @@\n",
        "+content\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "dir/7_🧮_HLL.py");
    assert_eq!(files[0].status, "added");
    assert!(
        !files[0].path.contains("diff --git"),
        "raw diff header must never become the displayed filename, got: {:?}",
        files[0].path
    );
}

#[test]
fn parse_diff_git_path_handles_quoted_form() {
    let line = r#"diff --git "a/dir/7_\360\237\247\256_HLL.py" "b/dir/7_\360\237\247\256_HLL.py""#;
    assert_eq!(parse_diff_git_path(line), "dir/7_🧮_HLL.py");
}

#[test]
fn parse_diff_git_path_handles_unquoted_form() {
    assert_eq!(
        parse_diff_git_path("diff --git a/src/main.rs b/src/main.rs"),
        "src/main.rs"
    );
}

#[test]
fn parse_diff_git_path_handles_rename_unquoted() {
    // Renames have different a/ and b/ paths, and we want the b/ side.
    assert_eq!(
        parse_diff_git_path("diff --git a/old_name.rs b/new_name.rs"),
        "new_name.rs"
    );
}

#[test]
fn parse_diff_git_path_returns_empty_when_unparseable() {
    // Garbage in, empty out, never the cryptic raw line.
    assert_eq!(parse_diff_git_path("diff --git malformed"), "");
    assert_eq!(parse_diff_git_path(""), "");
}

#[test]
fn unquote_git_path_decodes_octal_to_utf8() {
    assert_eq!(
        unquote_git_path(r#""7_\360\237\247\256_x.py""#),
        "7_🧮_x.py"
    );
}

#[test]
fn unquote_git_path_decodes_standard_escapes() {
    assert_eq!(unquote_git_path(r#""a\tb\nc\\d\"e""#), "a\tb\nc\\d\"e");
}

#[test]
fn unquote_git_path_passthrough_when_unquoted() {
    assert_eq!(unquote_git_path("plain/path.txt"), "plain/path.txt");
    assert_eq!(unquote_git_path(""), "");
    assert_eq!(unquote_git_path("\""), "\"");
}
