# 0594: /harden selects clippy and tests per reached crate from the cargo dependency graph, and splits the rust suite; full-tree paths and graph failures select every crate.

- **Status**: Accepted
- **Date**: 2026-10-28
- **Amends**: [0292](0292-harden-early-suite-run.md) and
  [0592](0592-harden-selects-tests-not-crates.md). Both selected one `rust`
  suite for any Rust-relevant path. That suite is now split by crate.

## Context

`/harden` Phase 4.5 ran `make lint && make test` for any `.rs`, `Cargo.toml`,
`Cargo.lock` or `.sql` change, and for any production compile input. A cold
run took about 14 minutes: two workspace-wide clippy passes, then every
crate's tests. A gateway edit paid for the engine compile.

Plan: `docs/plans/2026-10-28-harden-rust-selection-per-crate.md`.

## Decision

1. **The crate graph comes from `cargo metadata`.** `start` records each
   member's directory and path dependencies once, in `$STATE/crate-graph`.
   It records compile owners in `$STATE/compile-owners`. No crate list is kept
   by hand beyond the six crates that own a dedicated suite.
2. **A path maps to its owning crates, and the reached set is their reverse
   closure.** A production compile input belongs to every crate that compiles it.
3. **The `rust` suite splits.** The suites are `lint`, `crates`, `gateway`,
   `cli`, `app-lib`, `eval`, `engine` and `engine-filtered`. They run in that
   order in the one cargo lane. `lint` runs clippy on the reached set through
   `CLIPPY_SCOPE`, and both feature passes still run.
4. **These select every crate:** `Cargo.lock`, the root `Cargo.toml`,
   `.cargo/**`, `rust-toolchain.toml`, `clippy.toml`, `Makefile`,
   `crates/vendor/**`, a Rust path no crate owns, and a missing or failed
   graph.
5. **A shell-only diff skips clippy.** `shell-lint` runs the shell, eval,
   knowhow and fmt linters only.
6. **A retry record naming `rust` is dropped.** The branch gets a full run.

## Rationale

A crate's lint and test result can change only when its own source or a
dependency changes. The reverse closure is exactly that set. Reading it from
cargo means a new crate or edge needs no edit here.

Reach on the real tree:

| Edit | Reached crates |
|---|---|
| a gateway source file | `lucidos-gateway` |
| an engine source file | `lucidos-e2e`, `lucidos-engine`, `lucidos-eval` |
| a `lucidos-home` source file | 12 crates |
| `Cargo.lock` | every crate |

Both clippy passes stay, because only the default pass lints the production
push transport in `lucidos-engine`. Every fail-safe selector runs more, never
less, so a graph the script cannot read costs time and never coverage.

## Consequences

- A gateway, app or CLI edit skips the engine compile and its tests.
- An every-crate selection now also runs the `cli` and `app-lib` suites,
  which `make test` never ran.
- A feature change in one member `Cargo.toml` selects that crate's closure.
  Feature unification across unrelated crates is not modelled. A dependency
  change rewrites `Cargo.lock`, which selects everything.
- Build-script inputs outside dep-info, such as `tauri.conf.json`, stay
  unselected as before.

## Alternatives considered

- **A hand-kept crate table in the script.** It drifts when a crate or edge is
  added, and a stale row silently under-selects. Rejected for the graph.
- **Scope clippy to one feature pass.** It saves time but leaves the production
  push transport unlinted. Rejected.
- **Keep one `rust` suite and only trim its tests.** The clippy cost stays, and
  the retry and early-run records stay coarse. Rejected.
