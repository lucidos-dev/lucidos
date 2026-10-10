# 0292: /harden starts its test suites during review

- **Status**: Accepted
- **Date**: 2026-09-26
- **Amended by**: [0391](0391-agent-teardown-reaps-by-agent-run-marker.md). The
  turn's process group named below never held a Bash call. The teardown now
  reaps the early run by agent run marker.

## Context

`/harden` ran the Phase 4.5 test suites strictly after the review phases. Over
the week to 2026-09-26, Rust hardens took a median of 27 min, and review took a
median of about 4 min (p90 13 min). Starting the suites earlier saves roughly
the review time. But review can fix code, and then the early result describes a
tree that no longer exists.

Engine tests read repo files in two ways. Cargo dep-info lists the compile-time
inputs, `include_str!` targets included. Run-time reads are invisible to it, and
they are wide: the voice scan walks every crate's sources, and several tests
walk `system-knowhow/`.

## Decision

`/harden` starts every suite the Phase 4.5 selection picks at the Phase 1
kickoff, through `scripts/harden-suites.sh start --early`. This is an *early
suite run*. Its result counts only if every path changed since its start commit
is on `HARDEN_SAFE_PATHS`, a short allowlist of files no test reads. Any other
change voids it, and the suites run again.

## Rationale

An allowlist of known-safe paths fails closed by construction. It fails open
only if a new test reads an allowlisted file, and the script checks for exactly
that before it trusts the list. A list of every file the tests read has the
opposite property: forgetting one entry passes a stale result. The allowlist
covers the one common review edit that touches nothing the suites read, a new
entry in the priors ledger or the registry.

A fix outside the allowlist stops the early run before the edit, so a mixed
tree is never tested. Each suite also records what changed when it exits, so a
missed stop voids the result instead of passing it.

The early run stays in the turn's process group. The engine ends that group
when a turn ends, and a job that escaped it would outlive `/harden`.

The early engine run skips `runtime::codex::driver_tests` and
`runtime::codex_app_server::driver_tests`, and runs them alone after the Codex
review is joined. Those tests spawn a shell stub and share no state with a
real Codex CLI. Their timeouts fail under host load, and a Codex review running
beside the engine suite is load.

## Consequences

- Most Rust hardens finish about one review phase sooner.
- A review that fixes code gets no saving and loses nothing against the old
  order: the early run stops and the suites run once after the fix.
- Suite selection lives in the script rather than in a prose table, so it is
  tested. A file any crate compiles in selects the Rust suite. The script reads
  that list from dep-info and from `include_str!` literals in source, so a
  docs-only edit to `CHANGELOG.md` no longer skips it.
- Codex-backend runs have no background shell. They start at Phase 4.5 and
  wait at once, as before.

## Alternatives considered

- **Dep-info plus a list of run-time reads.** Re-run only when a fix touches
  either. It keeps more early results in theory, but the run-time set already
  spans `crates/**` and `system-knowhow/**`, and it fails open on the first
  unlisted read.
- **Any commit re-runs.** Simplest, but it loses the saving whenever Phase 3
  only logs a priors entry.
- **Hold fixes until the join.** The early run is never wasted, but a fix then
  waits for a whole suite before the second one starts.
- **Run the early suite in a snapshot worktree.** Edits never reach it, but a
  separate target dir means a cold build, and a shared one hits the baked-path
  hazard in ADR 0079.
- **Make the driver tests tolerate load.** Cleaner, but the cause is unproven,
  and skipping them early is correct whatever it turns out to be.
