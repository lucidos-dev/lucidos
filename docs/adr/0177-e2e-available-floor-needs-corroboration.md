# 0177: The e2e available-memory floor is a corroborated stop, and the pre-flight gate matches it

- **Status**: Accepted (successor to
  [ADR 0176](0176-available-floor-measures-sustained-scarcity.md))
- **Date**: 2026-09-09

## Context

ADR 0176 gave the available floor a sustain rule. The floor kept stopping
healthy nightlies, and `mobile-webkit` went four consecutive nights without a
full verdict. The last of them stopped at available 9.05 GB against the 9.60 GB
floor, with kernel pressure `normal` and swap 0.00 GB. Nothing else about the
host was wrong.

**This is the same failure as the retired compressor cap.** ADR 0175 retired a
flat compressor ceiling because the compressor measures an artifact of macOS
rather than danger. The floor has now inherited that defect through the same
pool.

`available` is `free + speculative + purgeable + file-backed`. On this host it
is depressed by the idle compressed-page pool. Each fresh browser squeezes cold
pages host-wide, and macOS never proactively decompresses. So the pool sits
there overnight, and `available` reads low on a healthy host.

Three measurements settle it.

- **An idle host reads under the floor.** The 7-day host read available 8.79 GB
  at 09:00 with nothing running. The guard would refuse to start a run on a
  completely idle Mac. A threshold a quiet idle host cannot clear is not
  measuring scarcity.
- **The suite gives back what it takes.** The five cheap projects cost 1.12 GB
  of available and returned all of it, ending 0.90 GB above their own baseline.
- **The run's own footprint is flat.** Summed `phys_footprint` over one whole
  nightly went 7.41 GB to 7.99 GB across 37 boundaries.

**The disproof closed the loop after this was decided.** The 55 specs the last
false stop cut were discharged on the same commit that had stopped twice, with
`LUCIDOS_E2E_WEBKIT_CHUNKS=16-34`: 199 passed, 0 failed, 0 flaky, 9 skipped,
exit 0, 19 minutes. The lowest available across all 23 boundaries was 12.86 GB
against the 9.60 GB floor. Peak compressor was 2.36 GB, and pressure read
`normal` with swap 0.00 everywhere. Host load peaked at 1.46x of 18 cores, with
0 WebKit reaps. So the range is not intrinsically heavy, and the floor was the
only thing that ever stopped it.

**What pressure does say.** The one recorded freeze read free 0.04 GB, pressure
`critical`, compressor 17.41 GB. A healthy nightly stop read compressor
17.11 GB, free 4.16 GB, pressure `normal`. Pressure separates the two, and it is
no longer inert. A discharge attempt stopped on `critical` at 8.63 GB available,
so the kernel does raise the level when headroom genuinely collapses.

## Decision

A low available reading is necessary and never sufficient. The floor stops only
when it is sustained AND an independent signal agrees the host is in trouble.
This revises decision 6 of ADR 0176, which left the floor a stop on its own.

1. **The corroborating signals are the kernel's pressure level at `warn` or
   worse, or any swap in use.** Both measure the host rather than the pool.
2. **Corroboration carries the same sustain shape as the floor.** Pressure
   corroborates at the boundary instant, or in at least two window samples.
   Swap needs no sustain: it folds with `max` and is accumulated state.
3. **The ADR 0176 sustain rule stays, unweakened.** A stop needs sustain AND
   corroboration, and one dip under `warn` is still not a stop.
4. **The floor VALUE does not change.** It stays `max(8 GB, 20% of RAM)`, with
   the same two knobs.
5. **`warn` alone still never stops a run**, per ADR 0175 decision 4. What is
   new is the conjunction, which is far rarer than either half.
6. **Critical pressure, swap over its limit and the runaway backstop are
   unchanged**, and all three still stop on their own.
7. **The floor fails open when it cannot corroborate.** An unreadable pressure
   oid corroborates nothing, and the boundary line says so.
8. **The pre-flight gate gets the same correction**, so the two never disagree.
   Its `AVAILABLE_MIN_GB` line becomes corroborated on the same two signals. Its
   unconditional `COMPRESSOR_MAX_GB=8` is retired for a runaway backstop at 50%
   of RAM, mirroring the running guard.
9. **The stop renames itself `CORROBORATED SCARCITY`.** The claim that a bare
   available number is real scarcity is exactly what the evidence falsified.

## Rationale

**A reading an idle host produces cannot be a stop condition.** That is the
whole argument, and it is the same one ADR 0175 made about the compressor. Both
instruments read the compressed-page pool, and the pool is a record of past
peaks rather than present danger. A guard built on either alone stops the run
for being warm.

**Corroboration is cheaper than recalibration.** The floor value was never
wrong. Lowering it would move the artifact rather than remove it: the idle pool
grows without bound overnight, so any fixed number is eventually under-read.
Raising it stops more healthy runs. Requiring a second instrument changes what
the stop MEANS instead of where it sits.

**The two corroborators measure the host, not the run.** Kernel pressure is the
verdict that separated the one recorded freeze from every healthy night. Swap in
use means compression stopped keeping up. Neither can be produced by a browser
squeezing cold pages, which is precisely the confound the available reading has.

**Swap corroborates from the first byte.** That is deliberately a lower bar than
the 1 GB ceiling that stops on its own. A host that swapped at all while also
short of headroom is not merely warm. The two readings together say more than
either does alone.

**The gate has to move with the floor, or the two disagree.** They are pinned on
the same 8 GB minimum. A gate that refuses a host the running guard would
happily continue on is a contradiction the operator has to resolve at 06:30. The
gate's flat 8 GB compressor line had the same defect for the same reason. So it
is retired in the same change, rather than left to fail on its own schedule.

## Consequences

- A healthy warm host runs to the end. The four-night `mobile-webkit` blackout
  had one cause, and this removes it.
- **The guard now has no stop that fires on this host in normal operation.**
  Pressure has never left `normal` here, and swap is structurally absent. The
  runaway backstop sits at 24 GB against a 2.36 GB peak on the discharge run. So
  the guard is effectively inert, by design: it fires on distress and on nothing
  else. Read that as the intended state, not as coverage lost.
- The risk traded for is a stop that fails to fire. Available would have to
  collapse while the kernel stays calm and nothing swaps. The recorded freeze
  does not have that shape, and ADR 0176's risk of two missed arms is now
  compounded by a second missed signal.
- Every declined stop is still logged. The boundary line keeps the window
  minimum, and a distinct note names which half was missing: sustain,
  corroboration, or both.
- The fold's line has eight fields rather than seven, gaining a warn-or-worse
  count beside the critical one.
- The pre-flight gate and the running guard are now verified against each other
  by a test that replays the idle-host reading through both. The gate is knowhow
  rather than a repo file, so that test locates it in the ops workspace. It
  reports a loud skip when the gate is not on the machine.

## Alternatives considered

**Lower the floor.** Rejected, and this is the second time. ADR 0176 rejected it
as treating the symptom of a growing window. It fails here for a stronger
reason: the idle pool sets no bound on how low `available` reads on a healthy
host, so no fixed number is safe.

**Drop the floor entirely and rely on kernel pressure.** Rejected. Pressure is
inert on this host today. A guard with no headroom stop would then have nothing
to say about a host whose `available` genuinely collapses under a calm kernel.
The corroborated floor keeps that case covered, at the cost of needing
agreement.

**Stop on `warn` pressure alone.** Rejected, and ADR 0175's 5749-sample
distribution is why: 92 occurrences with no freeze, and it tracks host load as
much as memory. Stopping on it trades one false positive for another.

**Add an unconditional deep-scarcity arm below the floor**, say a stop at 2 GB
whatever the other signals say. Rejected. It would be an invented number on the
instrument this change just demoted. An idle host with a large enough pool
reaches any such number eventually.

**Correct the running guard and leave the pre-flight gate alone.** Rejected. The
gate would then refuse to start runs the guard would happily finish, which is
the disagreement both files were pinned together to prevent. It also leaves a
second copy of the retired reasoning in the tree. The next reader would find one
of the two and believe it.
