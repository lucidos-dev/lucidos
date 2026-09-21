---
name: project-stats
description: Use when user asks about project size, lines of code, test coverage, codebase comparison, or "how big is the project"
---

# Project Stats

Report Lucidos project size, test coverage, and comparison to similar projects.

## Counting Rules

**Count code lines the conventional way: blank lines and comment lines are NOT code.**
This is cloc's / tokei's / scc's definition, and it is what every benchmark in the
comparison table below is quoted in. A line is code OR comment OR blank, never two at
once; a line carrying both code and a trailing comment counts as code.

`wc -l` is **not** an acceptable substitute and must not appear in a reported figure.
It counts prose as code, and it does so unevenly. Comment share runs about 21% across
the engine's Rust as a whole, but above 45% in a dozen of the most recently rewritten
files. So a raw total overstates by roughly 40%, and overstates hardest in exactly the
modules that grew last. Quoting that against ripgrep's ~50K would be comparing two
different units.

Nothing in the cloc family is installed here, so the counter is bundled with this skill:
[`sloc.awk`](sloc.awk). It reads a newline-separated file list on stdin, never argv, and
prints `code=<n> comment=<n> blank=<n> total=<n> files=<n>`. Reading stdin is what makes
the count whole: the old `… | xargs wc -l | tail -1` idiom breaks silently once the file
list exceeds `ARG_MAX`, because xargs then runs `wc` more than once and `tail -1` keeps
only the LAST batch's total. Measuring VS Code caught it in the act, reporting 973,759
lines for a tree that has 2,474,634. It undercounts without any sign that it did. With `-v per_file=1` it prints
`code comment blank total path` per file instead.

It does five things beyond splitting on a comment token, each because the naive version
was measurably wrong:

- **Skips string literals.** The `/*` inside `engine/command_guard.rs`'s `"/" | "/*"`
  match arm reads as a block-comment open otherwise, and with no `*/` later in the file
  that one misread booked 944 lines of code as comment.
- **Tracks shell heredocs.** A `#` in a heredoc body is payload the script emits, or a
  comment in an embedded language, never a comment in the `.sh` file: 122 lines here.
- **Counts the shebang as code.** Deleting a comment cannot change how a file behaves and
  deleting `#!/usr/bin/env bash` changes what runs it: 110 scripts.
- **Reads `{/* … */}` as a comment in TSX.** Those braces exist only to host the comment:
  151 comment-only lines here were reading as code.
- **Books a Python docstring as comment**, which is cloc's rule. This tree holds two
  Python files, so it earns its place on foreign repos: without it a scanner reads every
  `#` line as code, which is most of what a Python tree writes.

Fixtures for all five, and for the shapes that must NOT trigger them (`<<<` here-strings,
`1 << 20` shifts, a `<<` inside a quoted string, `<div>{/* x */}`, `} /* note */`), are in
[`sloc_test.sh`](sloc_test.sh); run it after touching the counter. Three canaries turn a
misparse into a non-zero exit plus a named file rather than a quietly wrong number:
unterminated block comment, unterminated heredoc, and more than one heredoc opened on a
line. Watch stderr. The limits that remain are deliberate and measured, and the reasoning
is in the counter's header and in `docs/code-review-priors.md`, so check there before
"fixing" one.

**Count tracked files only, and let git decide which those are.** `git ls-files` excludes
`target/`, `node_modules/`, `dist/` and `.lucidos/` because all four are gitignored. That
last one is the one worth naming: it holds every coding-agent worktree
(`.lucidos/worktrees/`) and the release worktrees beside them, each a full copy of the
tree. Counting them inflates by 3-4x.

Do not hand-write an exclusion list instead. One did exist here, and it named
`.worktrees/`, a directory this repo has never had. The real path is `.lucidos/worktrees/`
(`engine/git_ops/worktree.rs`), so the entry excluded nothing.

## Commands

Run all from repo root.

**Enumerate the tree. Never hand-list what to measure.** Every loop below globs
`crates/*/` or `crates/lucidos-engine/src/*/` for one reason: a literal list of paths
drops whatever was added after the list was written, and drops it in silence. The report
just comes back short.

This skill shipped such a list, and it was already incomplete on the day it was authored.
`triggers/` and `lucidos-cli` predate it. By 2026-09 it named nine of twelve engine
modules and two of thirteen crates. That hid over 19K lines of engine code, and eleven
whole crates. Nothing failed, so nothing said so. The coverage check below is what makes
a residual gap loud.

Paste this preamble first. It caches the tracked-file list once, and every later command
is a `grep` over it. So no command carries its own idea of what to exclude, and nothing
walks `node_modules`.

```bash
SLOC=.claude/skills/project-stats/sloc.awk
FILES=/tmp/sloc-files.$$
git ls-files | sort > "$FILES"
src() { grep -E "$1" "$FILES"; }
```

The `$$` is not decoration. Coding-agent sessions share the host, so a fixed
`/tmp/sloc-files.txt` lets a concurrent run overwrite this one's list mid-report. The
figures would then describe another branch's tree, with nothing to say they do. Same
hazard class as the `pgrep` and `git stash` warnings in `CLAUDE.md`.

**Use `git ls-files`, not `find` with a prune list.** A prune list is itself a
hand-maintained list of paths, which is the exact failure this page is written against.
The version that shipped first pruned `-name .worktrees` and matched nothing. A run from
the main checkout then walked into `.lucidos/worktrees/`, counting every agent worktree as
extra source. A run from inside a worktree looked clean, because there that directory is
nearly empty. Git already knows what is ignored, so ask it.

The tradeoff is explicit: an untracked new file is not counted. That is the right default
for a project-size figure, and it applies to a foreign repo too.

`src` takes a **regex**, not a glob, so one language can span several extensions. Do not
rewrite it to take a list of extensions and loop with `for e in $exts`. The Bash tool
runs zsh, which does not word-split an unquoted expansion, so that loop silently searches
for the literal `*.ts tsx` and reports zero. Both multi-extension rows read 0 when this
file first shipped that version.

Write `command find` and `command npx`, not bare `find` and `npx`. The `command` builtin
runs the real binary and skips any shell alias or function shadowing the name. A wrapper
that does not accept compound `find` predicates otherwise breaks these commands.

### Size by language

One row per language, every file counted exactly once. Two things are outside the total
by design. There is no `.html` row, because the counter has no HTML mode and would fall
back to C-style tokens on the four HTML files. Markdown is documentation, not code.

Two things are inside it, deliberately. Generated sources under `src/generated/` are
tracked code and cloc counts them, and they are about 3K lines. The one vendored bundle,
`html2canvas.min.js`, is minified into 20 lines, so it distorts nothing.

```bash
for lang in 'Rust:\.rs$' 'TypeScript:\.tsx?$' 'Shell:\.sh$' 'CSS:\.css$' \
            'SQL:\.sql$' 'JavaScript:\.(js|mjs|cjs)$' 'Python:\.py$'; do
  printf '%-12s %s\n' "${lang%%:*}" "$(src "${lang#*:}" | awk -f "$SLOC")"
done
```

### Rust by module and by crate

```bash
for d in crates/lucidos-engine/src/*/; do
  echo "${d%/} $(src "^${d%/}/.*\.rs$" | awk -f "$SLOC")"
done
echo "engine/src root $(src '^crates/lucidos-engine/src/[^/]*\.rs$' | awk -f "$SLOC")"

for c in crates/*/ signers/*/; do
  echo "${c%/} $(src "^${c%/}/.*\.rs$" | awk -f "$SLOC")"
done
```

`signers/*/` is in that glob because two plugin crates live outside `crates/`. They are
also outside the cargo workspace, so the root `Cargo.toml` member list does not name them
either. A crate is not always where the obvious glob looks, which is what the coverage
check below exists to catch.

### Coverage check: run it every time

The crate rows are a decomposition, so they must account for every tracked Rust file.
This names any file that reached no row. Treat a gap as a failed report, not a footnote.

```bash
# Whole: every tracked Rust file
src '\.rs$' > /tmp/sloc-whole.$$

# Rows: rebuilt from the same loop the report prints
{ for c in crates/*/ signers/*/; do src "^${c%/}/.*\.rs$"; done; } | sort -u > /tmp/sloc-rows.$$

diff /tmp/sloc-whole.$$ /tmp/sloc-rows.$$ \
  && echo 'COVERAGE OK' || echo 'COVERAGE GAP: the files above reached no row'
```

**Build the rows from the printed loop, never as a complement.** The first version of this
check defined its last term as "every Rust file not under `crates/lucidos-engine/src/`".
Unioned with the two engine terms above it, that is the whole tree by construction, so the
diff was always empty and `COVERAGE OK` printed unconditionally.

It was not idle, it was actively misleading. The per-crate loop read `crates/*/` at the
time, which cannot match `signers/`. So two crates got no row while the check called the
report complete. A check that mirrors the real loop fails loudly on exactly that.

The module rows are a second, narrower view: every file under `crates/lucidos-engine/src/`
is either in a subdirectory or at the root, so that split needs no check. Four engine
files sit outside `src/` altogether, `build.rs` and three in `tests/`, and the per-crate
row is what counts those.

### Production versus test split

The skill asks you to compare production against production, so here is the command for
it. Rust keeps unit tests inline, so its split is the region from the first
`#[cfg(test)]` to end of file.

```bash
OUT=$(mktemp -d); mkdir -p "$OUT/prod" "$OUT/test"; n=0
src '\.rs$' > "$OUT/files.txt"

while IFS= read -r f; do n=$((n+1))
  awk -v p="$OUT/prod/$n.rs" -v t="$OUT/test/$n.rs" \
    '/^[[:space:]]*#\[cfg\(test\)\]/ && !it { it=1 } { print > (it ? t : p) }' "$f"
done < "$OUT/files.txt"

echo "Rust prod  $(command find "$OUT/prod" -name '*.rs' | awk -f "$SLOC")"
echo "Rust test  $(command find "$OUT/test" -name '*.rs' | awk -f "$SLOC")"
rm -rf "$OUT"

TESTPAT='(__tests__/|\.test\.tsx?$|\.spec\.tsx?$|/e2e/|/tests/)'
src '\.tsx?$' > /tmp/sloc-ts.$$

echo "TS prod  $(grep -vE "$TESTPAT" /tmp/sloc-ts.$$ | awk -f "$SLOC")"
echo "TS test  $(grep -E  "$TESTPAT" /tmp/sloc-ts.$$ | awk -f "$SLOC")"
echo "sh prod  $(src '\.sh$' | grep -vE '_test\.sh$' | awk -f "$SLOC")"
echo "sh test  $(src '_test\.sh$' | awk -f "$SLOC")"
```

### Test counts

`#[tokio::test]` is about 27% of the Rust tests here, so a bare `#[test]` grep drops
roughly a quarter of them. Match both attributes.

```bash
grep -rhE '#\[(tokio::)?test\b' --include='*.rs' ./crates ./signers | wc -l
for d in crates/lucidos-engine/src/*/ crates/*/ signers/*/; do
  echo "$(grep -rhE '#\[(tokio::)?test\b' --include='*.rs' "${d%/}" 2>/dev/null | wc -l | tr -d ' ') ${d%/}"
done

( cd crates/lucidos-app && command npx vitest --run --reporter=dot 2>&1 | tail -5 )
grep -rhcE '^[[:space:]]*(test_|it_)[a-z_]*\(\)' --include='*_test.sh' ./scripts | awk '{s+=$1} END {print s+0}'
```

Two things there are load-bearing. Keep the `cd` inside a subshell, or every command after
it runs from the wrong directory and `$SLOC` stops resolving. Use `--reporter=dot`, not
`verbose`, which prints all 14K test names through the pipe before `tail` discards them.

### Comment density

Only when asked about it.

```bash
command find crates/lucidos-engine/src -name '*.rs' | awk -v per_file=1 -f "$SLOC" \
  | awk '$4 > 300 { printf "%3.0f%% %6d %s\n", $2 * 100 / $4, $4, $5 }' | sort -rn | head -12
```

## Rust Module Descriptions

**This table names things. It never decides scope.** The loops above do that. A module or
crate missing here is still measured and still reported, with its description left blank.
That is the whole point: the previous version of this table doubled as the scope list, so
a gap in it silently became a gap in the numbers.

Engine modules, under `crates/lucidos-engine/src/`:

| Module | What it does |
|---|---|
| `engine/` | Core orchestrator: chat, agentic loop, Claude Code, tools, event bus, threads |
| `core/` | Events, event store, artifacts, credentials, preferences, backup, email, OAuth |
| `api/` | HTTP routes, SSE, skill UI serving |
| `llm/` | Provider trait, Vertex AI, OpenAI, tool definitions |
| `runtime/` | Python and browser execution |
| `voice/` | Voice calls and the voice mode of a thread |
| `scheduler/` | Cron tasks, notifications, persistence |
| `triggers/` | Trigger definitions, matching, fire-time dispatch |
| `memory/` | Embeddings, FastEmbed, pgvector index |
| `capability_manifest/` | App, plugin and signer manifest handling |
| `mcp/` | MCP server integration |
| `bin/` | Test data generators |

Workspace crates:

| Crate | What it does |
|---|---|
| `lucidos-engine` | The engine: everything in the table above |
| `lucidos-e2e` | API end-to-end suite |
| `lucidos-gateway` | Multi-workspace gateway and routing |
| `lucidos-app` | Tauri desktop shell, plus the frontend under `src/` |
| `lucidos-eval` | Evaluation harness |
| `lucidos-cli` | The `lucidos` command-line tool |
| `lucidos-installs` | Install discovery and update checks |
| `lucidos-build-slot` | Build-slot arbitration between concurrent builds |
| `lucidos-local-token` | Local auth token issue and verify |
| `lucidos-tailscale` | Tailscale integration |
| `lucidos-file-backup-exclusion` | Marks paths excluded from OS backup |
| `signers/binance-hmac` | Request-signing plugin, outside `crates/` and the workspace |
| `signers/test-echo` | Signing-plugin test fixture, same location |

## Comparison Benchmarks

Present total **code lines**: the sum of every row the by-language loop printed, excluding
docs and markdown. Name the languages the run actually found, rather than repeating a list
from here. The set grows.

**Every figure below was measured with `sloc.awk` itself**, on 2026-08-03, at each
project's default-branch HEAD, over the scope named in the row. That provenance is the
point of the table. It previously carried remembered round numbers of unknown origin and
unknown unit (ripgrep ~50K, bat ~30K, delta ~40K, GitLens ~200K, VS Code ~600K), and
measuring them found every one wrong, in both directions: bat and delta were about
double the truth, VS Code was low by a factor of 2.3. A comparison is only worth making
when both sides were counted the same way, so re-measure rather than quoting a figure
from memory, and record the scope when you do.

| Project | Scope measured | Code lines |
|---|---|---:|
| bat | all `*.rs` | 14.8K |
| delta | all `*.rs` | 22.5K |
| ripgrep | all `*.rs` | 39.7K |
| GitLens | `src/**/*.{ts,tsx}` | 276K |
| VS Code | `src/**/*.{ts,tsx}`, excluding `*.test.ts` | 1.36M |
| VS Code | `src/**/*.{ts,tsx}`, tests included | 1.88M |

| Range | Category |
|---|---|
| 5K-30K | Typical solo/side project (bat sits here) |
| 30K-100K | Substantial open-source tool (ripgrep, delta) |
| 100K-500K | Mid-size product (GitLens) |
| 500K+ | Large product (VS Code) |

Two caveats that survive correct counting, so state them when you report:

- **Scope differs.** The Lucidos figure sums five languages; the VS Code and GitLens
  rows are TypeScript only, and the Rust rows are one language by definition. A
  multi-language total is structurally larger than a single-language one.
- **Tests differ.** Rust keeps unit tests inline in the source files, so every Rust row
  here (Lucidos included) counts its tests. The VS Code rows show both, which is why
  they are 520K apart.

State the unit once when reporting ("code lines, comments and blanks excluded") so the
comparison is not read as a raw file-length total.

Measuring a foreign repo needs two flags this repo never does: run the counter under
`LC_ALL=C` (a non-UTF-8 source file, such as bat's `tests/snapshots/sample.modified.rs`,
aborts awk outright otherwise), and read stderr, since fixture files that embed `/*`
inside a string trip the unterminated-block warning and make that project's comment
column unreliable.

Stderr carries a third thing there: an extension missing from `set_style`'s table falls
back to `//` plus `/* */` and warns per file. Treat that warning as a result, not noise.
For a C-family language the fallback is right, so list the extension and the warning
stops drowning the canaries. Otherwise the column is wrong until the language is added.
That is why `.py` is in the table rather than left to the fallback.

Compare production code with production code. A path split on `src/test/`, `tests/` and
`*.test.ts` gets most of it. Rust hides its unit tests inline, so the Rust side needs the
region from each `#[cfg(test)]` to end of file counted separately. Skip that and a repo
with inline tests looks larger than one keeping them in a test tree.

## Reporting Shape

Lead with code lines. Carry comment lines as their own column rather than folding them in
or dropping them: comment share is the interesting secondary signal, and showing it is
what keeps the headline number honest.

| Module | Code | Comments | Comment share | Files |
|--------|-----:|---------:|--------------:|------:|
| `engine/` | … | … | …% | … |

Comment share is `comment / total`, matching the comment-density command above, so blanks
are in the denominator.

Report the production and test split too. Test code is a large share of this tree, and a
single headline number reads as behavior when much of it is assertions.

## Test Density Assessment

Ratios are **code lines per test**, using the same code-only figure as everything else.
Produce a per-module assessment table:

| Module | Code | Tests | Ratio | Rating |
|--------|-----:|------:|-------|--------|
| `some-module/` | 16,029 | 150 | 1:107 | Good |
| `other-module/` | 9,728 | 0 | n/a | Needs tests |
| Frontend (TS/TSX) | 26,056 | 687 | 1:38 | Excellent |
| **Overall** | **70K** | **1,059** | **1:66** | **Production-grade** |

The names and numbers there show the table's shape. They are not current figures, and
they are not claims about any real module. Always recompute.

Write `n/a` in an empty ratio cell, never a bare dash. An em dash is banned repo-wide
(`.claude/rules/no-em-dashes.md`) and a hyphen reads as a minus beside numbers.

### Rating scale

| Ratio | Rating |
|---|---|
| 1:1–1:50 | Excellent |
| 1:50–1:100 | Production-grade |
| 1:100–1:200 | Good |
| 1:200–1:500 | Adequate, room to improve |
| 1:500+ | Needs more coverage |
| 0 tests | Needs tests |

### Rules
- Rate **every Rust module, every crate, and the frontend** individually, not just totals. The set comes from the loops, so a module added tomorrow gets a row without editing this file
- Modules with **0 tests** always get "Needs tests" regardless of size
- **Exclude two things, and state why in the report.** `bin/` is test-data generators. `lucidos-e2e` *is* the API e2e suite, so rating it against itself measures nothing
- **Do not exclude `lucidos-app`.** An earlier rule here dropped it as a Tauri shell "not expected to have tests". It has hundreds. Verify an exclusion against the test count before applying it, rather than against what a crate sounds like
- Ratios move when the counting unit does. A module's ratio computed on code lines is roughly 20-40% tighter than the same module's ratio computed on `wc -l`, so don't compare a fresh number against one quoted in an older session that used raw lines
- Rust code columns include inline tests, because `#[cfg(test)]` lives in the source file. Give the frontend row the same treatment so the two are comparable, then note the production-only ratio beside it
- After the table, add a **Summary** paragraph: overall assessment, which modules are strongest, which need the most attention, and whether overall density is appropriate for the project's stage
