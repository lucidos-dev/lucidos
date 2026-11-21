# 0738: Keep engine crates small and wide

- **Status**: Accepted, amended 2026-11-20
- **Date**: 2026-11-19

**Amendment (2026-11-20).** The ceiling was shrink-only at first. That froze
each oversized crate at its size when the ratchet landed. Ordinary feature
work then hit the gate before any split was ready. The owner chose 10%
headroom over a tiny extraction crate made only to fit the number.

## Context

Phases 6 and 7 formed a stack of three engine libraries. Their largest
targets range from 244,000 to 328,000 lines. The original Phase 7 runtime
probe shifted async function spans and inflated the Mac test rebuild.
The [corrected Mac benchmark](../plans/2026-11-19-engine-build-speed-results.md#mac-runtime-edit-rebuild-corrected-probe)
preserves those spans. Driver edits still recompile the API test target
after the orchestration library. The runtime cut removes that rebuild
for driver-only edits.

## Decision

- Engine crates sit in numbered dependency tiers. Cargo dependencies point
  down, including build and dev dependencies. The checked
  [tier map](../../crates/crate-tiers.tsv) records membership.
- Hot providers, coding-agent drivers, tools, voice and API handler
  groups go in leaf crates. Only the engine executable can depend on a leaf.
  The executable wires their implementations into lower-tier interfaces.
  The boot leaf constructs the engine. The kernel holds its shared types.
- The default cap is 50,000 physical Rust lines per compilation target,
  with separate counts for libraries, binaries and integration tests.
  Existing oversized crates have a line ceiling in
  [the ceiling file](../../crates/crate-size-ceilings.tsv). It may rise to
  at most 10% above the crate's largest target, so feature work goes on
  until a planned split. `--reset` sets it to the current size plus 10%, or
  removes the row once the crate fits the cap.
- A crate's dependents are its transitive dependents. A dev edge stops at
  the crate that owns the tests. An existing crate may not gain a
  dependent that already exists on `main`. A crate extracted in the same
  change is exempt, so splits stay possible. A crate whose manifest path
  exists on `main` is not extracted, even under a new name.
- A crate's blast radius sums Rust lines across its dependents' targets,
  test targets included. The graph report shows it, and new crates start
  at or below 150,000 dependent lines. Existing radii are not ratcheted:
  any line added to a dependent would raise them, so a line ratchet fails
  ordinary feature work.
- The longest dependency path starts at six edges and can only get
  shorter. The [graph baseline](../../crates/crate-graph-baseline.tsv)
  records depth and each crate's dependents. All graph and size
  guardrails run during hardening.
- Build slot capacity follows measurements on the host. The test build,
  Clippy and local-release landing replay must fit the per-slot memory
  budget after reserving memory for the OS and services.
- The layout report uses recent Git activity to flag busy crates with many
  dependents. It informs the review of each split.

## Rationale

Independent crates can compile in parallel. Moving frequently edited
implementations upward limits the rebuild after a change. The largest rustc
target matters for memory, and adding parallel compiler processes can raise
total RSS even when each one is smaller.

## Consequences

Each crate extraction lands on its own and passes the benchmark gate in the
[Phase 8 plan](../plans/2026-11-19-engine-crate-stack-split.md).
Crates with upward references need shared types moved down or lower-tier
traits implemented above. A change to the default slot formula requires
revisiting [ADR 0070](0070-engine-owned-build-slot.md).

## Alternatives considered

- Continue building the stack. This leaves cores idle and gives hot modules
  many dependents.
- Create a crate for every module. Each crate adds compiler and test link
  overhead.
- Gate the cap in CI. Hardening is the repository's per-change gate
  ([ADR 0386](0386-e2e-defaults-to-github.md) and [ADR 0464](0464-main-check-on-github.md)).
- Gate churn. The set of recent commits changes without any code change.
