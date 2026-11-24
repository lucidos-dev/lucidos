# 0752: Hardening rules stay in bash: a selector must run from the tree under test before any compile, and a typed port would prevent 4 of the 12 selection fixes read

- **Status**: Accepted
- **Date**: 2026-11-21

## Context

Rank 14 of the architecture roadmap proposed moving build and hardening logic
from bash into `lucidos-cli`, so that suite selection has typed tests.
The hardening scripts took 49 fix commits in 30 days.
The [measurement](../plans/2026-11-21-build-logic-bash-to-cli-measurement.md)
classified those fixes and timed the bash fixture test.

## Decision

The rules in `scripts/lib/harden_suites.sh`, `scripts/harden-suites.sh` and
`scripts/harden-gates.sh` stay in bash.
Rank 14 is closed with no plan.

## Rationale

- **The rules must come from the tree under test.** A branch that edits a
  selection rule is selected by its own edit. The `lucidos` binary on `PATH`
  is built from `main`, so a selector inside it applies stale rules.
- **A Rust selector therefore compiles before it selects.** `lucidos-cli` has
  198 dependency crates. Selection and the cheap gates would wait for a build
  slot, which the cheap-gates work removed.
- **Hardening must report on a tree that does not compile.** A fresh clone or
  a broken build has no binary to ask.
- **Typing prevents little of what recurs.** Of 17 selection fixes, 12 were
  read. Six added a path row, three added a suite to a property list, two
  changed a graph rule and one was a shell-language bug.
- **The slow test stays slow.** The fixture test takes 214 s. Its runner part
  takes 155 s and drives real process trees, which a Rust test needs too.
- **Bash readers remain.** `scripts/e2e-github-shard.sh` and
  `scripts/scoreboard.sh` source the suite lists. A move leaves a generated
  second copy.

## Consequences

- The scripts keep their bash test, `scripts/lib/harden_suites_test.sh`.
- The hardening marker stays in Rust (`lucidos hardened mark`). It records a
  result and selects nothing, so the tree-under-test argument does not apply.
- The suite properties still live in about eight bash lists. One bash table
  with a completeness test would cover that class. It is open work, not part
  of this decision.
- Reopen this when a selector can run from the tree with no compile, or when
  shell-language bugs become the recurring class.

## Alternatives considered

- **A selector command in `lucidos-cli`.** Lost on the two points above: stale
  rules from `PATH`, or a 198-crate compile before the first gate.
- **A dependency-free selector crate.** It should compile fast enough to skip
  the slot. It still builds before the first gate and keeps the bash readers,
  for about four prevented fixes a month. Not built or timed.
- **Generate the bash lists from Rust.** This gives one definition, at the cost
  of a codegen step before selection. Codegen itself is one of the suites the
  selector orders.
- **Move the runner only.** The 14 lane, lock and stop fixes were races between
  real processes. The language does not remove them.
