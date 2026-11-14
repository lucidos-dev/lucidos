# 0176: The e2e available-memory floor stops on sustained scarcity, not on the minimum of a growing window

- **Status**: Accepted (successor to
  [ADR 0175](0175-e2e-memory-guard-measures-kernel-pressure.md)). Decision 6,
  leaving the floor a stop on its own and the pre-flight gate unchanged, is
  superseded by
  [ADR 0177](0177-e2e-available-floor-needs-corroboration.md). The sustain rule
  here still holds, unweakened.
- **Date**: 2026-09-08

## Context

ADR 0175 replaced the compressor cap with four stops in
`check_host_memory_at_boundary`. A full nightly on the real host shows that
three of them cannot fire here.

- **Kernel pressure critical** never happens. Every sample of the nightly read
  `normal`, including one taken with the compressor at 19.15 GB.
- **Swap over 1 GB** is structurally absent since macOS 26.5.2. Every watchdog
  sample reads 0.00 GB.
- **The runaway backstop** is 50% of RAM, so 24 GB on this machine. Last night
  the compressor peaked at 19.15 GB.

So the available floor is the only live stop, and every run necessarily ends on
it. It then ended one on a sampling artifact.

**The evidence.** Step 5 of the 2026-09-08 nightly, project `mobile-webkit`,
stopped at `available memory 9.45 GB is under the 9.60 GB floor`. That is
0.15 GB and 1.5% under. Pressure was `normal` on every sample all night and swap
was 0.00.

The boundary at nav chunk 4 of 34 read 13.48 GB available. The 16 specs
discharged individually afterwards ran at 11.51 and 11.79 GB. An idle-host probe
the same morning read a median of 18.41 GB over 45 samples, minimum 17.87.
Nothing about the host was scarce.

Two structural problems produced that stop, both in the fold-and-judge block.

**The window folded available with MIN.** Every other dimension folds with
`max`. For available, the minimum over N samples is a decreasing function of N.
The sampler ticks every 5 s and a chunk contributes roughly 50 to 120 samples.
So the boundary judged the deepest instantaneous trough of the whole window.
Launching a fresh browser per chunk produces exactly such troughs, as normal
operation.

**The floor had no sustain rule.** The critical-pressure stop directly above it
does, and its comment says why. One 5-second blip is not the freeze signature.
The window is sampled every 5 s, and the evidence behind that stop comes from a
ten-minute host series.

So one isolated sample sits below the resolution anything was judged at. That
reasoning applies to available memory identically. It was never applied there.

## Decision

The available floor measures sustained scarcity, symmetric with the
critical-pressure stop above it. This revises decision 5 of ADR 0175, which left
the floor alone.

1. The boundary captures `avail_now` before the fold overwrites it, exactly as
   it already captures the pressure level and the compressor.
2. The floor stop needs one of two arms: still under the floor at the boundary
   instant, or under the floor in at least two samples of the window.
3. `_host_mem_window_worst` takes the floor and counts under-floor samples,
   beside the critical count it already returns. Its line gains a field.
4. The floor VALUE does not change. It stays `max(8 GB, 20% of RAM)`, with
   `LUCIDOS_E2E_FREE_FLOOR_MIN_GB` and `_PCT` as before.
5. The boundary line still prints the window minimum. A dip the run survived
   adds a note naming how many samples were under the floor.
6. The swap stop, the runaway backstop and the pre-flight gate are unchanged.
   **[Superseded by ADR 0177:** a sustained sub-floor reading also needs
   corroboration, from the kernel at `warn` or worse or from any swap in use.
   The gate gains the same rule and retires its flat compressor ceiling. The
   swap stop and the runaway backstop are genuinely unchanged.**]**

## Rationale

**The minimum of a noisy series over a growing window is the wrong statistic.**
ADR 0175 established that the compressor was the wrong instrument. This is the
same class of error, one level down: the instrument is right and the statistic
is wrong. A trough deepens with the number of samples taken. So the stop grew
stricter the longer a chunk ran, while the host itself did not change.

**A sustain rule is the fix, and a different floor is not.** The value was never
the problem. Lowering it moves the artifact instead of removing it, because a
longer window finds a deeper trough at any threshold. Raising it stops more
healthy runs. So the number stays, and the pre-flight gate keeps agreeing with
it.

**Two arms, because they answer different questions.** The boundary instant
answers whether the host is low right now. The sample count answers whether it
was low for longer than one tick. A genuinely scarce host satisfies both. A
browser launch satisfies neither.

**Swap keeps folding with MAX, and needs no sustain rule.** The direction is
safe: a longer window can only find a higher reading. Swap in use is accumulated
state rather than an instantaneous level. A sample over the limit means the host
really did swap that much out, and a later sample does not undo it.

**The runaway backstop keeps its single reading.** It is a net for a host doing
something nobody modelled, at 24 GB against a 19.15 GB peak. One reading over it
is worth stopping on.

## Consequences

- The floor stops on scarcity rather than on window length, so a run is no
  longer certain to end on it.
- **Three of the four stops are inert on this host, so the fourth carries the
  whole guard.** Any future defect in the floor is a total guard failure, not a
  partial one. Read a change to the floor as a change to the entire stop
  condition.
- Observability shows the same numbers and explains more. The line keeps the
  window minimum, and a survived dip states its sample count.
- The fold's line has seven fields rather than six. It is internal to this
  library, and the three tests matching it exactly were updated with it.
- The risk traded for is a stop that fails to fire. Both arms have to miss, on a
  host where pressure and swap say nothing.

## Alternatives considered

**Fold available with `max`, or with a median.** Rejected. `max` would judge the
healthiest moment of the chunk, discarding the peak-awareness ADR 0175 added. A
median hides a real sustained dip inside a mostly healthy window. The sustain
count keeps the peak visible and judges its shape instead.

**Lower the floor.** The stop was 1.5% under, so a small reduction would have
carried this run. Rejected as treating the symptom. The trough deepens with
window length at any floor, so the next longer chunk reopens the question.

**Require three or more under-floor samples.** Rejected as unearned precision.
Two is the threshold the pressure rule already uses. Matching it is worth more
than a number tuned to one night.

**Sample less often, so a window holds fewer troughs.** Rejected. It removes the
artifact by measuring less, and the same samples feed the peak-aware compressor
and pressure folds.

**Drop the floor and rely on kernel pressure.** Rejected. Pressure is inert
here, so the guard would have no live stop at all. The floor is also the
threshold the pre-flight gate mirrors, and the two must agree.
