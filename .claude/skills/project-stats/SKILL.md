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
It counts prose as code, and it does so unevenly: comment share runs about 18% across
the engine's Rust as a whole but above 40% in a dozen of the files rewritten most
recently, so a raw total overstates by roughly a third and overstates hardest in exactly
the modules that grew last. Quoting that against ripgrep's ~50K would be comparing two
different units.

Nothing in the cloc family is installed here, so the counter is bundled with this skill:
[`sloc.awk`](sloc.awk). It reads a newline-separated file list on stdin, never argv, and
prints `code=<n> comment=<n> blank=<n> total=<n> files=<n>`. Reading stdin is what makes
the count whole: the old `… | xargs wc -l | tail -1` idiom breaks silently once the file
list exceeds `ARG_MAX`, because xargs then runs `wc` more than once and `tail -1` keeps
only the LAST batch's total. Measuring VS Code caught it in the act, reporting 973,759
lines for a tree that has 2,474,634. It undercounts without any sign that it did. With `-v per_file=1` it prints
`code comment blank total path` per file instead.

It does four things beyond splitting on a comment token, each because the naive version
was measurably wrong here:

- **Skips string literals.** The `/*` inside `engine/command_guard.rs`'s `"/" | "/*"`
  match arm reads as a block-comment open otherwise, and with no `*/` later in the file
  that one misread booked 944 lines of code as comment.
- **Tracks shell heredocs.** A `#` in a heredoc body is payload the script emits, or a
  comment in an embedded language, never a comment in the `.sh` file: 122 lines here.
- **Counts the shebang as code.** Deleting a comment cannot change how a file behaves and
  deleting `#!/usr/bin/env bash` changes what runs it: 110 scripts.
- **Reads `{/* … */}` as a comment in TSX.** Those braces exist only to host the comment:
  151 comment-only lines here were reading as code.

Fixtures for all four, and for the shapes that must NOT trigger them (`<<<` here-strings,
`1 << 20` shifts, a `<<` inside a quoted string, `<div>{/* x */}`, `} /* note */`), are in
[`sloc_test.sh`](sloc_test.sh); run it after touching the counter. Three canaries turn a
misparse into a non-zero exit plus a named file rather than a quietly wrong number:
unterminated block comment, unterminated heredoc, and more than one heredoc opened on a
line. Watch stderr. The limits that remain are deliberate and measured, and the reasoning
is in the counter's header and in `docs/code-review-priors.md`, so check there before
"fixing" one.

**Exclude** `.worktrees/`, `target/`, `node_modules/`, `dist/` from every count. Worktrees
duplicate the entire codebase and inflate counts 3-4x.

## Commands

Run all from repo root:

```bash
SLOC=.claude/skills/project-stats/sloc.awk

# Rust by module: prints "<dir> code=… comment=… blank=… total=… files=…"
for dir in crates/lucidos-engine/src/engine crates/lucidos-engine/src/core crates/lucidos-engine/src/llm crates/lucidos-engine/src/memory crates/lucidos-engine/src/runtime crates/lucidos-engine/src/scheduler crates/lucidos-engine/src/api crates/lucidos-engine/src/mcp crates/lucidos-engine/src/bin crates/lucidos-app/src; do echo "$dir $(command find "$dir" -name '*.rs' 2>/dev/null | awk -f "$SLOC")"; done

# Frontend (TS/TSX, CSS separately)
command find ./crates/lucidos-app -name '*.ts' -o -name '*.tsx' | grep -v node_modules | grep -v dist | awk -f "$SLOC"
command find ./crates/lucidos-app -name '*.css' | grep -v node_modules | grep -v dist | awk -f "$SLOC"

# SQL migrations
command find ./crates/lucidos-engine/migrations -name '*.sql' | awk -f "$SLOC"

# Shell scripts
command find ./scripts -name '*.sh' | awk -f "$SLOC"

# Comment-heaviest Rust files over 300 lines (only when asked about comment density)
command find crates/lucidos-engine/src -name '*.rs' | awk -v per_file=1 -f "$SLOC" | awk '$4 > 300 { printf "%3.0f%% %6d %s\n", $2 * 100 / $4, $4, $5 }' | sort -rn | head -12

# Rust test count (total + per module), BOTH attributes: ~29% of the tests
# under ./crates are #[tokio::test], which a bare #[test] grep silently drops
grep -rhE '#\[(tokio::)?test\b' --include='*.rs' ./crates | wc -l
for dir in crates/lucidos-engine/src/engine crates/lucidos-engine/src/core crates/lucidos-engine/src/llm crates/lucidos-engine/src/memory crates/lucidos-engine/src/runtime crates/lucidos-engine/src/scheduler crates/lucidos-engine/src/api crates/lucidos-engine/src/mcp crates/lucidos-engine/src/bin crates/lucidos-app/src; do tests=$(grep -rhE '#\[(tokio::)?test\b' --include='*.rs' "$dir" 2>/dev/null | wc -l | tr -d ' '); echo "$tests $dir"; done

# Frontend test count
cd crates/lucidos-app && command npx vitest --run --reporter=verbose 2>&1 | tail -5
```

Use `command find` and `command npx` to bypass rtk hook interception (rtk doesn't support
compound find predicates).

## Rust Module Descriptions

| Module | What it does |
|---|---|
| `engine/` | Core orchestrator: chat, agentic loop, Claude Code, tools, event bus, threads |
| `core/` | Events, event store, artifacts, credentials, preferences, backup, email, OAuth |
| `api/` | HTTP routes, SSE, skill UI serving |
| `llm/` | Provider trait, Vertex AI, OpenAI, tool definitions |
| `runtime/` | Python & browser execution |
| `scheduler/` | Cron tasks, notifications, persistence |
| `memory/` | Embeddings, FastEmbed, pgvector index |
| `mcp/` | MCP server integration |
| `bin/` | Test data generators |
| `lucidos-app` | Tauri desktop shell |

## Comparison Benchmarks

Present total **code lines** (Rust + TS/TSX + CSS + SQL + shell, excluding docs/markdown).

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

## Reporting Shape

Lead with code lines. Carry comment lines as their own column rather than folding them in
or dropping them: comment share is the interesting secondary signal, and showing it is
what keeps the headline number honest.

| Module | Code | Comments | Comment share | Files |
|--------|-----:|---------:|--------------:|------:|
| `engine/` | … | … | …% | … |

## Test Density Assessment

Ratios are **code lines per test**, using the same code-only figure as everything else.
Produce a per-module assessment table:

| Module | Code | Tests | Ratio | Rating |
|--------|-----:|------:|-------|--------|
| `engine/` | 16,029 | 150 | 1:107 | Good |
| `core/` | 9,728 | 0 | — | Needs tests |
| Frontend (TS/TSX) | 26,056 | 687 | 1:38 | Excellent |
| **Overall** | **70K** | **1,059** | **1:66** | **Production-grade** |

Those numbers are an illustration of the table's shape, not current figures. Always
recompute.

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
- Rate **every Rust module** and the **frontend** individually, not just totals
- Modules with **0 tests** always get "Needs tests" regardless of size
- Exclude `bin/` (test data generators) and `lucidos-app` Tauri shell from density assessment: they're not expected to have tests
- Ratios move when the counting unit does. A module's ratio computed on code lines is roughly 20-40% tighter than the same module's ratio computed on `wc -l`, so don't compare a fresh number against one quoted in an older session that used raw lines
- After the table, add a **Summary** paragraph: overall assessment, which modules are strongest, which need the most attention, and whether overall density is appropriate for the project's stage
