# 0351: Heavy builds wait on a host-memory gate, and the e2e harness resumes after its own memory stops

- **Status**: Accepted (builds on [ADR 0175](0175-e2e-memory-guard-measures-kernel-pressure.md),
  [ADR 0177](0177-e2e-available-floor-needs-corroboration.md) and
  [ADR 0182](0182-instantaneous-critical-pressure-is-not-distress.md),
  whose thresholds and stop rules all stand. Narrows
  [ADR 0070](0070-engine-owned-build-slot.md)'s "a limiter never stops a build"
  to the slot alone.)
- **Date**: 2026-10-03

## Context

The nightly e2e ended INCOMPLETE on most nights from 2026-09-26. Every night,
mobile-webkit met a memory stop, and the run exited 71. A coding-agent child
then improvised a second leg in the small hours. The green nights came from
that leg. The incomplete ones came from that leg going wrong:

- a retry without the phase selector, which paid for the whole CC phase again
  and collapsed the host inside its first chunk;
- retries refused by the host-load guard after a teardown storm;
- a host that stayed short of memory until the morning deadline;
- on 2026-10-03, `/harden`'s cargo suites and a release engine build started
  while the pre-flight gate read NO-GO. The Mac kernel-panicked.

The in-chunk stop had fired correctly that night. No chunk was running at the
panic. The load came from cargo, and no memory gate covered a build.

Two more facts shaped the decision. Last night's attribution showed the run
leaks nothing it owns: the e2e engine held 1.03 to 1.05 GB over 51
boundaries, and coding-agent subprocesses read 0 at all but one. And the
e2e workspace was never reset between runs, so its tree held 1,025 apps,
1,842 artifacts and 27,341 commits.

Plan: `docs/plans/2026-10-03-nightly-e2e-finishes-in-one-night.md`.

## Decision

1. **Every heavy build waits on a build memory gate.** It is the pre-flight
   gate's rule, applied by `host_memory_gate_verdict` and run by
   `scripts/build-memory-gate.sh`. `with-build-slot.sh` runs it before it asks
   for a slot, and `harden-suites.sh` before each cargo suite. On NO-GO it
   waits up to 15 minutes, then exits **72**. It is GO on a host it cannot read.
2. **The e2e harness owns its recovery.** A memory stop can land at a chunk
   boundary, inside a chunk or at a project boundary. Each one tears down,
   waits for the host to recover, restarts the engine on a fresh database, and
   carries on. The run reports one verdict. A run whose host never recovers
   exits 71 and names the exact chunks with no verdict.
3. **Each chunk has a 20-minute wall-clock ceiling.** A chunk past it is a
   failure. One trip restarts the engine, and a second ends the project.
4. **Every run starts on a fresh workspace tree.** Only `.lucidos/` survives.
   The engine bootstraps the rest on its next boot.

## Rationale

**The gate keeps the pre-flight gate's rule exactly.** A build gate with its
own thresholds would be a fifth instrument to tune. The four before it all
false-stopped on the same compressed-page pool. `host_memory_guard_test.sh`
replays eight readings through both and asserts one verdict.

**Why the gate sits in the slot wrapper and the suite runner.** Every engine
build passes `run_engine_cargo_build`, which calls the slot wrapper: the e2e
build, a `web-dev.sh -b` and the engine's Apply rebuild. The harden suites add
the bare `cargo test` calls the slot never sees. Two call sites cover every
heavy build in the repo, and a build waiting on memory holds no slot.

**Why a refusal is a wait and then an exit, not an instant exit.** A host
that reads NO-GO after a teardown often recovers within minutes. The nights
that failed were the ones that gave up or launched anyway. A bounded wait
takes the recovery when it comes and still refuses a host that stays sick.

**Why the harness, not the child, recovers.** The child worked at 05:00 with
nobody to ask. It had to know the right flags, read two gates, wait out a load
burst and compute a chunk range. It got one of those wrong most nights. The
harness already holds the chunk list, the samplers and the teardown. So it
cannot pass the wrong range or re-run the CC phase by accident: the loop simply
continues where it stopped.

**What "recovered" means.** Three readings in a row, 30 s apart. Each must
pass the running guard's own boundary rule and the host-load cap, with no
jetsam report in the last five minutes. The boundary rule is used rather than
the build gate's 8 GB line, so a resumed run does not stop again at its first
boundary. The jetsam test is the 2026-09-24 lesson: a GO minutes after jetsam
fired is not a cold host. Each wait is bounded, and a run spends at most three
resumes.

**Why the ceiling is a failure.** A chunk that hung has no verdict, and a
missing verdict must never read as green. Real chunks take a median of 25 s and
at most about 4 minutes, so 20 minutes only ever meets a stuck one.

**Why reset the tree.** The database was already rebuilt each run because a
long-lived one hid bugs. The tree is the same state on disk, read by every
engine boot and every WebKit page, and no test owned what earlier runs left.
Every first run on a new machine already starts on an empty workspace, so the
suite must pass there.

## Consequences

- A cargo build on this host can wait up to 15 minutes before it starts. An
  Apply rebuild refused here shows its ERROR line in the build-failed toast.
- `/harden` reports a gate-refused suite as `VOID`, which asks for a rerun,
  never as `FAIL`.
- The nightly's Step 5 runs the suite once and reads one verdict. The knowhow
  forbids a second leg, a discharge or any build after a stop.
- A stop now costs a wait. The worst case is three waits of 30 minutes, which
  `LUCIDOS_E2E_RESUME_BY` caps at a local time.
- After a resume the database is fresh, as it already is between projects. A
  spec that needed state from an earlier chunk would surface there.
- Each boundary prints a left-behind line, and a coding-agent subprocess alive
  at two boundaries in a row is named `LEFTOVER`. It is reported, not killed.

## Alternatives considered

- **Keep exit 71 and teach the child better.** The knowhow has grown that
  procedure for a month, and the child still got it wrong most nights. A rule
  nobody can follow at 05:00 is the harness's job.
- **A separate memory threshold for builds.** Rejected for the reason above:
  every instrument tuned on its own drifted from the others and false-stopped.
- **Gate only the e2e build.** The 2026-10-03 panic came from `/harden`'s
  suites as much as from the e2e build. A gate that covers one of them leaves
  the other open.
- **Kill coding-agent subprocesses at every boundary.** None leaked in the
  measured run, and killing an engine child mid-run puts a failure event into
  the next chunk's page. Reporting `LEFTOVER` makes a regression visible
  without that cost.
- **Resume without restarting the engine.** Cheaper, but the stop happened on
  a host in trouble, and the engine and its children are what a teardown can
  give back. A fresh database also matches what every project already gets.
- **Re-run the whole project after a stop.** Rejected: it pays for the CC
  phase again, which is exactly how the 2026-10-03 retry collapsed the host.
