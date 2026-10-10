# 0182: An instantaneous critical kernel pressure reading is not distress; the e2e stop needs sustain or corroboration

- **Status**: Accepted (narrows decision 1 of
  [ADR 0175](0175-e2e-memory-guard-measures-kernel-pressure.md), which made
  `critical` a stop on its own. Decisions 2, 3, 4 and 6 of that ADR stand, and
  so does all of [ADR 0177](0177-e2e-available-floor-needs-corroboration.md).)
- **Date**: 2026-09-12

## Context

This is the fourth instrument to false-positive on this host, and the third time
the same bug has come back in a new place.

ADR 0175 retired the compressor survivability cap, because the compressor
measures an artifact of macOS rather than danger. ADR 0176 gave the available
floor a sustain rule. ADR 0177 then made that floor a corroborated stop, for
exactly the same reason the cap was retired: an idle host reads under it.

Kernel memory pressure was the instrument all three of those leaned on. ADR 0175
called it "the kernel's own verdict" and made `critical` an unconditional
immediate stop. ADR 0177 made `warn or worse` the thing that corroborates the
available floor. Both of those readings are now measured, and one of them does
not survive.

**The measurement.** Ten samples of this host, 18 seconds apart, between 15:52
and 15:55 local on 2026-09-12. Nothing of ours was running: no e2e runner, no
Playwright, no e2e engine, no lock, Chrome closed, only the two long-running
workspace engines up.

```
15:52:08  pressure=2  available=11.08GB  compressor=17.28GB  swap=0.00M  load1=2.33
15:52:26  pressure=2  available=11.03GB  compressor=17.28GB  swap=0.00M  load1=2.17
15:52:45  pressure=4  available=10.84GB  compressor=17.28GB  swap=0.00M  load1=2.04
15:53:03  pressure=4  available=11.05GB  compressor=17.28GB  swap=0.00M  load1=1.59
15:53:21  pressure=2  available=11.09GB  compressor=17.28GB  swap=0.00M  load1=1.52
15:53:41  pressure=4  available=11.07GB  compressor=17.28GB  swap=0.00M  load1=1.39
15:53:59  pressure=4  available=11.09GB  compressor=17.28GB  swap=0.00M  load1=1.30
15:54:17  pressure=2  available=11.10GB  compressor=17.28GB  swap=0.00M  load1=1.33
15:54:36  pressure=4  available=11.00GB  compressor=17.28GB  swap=0.00M  load1=3.16
15:54:54  pressure=4  available=11.01GB  compressor=17.28GB  swap=0.00M  load1=2.55
```

`kern.memorystatus_vm_pressure_level` oscillates 2 to 4 and back inside 18
seconds. Available memory is flat at 11 GB. Swap is exactly zero. The compressor
does not move by a single page across three minutes. Load is falling. **Six of
ten samples read critical on a machine doing nothing.**

**The cost.** The same four `mobile-webkit` nav specs have had no verdict since
2026-09-11, across three consecutive incomplete nightly runs. Earlier the same
day, 22 pre-flight gate checks all returned NO-GO on kernel memory pressure.
They ran across three windows between 03:57 and 06:25. Available was 11.4 to
12.4 GB and the compressor was flat at 17.2 to 17.5 GB.

**The two instruments had already drifted apart.** The gate's threshold was
`pressure > PRESSURE_MAX`, with `PRESSURE_MAX=1`, so **warn** was a NO-GO there.
The running guard has never stopped on warn: ADR 0175 decision 2 says warn is
reported and never acted on, on a 5749-sample distribution. So the gate refused
hosts the guard would have run on, which is the disagreement the two files are
pinned together to prevent. Nothing else in the gate could have fired on those
22 checks. Available was well over its 8 GB line, and the compressor well under
its 24 GB backstop.

**What the level actually is.** A transient edge notification the kernel raises
as the compressed idle-page pool is probed. It clears on its own. That is the
same class of false positive as the retired compressor cap and the
uncorroborated available floor, and it has the same root: all three read the
idle compressed-page pool rather than the host.

**What the freeze looked like, for contrast.** The one recorded freeze, on
2026-07-26, read compressor 17.41 GB, free 0.04 GB, pressure critical. Then came
a six-hour hole in the watchdog series, and a reboot. Critical pressure was
there. So was the headroom being gone. The second half is what the trace above
does not have.

## Decision

A critical pressure reading stops the run on three grounds, and on nothing else.
Both instruments carry the same three, with the same numbers.

1. **Collapse, immediately and unconditionally.** Critical with available memory
   at or under `max(2 GB, 5% of RAM)`, 2.40 GB on this host. That is the freeze
   signature, and it never waits.
2. **Swap, immediately.** Critical with any swap in use. Swap is accumulated
   state and means compression stopped keeping up, so it needs no sustain.
3. **Sustain, otherwise.** Critical still standing at the boundary, re-sampled 8
   times at 5-second intervals, with **every** sample reading critical. The loop
   exits on the first sample that is not, so a healthy host pays seconds rather
   than the whole 40.

Six further decisions follow from those three.

4. **Sustain is unanimous, not a majority.** Six of the ten idle samples above
   read critical, so a majority rule would still refuse a healthy host.
5. **Only a critical STANDING at the boundary is confirmed.** One that cleared
   during the chunk leaves nothing live to re-sample, so it is recorded and
   never acted on. This narrows ADR 0175 decision 4. The window fold still feeds
   the collapse and swap arms, because a freeze counts whenever in the chunk it
   happened.
6. **The available FLOOR is not a corroborator for pressure. The collapse level
   is.** An idle host read 8.79 GB against the 9.60 GB floor on 2026-09-09. The
   same host produces critical ticks, so that conjunction is reachable with
   nothing running. The collapse level sits between the freeze's 0.04 GB and
   that 8.79 GB, with room on both sides.
7. **`PRESSURE_MAX` is retired from the pre-flight gate**, the way
   `COMPRESSOR_MAX_GB` was in ADR 0177. Warn is recorded there and never a
   refusal, matching the guard. A caller still setting it is named rather than
   silently obeyed.
8. **The confirm fails open.** An unreadable level ends the confirm without a
   stop, which is the posture every reader in the guard already has.
9. **`LUCIDOS_E2E_WEBKIT_PHASE=nav|cc|both` makes a nav-tail discharge cheap.**
   The chunk range narrows nav only. Three measurements put the CC phase at 93
   to 97 percent of the excursion, so discharging two nav specs paid for all ten
   CC specs.

## Rationale

**A reading an idle host produces cannot be a stop condition.** This is the same
sentence ADR 0175 wrote about the compressor and ADR 0177 wrote about the
available floor. It is now true of the pressure level as well, and the evidence
is stronger than for either of them. The idle host does not merely approach the
threshold. It crosses it in six samples of ten, while every other instrument
says nothing is happening.

**The kernel is still the better judge, and this does not contradict ADR 0175.**
That ADR's claim was that `memorystatus` integrates free pages, page-out rate,
wired growth and jetsam proximity. It also separated the freeze from every
healthy night. Both hold, and what it got wrong is the sampling. The evidence
for critical came from a ten-minute watchdog series, and an instantaneous sysctl
read is far finer than that. The rule was calibrated at one resolution and
applied at another.

**Unanimity is the only sustain rule the trace cannot satisfy.** A majority rule
loses to 6 of 10. Two-out-of-N loses badly. An unbroken run of eight criticals
across 40 seconds is a claim an oscillating level cannot make, and a genuinely
wedged host makes it trivially.

**The window length is measured, not guessed.** Six gate runs at 18:15 on the
same host caught critical twice. One cleared after 5 seconds. The other held
critical through four samples and cleared on the fifth, 25 seconds in. A
20-second window would have refused that host. Both runs returned GO, and the
four that read warn cost nothing.

**Early exit is what makes the window affordable.** The naive version waits 40
seconds at every boundary that sees a critical reading. On this host that is
most of them, so a 34-chunk nav phase would pay 20 minutes for nothing. Exiting
on the first non-critical sample inverts that. A healthy oscillating host pays
5 to 15 seconds. Only a genuinely critical host pays the full 40, where the run
is ending anyway.

**The collapse level is a real corroborator and the floor is not.** This is the
one place where the obvious reading of "corroborate it the way ADR 0177
corroborates the floor" would have reintroduced the bug. Available under
9.60 GB is a reading an idle host produces, and so is a critical tick, so their
conjunction is reachable with nothing running. 2.40 GB is not: the lowest idle
reading on record is 8.79 GB, 3.7 times higher, and the freeze read 0.04 GB.

**The runaway backstop is deliberately not a corroborator.** It already stops on
its own, so naming it again would change only the wording of a stop that fires
either way.

**The gate has to move with the guard, or the two disagree.** ADR 0177 made this
argument about the available floor and the compressor line, and the pressure
line was left behind in the same change. That omission is what refused 22 checks.
The two are now pinned on four more numbers, read out of the gate's own source by
a test rather than promised by a comment.

**A phase-narrowed discharge is the same trade the chunk range made.** The range
exists so a lost tail can be closed without an unfiltered pass. It stopped short
because it could only narrow the cheap end of the cost. The phase selector
finishes the job. It carries the range's guarantees unchanged: the real chunked
path, every skip announced, and an end report that calls the coverage
incomplete.

## Consequences

- **A healthy host with a flapping pressure level runs to the end.** That was the
  cause of the three-night blackout, and this removes it.
- **The gate stops refusing on warn.** Its pressure line and the guard's are now
  the same rule.
- **A boundary that sees critical can cost up to 40 seconds.** On a host that is
  merely flapping it costs 5 to 15. That is the price of not throwing away a run,
  and the boundary line records how long it waited and what ended the wait.
- **A critical excursion that clears inside a chunk no longer stops the run.**
  It is still counted and still printed. This is the coverage deliberately given
  up, and it is what a healthy machine produces here.
- **The guard still has no stop that fires on this host in normal operation.**
  ADR 0177 said to read that as the intended state rather than as coverage lost,
  and that reading is unchanged.
- **What is left that we trust, in order.** Swap in use, which is accumulated
  state and cannot be produced by squeezing idle pages. Available memory at or
  under the collapse level, which no idle reading has ever approached. Critical
  pressure sustained across 40 seconds. The compressor above a 50% of RAM
  runaway backstop, which is a net rather than a danger reading. Everything
  else, including a single pressure reading at any level and a bare available
  number, is recorded and never acted on.
- **One thing we now trust LESS, and have not changed.** ADR 0177 decision 1
  makes `warn or worse` a corroborator for the available floor. Every one of the
  ten samples above read warn or worse, so that corroborator currently says yes
  unconditionally. The floor is therefore closer to uncorroborated than ADR 0177
  assumed. It did not fire here, because available was 11 GB against the 9.60 GB
  floor, but it would on a sub-floor reading. This is left alone deliberately:
  changing it needs its own measurement, not an inference from this one.

## Alternatives considered

**Stop on a majority of the sustain window rather than all of it.** Rejected on
the trace: 6 of 10 is a majority, on an idle host. Any rule the measured
oscillation can satisfy is a rule that refuses a healthy machine.

**Use the existing 5-second peak sampler's window as the sustain evidence, with
no re-sampling.** Attractive, because it is free. Rejected on two counts. The
window is the whole chunk, so unanimity over it means one thing for a 30-second
chunk and another for a 10-minute one. A host that went critical in the last 40
seconds of a long chunk would never satisfy it. The pre-flight gate also has no
sampler at all, so the two instruments could not have shared one rule.

**Keep the window but never exit early.** Rejected on cost: most boundaries on
this host see a critical reading, so a 34-chunk nav phase would spend about 20
minutes waiting. Early exit gives the identical verdict for a fraction of it.

**Corroborate critical pressure with available under the 9.60 GB floor**, which
is the obvious reading of ADR 0177's rule. Rejected, and this is the trap the
decision above exists to avoid. An idle host read 8.79 GB, under that floor, and
the same host produces critical ticks, so the conjunction is reachable with
nothing running. That would have reintroduced the very bug in a fifth place.

**Raise the available floor, or lower it, or drop the pressure stop entirely.**
All rejected for the reasons ADR 0177 already gives. None of them is about the
instrument that actually misfired here.

**Leave the pre-flight gate alone and fix only the running guard.** Rejected for
the third time, for the reason ADR 0177 gives. The gate is what refused the 22
checks, so fixing only the guard would have left the observed failure in place.

**Treat `warn` as the level to stop on instead, since critical is noisy.**
Rejected outright. Warn is noisier still: it read warn or worse in 10 of 10
samples above, and ADR 0175's 5749-sample distribution already rejected it.

**Cut the WebKit matrix, or move `mobile-webkit` to a container or a hosted
runner.** All rejected in ADR 0175 and unchanged here. The measurements say
coverage was never what made the run fail, and cutting tests to fit a broken
instrument is the wrong order of operations.
