# 0558: /harden always asks the other model family to review

- **Status**: Accepted
- **Date**: 2026-10-23

## Context

ADR 0476 gave every `/harden` iteration one Claude review plus Codex on the
first iteration. That second review ran in one direction only. A Claude Code
run got Codex. A Codex run got nothing: "Codex review: skipped (Codex-backend
run)". So a Codex-built diff shipped with two passes from one family, the shape
ADR 0476 itself calls weak.

A Codex-backend run also has no background shell (ADR 0292). A job it starts
with `&` dies with the shell call, so the Claude Code launch could not be
copied.

The plan is `docs/plans/2026-10-23-harden-other-model-review.md`.

## Decision

Each first iteration asks the family other than the hardening backend to
review. Codex reviews a Claude Code run, and Claude reviews a Codex run. Both
directions launch through one engine background task
(`lucidos background-task run --poll`), and Phase 3 joins it with `wait`.

## Rationale

- **The other family is the point.** Same-model reviewers saturate, and a
  different family keeps adding catches. That holds in both directions.
- **One script, two adapters.** `scripts/harden-other-model-review.sh` owns the
  probes, the retry, the attempt timeout, the env scrub and the status line.
  Each reviewer is a small adapter in `scripts/lib/other_model_review.sh`.
- **Selection lives in tested code.** The `decide` subcommand turns the run's
  facts into `run <reviewer> <base>` or a `skipped` line. No input skips a
  review because of the backend.
- **The engine's background task works on both backends.** It runs in the
  worktree, outlives a shell call, and `wait` joins it. One launch and one join
  replace two shapes.
- **The Claude reviewer cannot write.** It gets `Read`, `Grep` and `Glob` only,
  under `dontAsk`, with no MCP servers and no Lucidos credential. It reads the
  diff from `.lucidos/other-model-review.diff`, so it needs no `Bash`.

## Consequences

- A Codex-built diff now gets a Claude review at `opus`, effort high, capped
  at $5 per run.
- The review stays advisory. `NO VERDICT` is reported as configuration and
  `unavailable` as a note. Neither blocks the hardened marker.
- An abandoned review is stopped with `lucidos background-task stop`, so it
  spends nothing after `/harden` moves on.
- `HardenRunCompleted` records `findings.other_model`, `other_model_reviewer`
  and `other_model_status`.
- A contributor with one backend installed gets `unavailable`, loudly.
- The Codex driver tests still run alone, after the review ends. The gate now
  keys on `.lucidos/other-model-review.done`.

## Live check

Both directions ran once against this change's own branch, base `main`, in an
engine background task. Each returned a verdict on the first attempt.

| Reviewer | Wall time | Cost | Findings |
|---|---|---|---|
| Codex, for a Claude Code run | 77 s | not reported by the companion | none |
| Claude, for a Codex run | 120 s | $1.06, served by `claude-opus-5-5` | two real bugs |

Both Claude findings were fixed before Apply. The watchdog had put the reviewer
in a process group of its own, so `background-task stop` missed it. A stopped
review also left a stale `.out` that a later `skipped` iteration never cleared.

## Alternatives considered

- **Keep Claude Code on its shell `&` launch.** It works there, but leaves two
  launch shapes and two joins to keep in step. The user chose one shape.
- **Pick the reviewer by who built the diff.** In Lucidos the builder and the
  hardener are the same thread, so this adds a fact without a difference.
- **`Bash` for the Claude reviewer, limited to git.** The Phase 0 spike showed
  a user's `auto` mode approving a `touch`. Under `dontAsk`, project allow rules
  still let `cargo check` run, and a `cargo test` beside the early suite run can
  OOM the host.
- **Opus at effort xhigh.** About 70% more spend for 4% more intelligence on
  the ladder. High matches the main `code-review`.
- **A compatibility shim for `harden-codex-review.sh`.** A run that merged
  `main` mid-harden fails one advisory launch, and the next run reads the new
  file. Not worth a shim.
