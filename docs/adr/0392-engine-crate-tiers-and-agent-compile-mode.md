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

## Amendment: build identity first, with gateway and suite benchmarks

Decisions 1, 3 and 4 stand. This amendment replaces the order in Decision 2.

1. Move build identity into `lucidos-engine-bin` first. The build id changes on
   every commit and engine edit. Embedding it in the engine library makes
   thin-local LTO re-optimise most of the crate. On an 18-core Mac, one small
   rebuild took 193 seconds with the current stamp and 19 seconds with a
   fixed stamp.
2. Next, turn off thin-local LTO in `local-release`, consolidate clippy into
   one pass, then trial `-Z threads`. Split the library after those changes.
3. The first library split moves `api` and the most-edited agent units above
   the rest of the engine. The `api` split alone helps 8% of commits.
4. The benchmark gate includes peak RSS. A `local-release` settings change
   also measures engine runtime. The 16 GB gateway sets the memory limit.
5. Opt-level 1 did not speed up edit rebuilds. A thin binary crate that leaves
   the stamp in the library still makes those rebuilds re-optimise it.

The shipped `release` profile keeps its settings. Apply builds use stable
compiler flags.

## Amendment: the agent crate

1. **Name:** `lucidos-agents-api`. Rejected: `lucidos-agent`, which collides
   with the *Lucidos Agent* glossary term and with `lucidos-agent-sandbox`.
2. **Boundary:** one injected `AgentHost` trait, passed to the engine
   constructor. Rejected: a newtype engine handle in the agent crate, because
   it changes 336 handle mentions and needs a weak back-reference.
3. **Inherent impls:** crate-private extension traits on `LucidosEngine`.
   Rejected: free functions, because every call site changes.
4. **Module tree:** the agent crate mirrors the engine's tree through a
   re-export facade, so a move is a pure rename and live branches follow it.
5. **Tests:** lower test helpers reach the agent crate through a
   `test-support` feature, and `test-engine.sh` covers both crates.

## Amendment: the core crate

1. **Name:** `lucidos-core` (parent decision 7). It holds the event types,
   the EventBus with its projections, the stores, `llm`, the paths and the
   migrations.
2. **Boundary:** move down first. A second injected trait only for behaviour
   calls left after that. Rejected: a trait for the projections, because
   each projection write runs in the event's transaction.
3. **Migrations:** one `MIGRATOR` in `lucidos-core`, and the directory moves
   with the pool. Rejected: leaving the directory in `lucidos-engine`,
   because the lowest crate would then read files from a crate above it.
4. **Composition up:** code that constructs the engine never sits in
   `lucidos-core`.
5. **Hardening:** per-crate suite selection plus one engine boot smoke
   check. The full e2e runs on the main check, never by hand.
