# sloc.awk - conventional source-line counting, cloc's definition.
#
# Reads a newline-separated list of file paths on STDIN, not argv, and prints
# ONE summary line. Taking the list on stdin is not a style choice: the
# `… | xargs wc -l | tail -1` idiom this replaced breaks once the list exceeds
# ARG_MAX, because xargs runs `wc` more than once and `tail -1` keeps only the
# LAST batch's total. On VS Code's src/ that reported 973,759 lines against a
# true 2,474,634, with nothing to signal the loss.
#
#     code=<n> comment=<n> blank=<n> total=<n> files=<n>
#
#     command find crates/lucidos-engine/src -name '*.rs' | awk -f sloc.awk
#
# With `-v per_file=1` it prints `code comment blank total path` per file
# instead, for ranking tables (which files are most comment-heavy).
#
# WHY THIS EXISTS: `wc -l` counts comments and blank lines as code. Every
# published line-count benchmark (ripgrep ~50K, VS Code ~600K) is a cloc-style
# count, so comparing a `wc -l` total against one overstates by however much
# prose the tree carries. In this repo that is about a third.
#
# THE DEFINITION, shared with cloc, tokei and scc: a line is code OR comment OR
# blank, never two at once, and a line carrying both code and a comment counts
# as code.
#
# STRING LITERALS ARE SKIPPED, and that is load-bearing rather than a nicety.
# A scanner that only looks for comment tokens reads the `/*` inside
# `"/" | "/*"` (engine/command_guard.rs) as a block-comment open, finds no `*/`
# in the rest of the file, and books 944 lines of code as comment: that one file
# went from 5% comment to 61%. So the scan walks each line left to right and
# jumps over quoted runs, honouring backslash escapes. Which quote characters
# are live is per language, because Rust's `'a` lifetime is not a string open
# while shell's `'...'` is.
#
# PYTHON DOCSTRINGS ARE COMMENTS, which is cloc's rule and the reason a `.py`
# tree cannot be counted with the generic scanner. It is also the single
# exception to the per-line string reset below: a triple-quoted run has to be
# matched to its close across lines or the prose inside it books as code. The
# EOF canary bounds the damage when that goes wrong.
#
# Only in DOCSTRING POSITION, though. A run opening after code is data, and its
# lines are code: SQL, templates and prompts live in `x = """…"""`, and one
# measured repo held 9,790 such lines against 8,113 real docstring lines.
#
# SHELL HEREDOCS ARE TRACKED for the same reason: inside a heredoc body a
# leading `#` is payload the script emits, or a comment belonging to an embedded
# language, and neither is a comment in the .sh file. See detect_heredoc for the
# three shapes that must NOT open one.
#
# KNOWN LIMITS, both chosen deliberately:
#   - String state does NOT carry across lines, so a line INSIDE a multi-line
#     string that starts with a comment token is booked as a comment. Measured
#     2026-08-03: 57 lines of Rust and 12 of TypeScript tree-wide, 0.02% of
#     code, and 49 of the 57 are `//` comments in JavaScript embedded in a
#     raw string, which are comments in any case. Carrying the state would
#     mean matching `r#"` to its `"#` and tracking unterminated `"` across
#     lines, and getting THAT wrong swallows a whole file as code, which is a
#     far larger error than the one it fixes. Resetting per line keeps every
#     misparse confined to its own line. The dangerous direction, a `/*` inside
#     a multi-line string eating later source, is caught by the unterminated
#     block canary below, which is silent across this whole tree.
#   - Rust block comments NEST in the language, and this closes at the first
#     `*/` anyway, which follows from the limit above rather than contradicting
#     it. Depth-counting was implemented and reverted on 2026-08-03 after
#     measuring it: the tree contains no nested comment, the only thing that
#     triggered the depth counter was a false `/*` from the glob `'**/*.md'`
#     inside the system-prompt raw string, and requiring two `*/` to close let
#     that misparse span LINES instead of staying on one. It moved 5 lines of
#     prompt text out of code and into comment, all of them wrong. A false `/*`
#     inside a string is common (globs, regexes, URLs) while a nested comment is
#     rare, so nesting amplifies the frequent error to fix the rare one.
#   - A Rust char literal holding a quote (`'"'`) is not modelled, since single
#     quotes are off for Rust. An unterminated block comment at EOF is the
#     symptom of any such misparse, so it warns and exits non-zero rather than
#     quietly inflating the comment count the way the bug above did.

function trim(x) {
    gsub(/^[ \t\r]+|[ \t\r]+$/, "", x)
    return x
}

# Number of consecutive backslashes immediately before position p in str.
# An odd run means the character at p is escaped.
function esc_run(str, p,   n) {
    n = 0
    while (p - 1 - n >= 1 && substr(str, p - 1 - n, 1) == "\\") n++
    return n
}

# Comment and string syntax for one path.
#   LC  line-comment token ("" when the language has none)
#   BLK 1 when /* */ block comments apply
#   QD / QS / QB  double quote / single quote / backtick open a string
#   HD  1 when << heredocs apply (shell only)
#   JSX 1 when `{/* ... */}` comment wrappers apply (tsx/jsx only)
#   PYDOC 1 when a triple-quoted run is a docstring (python only)
function set_style(path,   ext) {
    ext = path
    if (ext !~ /\./) { ext = "" } else { sub(/.*\./, "", ext) }

    HD = 0; JSX = 0; PYDOC = 0
    if (ext ~ /^(rs)$/) {
        # Single quotes are OFF: `&'a str` is a lifetime, not a string open.
        LC = "//"; BLK = 1; QD = 1; QS = 0; QB = 0
    } else if (ext ~ /^(ts|tsx|js|jsx|mjs|cjs)$/) {
        LC = "//"; BLK = 1; QD = 1; QS = 1; QB = 1
        if (ext ~ /^(tsx|jsx)$/) JSX = 1
    } else if (ext ~ /^(kt|kts)$/) {
        # Kotlin, listed rather than left to the fallback below. The fallback
        # picks the same syntax, but warns per file, and a JVM repo then buries
        # the real canaries under hundreds of lines of stderr.
        LC = "//"; BLK = 1; QD = 1; QS = 0; QB = 0
    } else if (ext == "py") {
        LC = "#";  BLK = 0; QD = 1; QS = 1; QB = 0; PYDOC = 1
    } else if (ext == "css") {
        LC = "";   BLK = 1; QD = 1; QS = 1; QB = 0
    } else if (ext == "scss") {
        LC = "//"; BLK = 1; QD = 1; QS = 1; QB = 0
    } else if (ext == "sql") {
        LC = "--"; BLK = 1; QD = 1; QS = 1; QB = 0
    } else if (ext ~ /^(sh|bash|zsh)$/) {
        LC = "#";  BLK = 0; QD = 1; QS = 1; QB = 0; HD = 1
    } else if (ext ~ /^(toml|yml|yaml)$/) {
        LC = "#";  BLK = 0; QD = 1; QS = 1; QB = 0
    } else {
        LC = "//"; BLK = 1; QD = 1; QS = 0; QB = 0
        printf("sloc.awk: unknown extension for %s, counted with // and /* */\n",
               path) > "/dev/stderr"
        rc = 1
    }
}

# Does this line open a heredoc? Sets hd_delim / in_hd if so.
#
# Scanning is manual rather than one regex because all three hazards are
# positional, and each one really occurs:
#
#   `<<<` is a here-string, not a heredoc (`done <<< "$active"`, workspace.sh).
#   Stepping past all three `<` is what separates them.
#
#   `$(( 1 << 20 ))` is a shift (release_staging.sh). Requiring a delimiter WORD
#   right after the `<<` excludes it.
#
#   A `<<` inside a quoted string is text, not a redirect. Missing this is not
#   theoretical: sloc_test.sh has the label "whitespace between << and the
#   delimiter word", which opened a phantom heredoc named `and` and swallowed
#   the rest of that file. So quoted runs are skipped. The delimiter's OWN quote
#   in `<<'EOF'` sits AFTER the `<<`, so it is read as a delimiter, never
#   skipped as a string, which is why the scan has to be positional this way.
#
# Only the part of the line before any comment is scanned, so a `# see <<EOF`
# note in prose cannot open one either.
function detect_heredoc(s,   head, i, n, ch, rest, d, dq, p, dash, found) {
    head = (cmt_pos > 0) ? substr(s, 1, cmt_pos - 1) : s

    # The walk below is per character, and 139 of this tree's 40,268 shell lines
    # contain `<<` at all. One index() keeps the other 99.65% out of it.
    if (index(head, "<<") == 0) return

    n = length(head)
    i = 1
    found = 0
    while (i <= n) {
        ch = substr(head, i, 1)
        if ((QD && ch == "\"") || (QS && ch == "'")) {
            rest = skip_string(substr(head, i + 1), ch)
            i = n - length(rest) + 1
            continue
        }
        if (substr(head, i, 2) == "<<") {
            if (substr(head, i + 2, 1) == "<") { i += 3; continue }
            rest = substr(head, i + 2)
            dash = sub(/^-/, "", rest)
            sub(/^[ \t]+/, "", rest)
            sub(/^\\/, "", rest)

            d = ""
            dq = substr(rest, 1, 1)
            if (dq == "'" || dq == "\"") {
                # A QUOTED delimiter is any word at all, so read to the closing
                # quote rather than matching an identifier. Stopping at the
                # identifier prefix turns `<<'END-JSON'` into a delimiter `END`
                # that the real `END-JSON` terminator never matches, and the
                # rest of the file counts as heredoc body.
                p = index(substr(rest, 2), dq)
                if (p > 1) d = substr(rest, 2, p - 1)
            } else if (match(rest, /^[A-Za-z_][A-Za-z0-9_.-]*/)) {
                # UNQUOTED it must still START with a letter or underscore. That
                # is the guard keeping `$(( 1 << 20 ))` from opening a heredoc
                # named `20`.
                d = substr(rest, RSTART, RLENGTH)
            }

            if (d != "") {
                found++
                if (found == 1) { hd_delim = d; hd_dash = dash; in_hd = 1 }
            }
            i += 2
            continue
        }
        i++
    }

    # `cat <<A <<B` declares two, whose bodies follow in order. Only the first
    # is tracked, so B's body would be handed back to the shell classifier and
    # its `#` lines counted as comments. There is no such line in this tree
    # (checked across all 112 tracked scripts) and a delimiter queue for a
    # construct that appears zero times is not worth the state, but unlike the
    # other heredoc hazards this one leaves NO trace, so it gets a canary
    # instead of silence.
    if (found > 1) {
        printf("sloc.awk: %d heredocs opened on one line in %s, only %s is tracked\n",
               found, path, hd_delim) > "/dev/stderr"
        rc = 1
    }
}

# Does this line terminate the open heredoc?
#
# The rule is exact, not "trimmed equals the delimiter". For a plain `<<` the
# terminator must be the delimiter alone at column 0, so an indented `  EOF` in
# the payload is body, not the end. Accepting it would close the heredoc early
# and hand the remaining payload back to the shell classifier, where every `#`
# line becomes a comment again. `<<-` is the one form that indents, and it
# strips leading TABS only, never spaces.
function hd_closes(l,   t) {
    t = l
    sub(/\r$/, "", t)
    if (hd_dash) sub(/^\t+/, "", t)
    return (t == hd_delim)
}

# Skip a quoted run opened by quote character q at the head of s, returning
# what follows the closing quote. An unterminated run swallows the rest of the
# line, which is correct: it is all string content.
function skip_string(s, q,   start, p, abs) {
    start = 1
    while (1) {
        p = index(substr(s, start), q)
        if (p == 0) return ""
        abs = start + p - 1
        if (esc_run(s, abs) % 2 == 1) { start = abs + 1; continue }
        return substr(s, abs + 1)
    }
}

# Position of the first UNESCAPED occurrence of d in s, 0 if there is none.
#
# A docstring showing a literal `"""` escapes it as `\"""`, and a plain index()
# reads that as the close. The prose after it then books as code, and the REAL
# terminator opens a second docstring, so one escape can run to end of file.
function find_unescaped(s, d,   start, p, abs) {
    start = 1
    while (1) {
        p = index(substr(s, start), d)
        if (p == 0) return 0
        abs = start + p - 1
        if (esc_run(s, abs) % 2 == 1) { start = abs + 1; continue }
        return abs
    }
}

# Set has_code / has_comment for one line, carrying in_block across lines.
# Also sets cmt_pos: where the line comment began in the ORIGINAL line, 0 if
# none. `origlen - length(s)` is how much has been consumed so far, which is
# what turns a position inside the shrinking `s` back into an absolute one.
function classify(input,   s, p, best, kind, q, t3, origlen, before) {
    has_code = 0
    has_comment = 0
    cmt_pos = 0
    s = input
    origlen = length(input)

    while (length(s) > 0) {
        # A Python triple-quoted run is a docstring, which cloc books as
        # comment, so it is the one place string state has to cross lines. The
        # hazard the KNOWN LIMITS note describes applies in full: a missed close
        # swallows the file. An unterminated run at EOF therefore warns and
        # exits non-zero, exactly as the block-comment tracker does.
        if (in_pydoc != "") {
            if (pydoc_is_doc) has_comment = 1; else has_code = 1
            p = find_unescaped(s, in_pydoc)
            if (p == 0) return
            s = substr(s, p + 3)
            in_pydoc = ""
            continue
        }

        if (in_block) {
            has_comment = 1
            p = index(s, "*/")
            if (p == 0) return
            s = substr(s, p + 2)
            in_block = 0
            # `… text */}` ends a JSX comment wrapper. That brace exists only to
            # host the comment, so it must not make the line code.
            if (JSX && trim(s) == "}") s = ""
            continue
        }

        # Earliest of: line comment, block open, string open. Whichever comes
        # first decides how the rest of the line is read.
        best = 0; kind = 0
        if (LC != "") { p = index(s, LC);   if (p > 0 && (best == 0 || p < best)) { best = p; kind = 1 } }
        if (BLK)      { p = index(s, "/*"); if (p > 0 && (best == 0 || p < best)) { best = p; kind = 2 } }
        if (QD)       { p = index(s, "\""); if (p > 0 && (best == 0 || p < best)) { best = p; kind = 3 } }
        if (QS)       { p = index(s, "'");  if (p > 0 && (best == 0 || p < best)) { best = p; kind = 4 } }
        if (QB)       { p = index(s, "`");  if (p > 0 && (best == 0 || p < best)) { best = p; kind = 5 } }

        if (best == 0) {
            if (trim(s) != "") has_code = 1
            return
        }
        before = trim(substr(s, 1, best - 1))
        if (before != "") {
            # `{/* … */}` is how JSX carries a comment, and the braces are there
            # only to host it: 7 single-line wrappers and 72 multi-line ones in
            # this tree, 151 comment-only lines that would otherwise read as
            # code. Exempt the brace ONLY when it opens the line and the comment
            # follows it directly, so `<div>{/* x */}` and `const a = {/* x */}`
            # stay code.
            if (!(JSX && kind == 2 && before == "{" && (origlen - length(s)) == 0))
                has_code = 1
        }

        if (kind == 1) { has_comment = 1; cmt_pos = (origlen - length(s)) + best; return }
        if (kind == 2) { has_comment = 1; in_block = 1; s = substr(s, best + 2); continue }

        q = (kind == 3) ? "\"" : ((kind == 4) ? "'" : "`")
        t3 = q q q
        if (PYDOC && substr(s, best, 3) == t3) {
            # POSITION decides, not the quote. A triple-quoted run is a
            # docstring only where it opens the line, bar an optional string
            # prefix, which is cloc's rule too. `x = """…` is data: SQL, a
            # template, a prompt. Reading every run as a docstring moved 9,790
            # lines of one measured repo out of code and into comment, 17% of
            # its Python. A prefix belongs to the opener, so `r"""docs"""` at
            # the head of a line is still a docstring: 71 of that repo's 2,412
            # openers carry one.
            #
            # py_prev_cont carries the rest: `SQL = (` then an indented `"""`
            # opens the line but continues a statement, so it is data. 99 of
            # that repo's 2,483 line-head openers sit in that position.
            pydoc_is_doc = ((origlen - length(s)) == 0 && !py_prev_cont &&
                            (before == "" || before ~ /^[rRbBuUfF]{1,2}$/))
            if (pydoc_is_doc) { has_code = 0; has_comment = 1 } else has_code = 1
            in_pydoc = t3
            s = substr(s, best + 3)
            continue
        }
        has_code = 1
        s = skip_string(substr(s, best + 1), q)
    }
}

{
    path = $0
    if (path == "") next

    set_style(path)
    in_block = 0
    in_hd = 0; hd_delim = ""
    in_pydoc = ""; pydoc_is_doc = 0; py_prev_cont = 0
    f_code = 0; f_comment = 0; f_blank = 0; f_total = 0

    r = (getline line < path)
    if (r < 0) {
        printf("sloc.awk: cannot read %s\n", path) > "/dev/stderr"
        rc = 1
        next
    }
    files++
    while (r > 0) {
        f_total++
        if (in_hd) {
            # Inside a heredoc the shell's own comment syntax does not apply: a
            # leading # is payload the script emits, or a comment belonging to
            # some embedded language, and either way it is not a comment in THIS
            # file. Counting it as one undercounted 122 lines across this tree.
            if (hd_closes(line))       { in_hd = 0; f_code++ }
            else if (trim(line) == "")  f_blank++
            else                        f_code++
        } else if (LC == "#" && f_total == 1 && line ~ /^#!/) {
            # A shebang is an interpreter directive, not a comment. The test
            # that separates the two: deleting a comment cannot change how the
            # file behaves, and deleting this line changes what runs it.
            f_code++
        } else {
            classify(line)
            if (has_code)         f_code++
            else if (has_comment) f_comment++
            else                  f_blank++
            if (HD && has_code) detect_heredoc(line)

            # Did this code line end mid-statement? A triple quote opening the
            # NEXT line is then data, not a docstring. Skipped while a run is
            # open, so the string's own content cannot set it.
            #
            # One line of memory, not bracket depth. Depth drifts, and drift
            # here is unbounded. See docs/code-review-priors.md for the
            # measurement that settled it.
            if (PYDOC && in_pydoc == "" && has_code) {
                ptail = (cmt_pos > 0) ? substr(line, 1, cmt_pos - 1) : line
                sub(/[ \t\r]+$/, "", ptail)
                if (ptail != "") py_prev_cont = (ptail ~ /[([{,=+\\]$/)
            }
        }
        r = (getline line < path)
    }
    close(path)

    # Real source never ends inside a block comment, so this is a misparse:
    # every line after the false open was booked as comment. Fail loud.
    if (in_block) {
        printf("sloc.awk: unterminated block comment in %s, comment count is wrong\n",
               path) > "/dev/stderr"
        rc = 1
    }

    # Same canary for the heredoc tracker. A false open is the failure mode that
    # matters here, because it books every line to EOF as code and silently eats
    # the file's real comments. Reaching EOF still inside one says the delimiter
    # was never matched, which a working script cannot do.
    if (in_hd) {
        printf("sloc.awk: unterminated heredoc (%s) in %s, counts are wrong\n",
               hd_delim, path) > "/dev/stderr"
        rc = 1
    }

    # And for the docstring tracker, whose failure direction is the same as the
    # block comment's: every line after a false open was booked as comment.
    if (in_pydoc != "") {
        printf("sloc.awk: unterminated docstring (%s) in %s, comment count is wrong\n",
               in_pydoc, path) > "/dev/stderr"
        rc = 1
    }

    code += f_code; comment += f_comment; blank += f_blank; total += f_total
    if (per_file) printf("%d %d %d %d %s\n", f_code, f_comment, f_blank, f_total, path)
}

END {
    if (!per_file)
        printf("code=%d comment=%d blank=%d total=%d files=%d\n",
               code, comment, blank, total, files)
    exit rc
}
