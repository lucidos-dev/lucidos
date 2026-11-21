# 0737: A launch build holds one build-slot lease from compile through codegen

- **Status**: Accepted
- **Date**: 2026-11-18
- **Amends**: [0070](0070-engine-owned-build-slot.md), [0210](0210-build-slot-governs-cpu-share.md), [0304](0304-apply-build-has-build-slot-priority.md)

## Context

A launch build compiles, runs preflight, publishes and runs codegen in one shell.
Previously, each wrapped step acquired a separate *build slot*.
The build released the slot between steps, including during preflight.

On a host with one slot, a follow-main version build compiled for 19 minutes.
It released the slot for preflight. Within seconds a hardening session took it for a 45-minute test run.
The codegen step then waited behind that run as a priority waiter.
The user saw "New version queued" for 21 minutes and counting after the compile had finished.

Priority (ADR 0304) did not help, because nobody was waiting when the slot freed.
Priority reorders waiters. It never stops a holder.

The codegen step also compiled a second time. It ran `cargo test -p lucidos-cli` to write the CLI surface.
That is a test-profile compile of the CLI and its dependencies, beside a release `lucidos` binary just built.

## Decision

1. **A launch build holds one slot for its whole body**, as a *build-slot lease*.
   The body is everything `run_launch_build` runs: compile, preflight, publish, codegen and the after hook.
2. **A helper process holds the lease, never the build shell.**
   `lucidos build-slot --lease` acquires like any wrapped build, reports its grant, then holds.
   It releases when the shell asks, when the shell dies, or when it dies itself.
3. **Steps inside the lease pass straight through**, on the existing `LUCIDOS_BUILD_SLOT_HELD` path.
   Each wrapped step applies the lease’s nice increment once to its own process tree.
4. **One memory gate and one class per lease.** The gate runs once, before the lease is asked for.
   The class comes from the caller's environment, as today, so a version build leases as a priority waiter.
5. **Codegen renders the CLI surface with the CLI just built** when one sits beside `lucidos-codegen`.
   Only a render with no usable built CLI compiles the writer test, as today.

## Rationale

The build holds the slot between steps, so `--status` shows it throughout.
The engine sees a holder in the build’s process group and clears the queued state.

**The helper pattern already exists.** `launch_tree_locked` holds the launch tree lock in a helper.
That helper releases on a line or on parent death. The lease copies that contract into the broker.

**The kernel still releases everything.** The lease is a flock in the pool, held by a live process.
A group kill reaches it, because it stays in the build's process group (ADR 0070).
A shell killed alone is noticed within a second, through `getppid`. Nothing is ticketed or reclaimed.

**Inside the tree lock, so lock order does not change.** Today a build takes the launch tree lock, then a slot.
The lease is taken inside the tree lock too. The engine's checkout build lock still comes first.

**The nice belongs to the steps, not the shell.** Nicing the build shell would leak into later work.
`web-dev.sh -b` starts the gateway and engine from that same shell, after the build.

**The CLI binary carries its own surface.** `Cli::command()` is in the release binary already.
Asking the binary costs milliseconds and no slot. The output is the same JSON either way.

## Consequences

- A slot stays held while preflight restores a database copy and boots the engine.
  Other builds wait for preflight to finish.
- On a one-slot host, a started launch build finishes before a competing build acquires the slot.
- Multi-slot hosts also hold one slot for the full build. Other builds can use the remaining slots.
- The core share is decided once, at the lease grant, for every step.
- A lease helper killed alone frees the slot while later steps run unslotted.
  That is the same narrow case ADR 0070 accepts for a wrapper killed alone.
- `--status` shows the lease's label for the whole body, not the current step.
- A broker that predates `--lease` makes the build fall back to per-step slots, exactly as before.
- Codegen in a launch build no longer compiles a test-profile CLI.

## Amendments

- **ADR 0070** said the build process holds the slot. For a launch build, a lease helper beside the build holds it.
- **ADR 0210** said a nested acquisition imposes nothing, because the outer one niced the tree.
  Under a lease the outer one niced nothing, so each nested step applies the lease's nice to its own tree.
- **ADR 0304** made the Apply build a priority waiter. It still waits as one, once, for the whole pipeline.

## Alternatives considered

- **Keep a priority claim up across the gaps.** Ordinary waiters would yield while the slot sat free.
  Rejected: it helps only the priority class, `--status` would show a free slot nobody may take, and the queued probe would misread it.
- **Run the whole body as a child of one wrapper.** The body is shell functions sharing many globals.
  Re-entering them in a child bash would silently drop any global nobody exported.
- **Wrap `web-dev.sh --engine-build` from the engine.** The slot would come before the launch tree lock.
  A human `web-dev.sh -b` takes the tree lock, then the slot, so a one-slot host could deadlock.
- **Nice the build shell once, at the lease grant.** Simplest. Rejected: the shell later launches the engine and gateway, which would run niced.
- **Preempt the agent build.** Rejected for ADR 0304's reasons: it throws away paid work.
