# 0408: Engine builds prune stale debug-object generations from target deps; unpacked split-debuginfo stays

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

Apply's engine rebuild in the main checkout took 7 to 13 minutes. The same
edit compiled in seconds in a coding-agent worktree. A spindump of a live
Apply build found the cause.

- Every main-checkout rustc slept in `dlopen` of a proc-macro dylib from
  `target/debug/deps`.
- The kernel held it in `AppleSystemPolicy::checkLibraryLoad`, waiting for
  `syspolicyd`. That is Gatekeeper library validation.
- `syspolicyd` builds a CFBundle for the dylib, which calls `readdir` across
  the dylib's whole parent directory.
- That directory held 761,043 entries, 750,072 of them `.rcgu.o` files. A bare
  `ls -f` took 32 s. Endpoint Security software hooks each `readdir`, which
  slows it further.

Cargo's macOS dev profile uses `split-debuginfo = "unpacked"`. A binary keeps
its debug info in its object files and names them in `OSO` stabs. Each build of
a binary or test target writes a new *debug object generation*, and nothing
deletes the old one. Worktrees stay fast only because their `target/` is young.

Measurements and the full capture:
[`docs/plans/2026-10-09-engine-build-prunes-stale-debug-objects.md`](../plans/2026-10-09-engine-build-prunes-stale-debug-objects.md).

## Decision

`build_or_find_engine` runs `prune_stale_debug_objects` on the profile's `deps`
directory between the compile and the publish. Per artifact it keeps two
generations: the one the artifact's binary links, read from its `OSO` stabs,
and the newest other one by mtime. It skips the whole prune while another
process holds the profile's `.cargo-lock`. It touches only generation-shaped
`.rcgu.o` files, and it never fails the build.

## Rationale

- **It removes the cause, not the symptom.** The stall scales with the
  directory, and the directory only grew. A bounded directory makes the
  Gatekeeper check cheap again.
- **The binary names its generation; mtime cannot.** Rustc hardlinks reused
  codegen units into the new generation, so generations can share inodes and
  mtimes. Ranking by mtime alone could keep an old generation and drop the
  linked one. The binary is read only for an artifact with more than two
  generations, so an Apply build reads three binaries, a few seconds.
- **Two generations, not one.** The engine still running from the previous
  build reads its own objects for backtrace line numbers. The second slot
  usually holds its generation until the user switches.
- **Defer to a running cargo, checked on its lock.** A human `cargo test` in
  the same checkout may be linking a generation the prune would drop. Cargo
  holds `.cargo-lock` open for a whole build, so `lsof` on it answers exactly.
  The check runs before listing and again before deleting. ADR 0294 already
  rules that an engine build never interferes with another cargo.
- **Never fatal.** A failed engine build aborts every co-located workspace's
  rebuild. Losing a prune costs disk and speed, never correctness.

## Consequences

- The first build after this lands deletes the whole backlog, which takes a few
  minutes once.
- A binary built more than two generations ago loses line numbers in its
  backtraces. Only an engine left unswitched across two later builds is one.
  When generations tie on mtime, the second slot is arbitrary, so the running
  engine can lose them one build early.
- This is a temporary measure, in `docs/temporary-measures.md`. It ends when
  cargo or rustc deletes superseded generations itself.

## Alternatives considered

- **`split-debuginfo = "packed"`.** Rejected: it runs `dsymutil` on every link
  of a 270 MB binary, adding seconds to every build.
- **`split-debuginfo = "off"`.** Rejected: the engine's backtraces lose file
  and line, which every panic report relies on.
- **A one-hour age floor on ctime.** Shipped first, then replaced. A build
  that reuses every codegen unit hardlinks the same objects again, which sets
  the ctime of every name sharing them. Old generations then never aged while
  Applies came less than an hour apart.
- **Rank generations by mtime alone.** Rejected after review: hardlinked
  reused objects tie, and the main checkout held an artifact with four
  generations at one mtime.
- **Read every binary's `OSO` stabs on every build.** Rejected: about 2 s per
  binary, and the main checkout held over a hundred. Reading only crowded
  artifacts gives the same answer.
- **Ask users to exclude `target/` from Gatekeeper or Endpoint Security.**
  Rejected: that is machine policy Lucidos does not own, and managed machines
  often cannot change it.
- **`cargo clean` on a schedule.** Rejected: it throws away the warm cache, so
  the next build is cold, at 5 minutes or more.
