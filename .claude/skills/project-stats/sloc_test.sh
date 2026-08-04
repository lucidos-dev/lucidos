#!/usr/bin/env bash
#
# sloc_test.sh - fixture tests for sloc.awk.
#
#   ./.claude/skills/project-stats/sloc_test.sh
#
# Every fixture below is a shape that actually occurs in this tree. The
# string-literal ones are regressions: a scanner without string awareness read
# the `/*` inside engine/command_guard.rs's `"/" | "/*"` match arm as a
# block-comment open, found no close in the rest of the file, and reported that
# file as 61% comment when it is 25%.
#
# Exit status: 0 when every case passes, non-zero otherwise.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SLOC="$SCRIPT_DIR/sloc.awk"

if [ ! -f "$SLOC" ]; then
    echo "ERROR: $SLOC is missing." >&2
    exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

FAILURES=0
CASES=0

# Count one fixture and compare the summary line against an expected one.
#
# Stderr must be EMPTY, not merely ignored. A regression that dropped .rs out of
# set_style would fall through to the unknown-extension branch, which happens to
# pick the same syntax for Rust, so the counts would still match while a warning
# went unread. Asserting silence is what catches that.
expect_count() {
    local label="$1" file="$2" want="$3" got err
    CASES=$((CASES + 1))
    got="$(printf '%s\n' "$file" | awk -f "$SLOC" 2>"$TMP/stderr")"
    err="$(<"$TMP/stderr")"
    if [ "$got" = "$want" ] && [ -z "$err" ]; then
        printf '  ok    %s\n' "$label"
    else
        printf '  FAIL  %s\n          want: %s\n          got:  %s\n' "$label" "$want" "$got"
        [ -n "$err" ] && printf '          unexpected stderr: %s\n' "$err"
        FAILURES=$((FAILURES + 1))
    fi
}

# Assert the scanner fails loud: non-zero exit plus a stderr message.
expect_warns() {
    local label="$1" file="$2" needle="$3" rc err
    CASES=$((CASES + 1))
    err="$(printf '%s\n' "$file" | awk -f "$SLOC" 2>&1 >/dev/null)"
    rc=$?
    if [ "$rc" -ne 0 ] && [ "${err#*"$needle"}" != "$err" ]; then
        printf '  ok    %s\n' "$label"
    else
        printf '  FAIL  %s\n          want: non-zero exit and stderr matching %s\n          got:  rc=%s stderr=%s\n' \
            "$label" "$needle" "$rc" "$err"
        FAILURES=$((FAILURES + 1))
    fi
}

cat > "$TMP/mixed.rs" <<'EOF'
// line comment
/// doc comment
fn main() {
    let x = 1; // a trailing comment leaves the line code
    /* block open
       block close */
    let url = "https://example.com";
    match p {
        "/" | "/*" => deny(),
    }
    let s: &'a str = "x";
    /* one liner */ let y = 2;

}
EOF
expect_count "rust: comments, strings holding comment tokens, lifetimes" \
    "$TMP/mixed.rs" "code=9 comment=4 blank=1 total=14 files=1"

cat > "$TMP/case.sh" <<'EOF'
#!/usr/bin/env bash
# comment
echo "#not a comment"
echo 'also #not'
EOF
expect_count "shell: shebang is code, # inside both quote styles is code" \
    "$TMP/case.sh" "code=3 comment=1 blank=0 total=4 files=1"

# Only on line 1, and only where # is the comment token. A #!-looking line
# further down is an ordinary comment.
printf '# header\n#!/usr/bin/env bash\necho hi\n' > "$TMP/late_shebang.sh"
expect_count "shell: #! below line 1 is a comment" \
    "$TMP/late_shebang.sh" "code=1 comment=2 blank=0 total=3 files=1"

# Heredoc payload is not shell comment territory. The <<< and `1 << 20` cases
# are the false-positive shapes: both occur in this repo's own scripts, and a
# tracker that read either as a heredoc open would swallow the rest of the file.
cat > "$TMP/heredoc.sh" <<'OUTER'
# a real comment
cat <<'PY'
# this is python, not a shell comment
print("hi")
PY
read -r a b <<< "$pair"
mask=$(( 1 << 20 ))
# mentions <<EOF but opens nothing
echo done
OUTER
expect_count "shell: heredoc payload is code, <<< and shifts are not opens" \
    "$TMP/heredoc.sh" "code=7 comment=2 blank=0 total=9 files=1"

printf 'cat <<\tEOF\n' > "$TMP/dash.sh"
cat >> "$TMP/dash.sh" <<'OUTER'
# payload
OUTER
printf 'EOF\necho after\n' >> "$TMP/dash.sh"
expect_count "shell: whitespace between << and the delimiter word" \
    "$TMP/dash.sh" "code=4 comment=0 blank=0 total=4 files=1"

printf 'cat <<UNCLOSED\n# body\nmore body\n' > "$TMP/openhd.sh"
expect_warns "unterminated heredoc fails loud" \
    "$TMP/openhd.sh" "unterminated heredoc"

# A << inside a quoted string is text. This file's own line 116 is the reason:
# the label "whitespace between << and the delimiter word" opened a phantom
# heredoc named `and` and ate the rest of the file until the canary caught it.
cat > "$TMP/quoted_shift.sh" <<'OUTER'
echo "compare << and >> here"
grep -n 'a << b' file
# comment
echo tail
OUTER
expect_count "shell: << inside a quoted string opens nothing" \
    "$TMP/quoted_shift.sh" "code=3 comment=1 blank=0 total=4 files=1"

# Block comments close at the FIRST */ in every language here. For TS that is
# the language rule. For Rust it is a deliberate deviation: Rust nests, but
# depth-counting only ever fired on a false /* from a glob inside a string, and
# needing two */ to close let that misparse run past the end of its line. See
# sloc.awk § KNOWN LIMITS. Pinning it so the revert is not undone by accident.
cat > "$TMP/nested.rs" <<'OUTER'
/* outer /* inner */ still outer */
fn a() {}
OUTER
expect_count "rust: block comments close at the first */, deliberately" \
    "$TMP/nested.rs" "code=2 comment=0 blank=0 total=2 files=1"

# `{/* … */}` is how JSX carries a comment; the braces only host it. The last
# three lines are the shapes that must STAY code, so the exemption cannot widen
# into "a brace next to a comment is free".
cat > "$TMP/jsx.tsx" <<'OUTER'
{/* single line wrapper */}
{/* wrapper opening
    a multi line comment */}
<div>{/* inline after markup */}</div>
const a = {/* not a wrapper */};
} /* closing brace with a note */
OUTER
expect_count "tsx: JSX comment wrappers are comments, real braces are not" \
    "$TMP/jsx.tsx" "code=3 comment=3 blank=0 total=6 files=1"

# The same text in a .ts file has no JSX, so the braces are ordinary code.
cat > "$TMP/plain.ts" <<'OUTER'
{/* single line wrapper */}
OUTER
expect_count "ts: no JSX exemption, the braces are code" \
    "$TMP/plain.ts" "code=1 comment=0 blank=0 total=1 files=1"

cat > "$TMP/nested.ts" <<'OUTER'
/* outer /* inner */
const a = 1;
OUTER
expect_count "ts: block comments do not nest" \
    "$TMP/nested.ts" "code=1 comment=1 blank=0 total=2 files=1"

# A plain << terminates only on the delimiter alone at column 0. An indented
# look-alike in the payload is body: closing on it early would hand the rest
# back to the shell classifier and turn payload # lines into comments again.
{ printf 'cat <<EOF\n'; printf '  EOF\n'; printf '# still payload\n'
  printf 'EOF\n'; printf '# a real comment\n'; } > "$TMP/indented_delim.sh"
expect_count "shell: an indented look-alike does not terminate a plain <<" \
    "$TMP/indented_delim.sh" "code=4 comment=1 blank=0 total=5 files=1"

# <<- is the form that indents, and it strips tabs only.
{ printf 'cat <<-EOF\n'; printf '\t# payload\n'; printf '\tEOF\n'; printf 'echo after\n'; } > "$TMP/dash_delim.sh"
expect_count "shell: <<- terminates on a tab-indented delimiter" \
    "$TMP/dash_delim.sh" "code=4 comment=0 blank=0 total=4 files=1"

printf 'cat <<A <<B\nx\nA\ny\nB\n' > "$TMP/two_heredocs.sh"
expect_warns "two heredocs on one line fails loud rather than silently" \
    "$TMP/two_heredocs.sh" "heredocs opened on one line"

cat > "$TMP/hyphen.sh" <<'OUTER'
cat <<'END-JSON'
# payload
END-JSON
echo after
OUTER
expect_count "shell: a quoted delimiter may hold non-identifier characters" \
    "$TMP/hyphen.sh" "code=4 comment=0 blank=0 total=4 files=1"

cat > "$TMP/case.css" <<'EOF'
/* header */
.a { color: red; }
.b { content: "/* not a comment */"; }

EOF
expect_count "css: block comments only, quoted block token is code" \
    "$TMP/case.css" "code=2 comment=1 blank=1 total=4 files=1"

cat > "$TMP/case.sql" <<'EOF'
-- comment
SELECT 1;
SELECT '-- not a comment';
EOF
expect_count "sql: -- line comments, quoted token is code" \
    "$TMP/case.sql" "code=2 comment=1 blank=0 total=3 files=1"

cat > "$TMP/case.ts" <<'EOF'
// c
const a = `tpl // not a comment`;
const g = 'src/**/*.rs';
export {};
EOF
expect_count "ts: template literals and single-quoted globs are code" \
    "$TMP/case.ts" "code=3 comment=1 blank=0 total=4 files=1"

cat > "$TMP/escaped.rs" <<'EOF'
let a = "he said \" // still string";
let b = 1;
EOF
expect_count "rust: escaped quote does not close the string early" \
    "$TMP/escaped.rs" "code=2 comment=0 blank=0 total=2 files=1"

cat > "$TMP/unterminated.rs" <<'EOF'
fn a() {}
/* opened and never closed
still swallowed
EOF
expect_warns "unterminated block comment fails loud" \
    "$TMP/unterminated.rs" "unterminated block comment"

cat > "$TMP/case.py" <<'EOF'
print("hi")
EOF
expect_warns "unknown extension fails loud" \
    "$TMP/case.py" "unknown extension"

# per_file mode: one line per file, in input order, with the path last.
CASES=$((CASES + 1))
PER_FILE="$(printf '%s\n%s\n' "$TMP/case.sql" "$TMP/case.css" | awk -v per_file=1 -f "$SLOC" 2>/dev/null | awk '{ print $1, $2, $3, $4 }')"
if [ "$PER_FILE" = "2 1 0 3
2 1 1 4" ]; then
    printf '  ok    per_file emits one row per file\n'
else
    printf '  FAIL  per_file emits one row per file\n          got: %s\n' "$PER_FILE"
    FAILURES=$((FAILURES + 1))
fi

echo
if [ "$FAILURES" -eq 0 ]; then
    printf 'sloc_test: %d cases, all passed\n' "$CASES"
    exit 0
fi
printf 'sloc_test: %d of %d cases FAILED\n' "$FAILURES" "$CASES" >&2
exit 1
