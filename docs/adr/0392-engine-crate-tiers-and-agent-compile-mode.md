# 0392: Engine builds: cold path first, then a tiered crate split

- **Status**: Accepted
- **Date**: 2026-10-08

## Context

`crates/lucidos-engine` is one crate of about 558k lines. Engine builds took
2.5 to 13 minutes, and every Apply, agent test run and lint paid that.

Measurement showed a different cost from the one assumed. A one-file edit
costs 19 s for `cargo build --lib`, 35 s for `cargo test --lib --no-run`, and
about 62 s for `make lint`. The minutes come from cold builds. Every fresh
worktree rebuilds vendored OpenSSL from C, which takes 137 s on the critical
path, and then the engine crate, which takes 102 s.

The module graph is one strongly connected component of 97 units and about
375k lines. It is held by a few hundred upward references. The two most
referenced modules, `engine::thread_events` and `engine::event_bus`, sit high
in the tree but belong at the bottom.

Plan, measurements and graph:
[`docs/plans/2026-10-08-engine-crate-split-and-cold-builds.md`](../plans/2026-10-08-engine-crate-split-and-cold-builds.md).

## Decision

1. **Cold builds first.** Lucidos-source agent builds get a *compile mode*,
   pinned per worktree `target/`. It links a shared prebuilt OpenSSL, and it
   trials rustc's parallel frontend (`-Z threads=8`).
2. **Then a tiered split, top-down.** `lucidos-engine-bin` → `lucidos-api` →
   `lucidos-engine` → `lucidos-core`. Subsystem crates above the engine come
   after.
3. **A cycle breaks by moving the item down.** If it truly needs the upper
   tier, the lower tier defines a trait, and the binary's composition root
   injects it. No event exists only to break a compile cycle.
4. **Every phase passes a benchmark gate.** Its target metric improves by at
   least 20%, and no other metric gets more than 10% worse. A failed gate
   stops the work for a new design pass.

## Rationale

- **The cold path is where the minutes are.** OpenSSL alone costs more than an
  incremental engine build, test build and lint together.
- **The compile mode lives in the agent compile env, pinned per `target/`.**
  Any change to rustc flags or OpenSSL source rebuilds the whole tree. One
  place, read the same way by a spawn and a background task, means a worktree
  never alternates.
- **Only Lucidos-source sessions get it.** The agent compile env reaches every
  coding agent. `OPENSSL_NO_VENDOR` or `RUSTC_BOOTSTRAP` in someone else's
  repo would break or change their build.
- **Apply and release stay on stable flags and vendored OpenSSL.** The
  shipped binary must not depend on an unstable compiler mode. The main
  checkout's target is warm, so OpenSSL costs it nothing there.
- **Top-down, because `api` is mostly cut already.** Its 108 inbound
  references are domain logic parked under `api/`, which belongs in the
  engine tier anyway. The foundation cut is the hardest, because event types,
  EventBus, `core` and `llm` are bound together.
- **The binary crate owns the build identity.** `build.rs` reruns on every
  HEAD move. An identity value compiled into a lower crate would recompile
  every tier on each commit.
- **A gate, because a split can move code without speeding anything up.** An
  edit in a crate recompiles every crate above it. Code low in the graph can
  get slower to edit, so each step must prove itself.

## Consequences

- Tier names are fixed: `lucidos-core`, `lucidos-engine` (the agent machinery,
  keeping the library name), `lucidos-api`, and `lucidos-engine-bin`, which
  builds the `lucidos-engine` binary. The binary's name and path do not change.
- Each new crate must be taught to restart detection, harden suite selection,
  `test-engine.sh`, and the rule `paths:` globs.
- Items crossing a crate boundary become `pub`, which widens what each crate
  exposes.
- A worktree built before the compile mode keeps its legacy mode until its
  `target/` is stripped.
- The `-Z threads` trial is a temporary measure, removed when the flag
  stabilises or a hang or ICE is traced to it.

## Alternatives considered

- **Split only, for incremental cost.** Rejected. It leaves the 137 s OpenSSL
  build on every cold build. The best case also saves only 10 to 25 s per edit
  in the most-edited modules, which sit mid-graph.
- **Cold path only, no split.** Rejected. Parallel frontends across crates and
  separately compiled test targets only come from a split.
- **Remove OpenSSL from the tree.** Deferred. git2's SSH transport needs a
  crypto backend, so it is a project of its own.
- **sccache path normalisation for OpenSSL and our crates.** Rejected on
  evidence. A non-incremental workspace crate built from a second checkout
  path missed the cache, even with `SCCACHE_BASEDIRS` set.
- **`-Z threads` everywhere, including release.** Rejected. It would put an
  unstable compiler mode on the shipped binary.
- **A fine-grained split up front.** Rejected. It inverts several hundred
  references before anything is measured.
- **Bottom-up, foundation first.** Rejected. It is the hardest cut, and it
  gives the smallest early win.
- **Break cycles with events.** Rejected. It turns synchronous calls into
  asynchronous ones, and it adds event variants that record no state change.
- **Engine-prefixed names** (`lucidos-engine-core` and so on). Rejected. They
  would rename every `lucidos_engine::` path in the tree and the scripts.
