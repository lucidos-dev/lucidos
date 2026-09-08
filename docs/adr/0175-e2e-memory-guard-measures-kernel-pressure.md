# 0175: The browser e2e memory guard stops on kernel pressure, not the compressor

- **Status**: Accepted (supersedes the compressor survivability cap decided two
  days earlier in `docs/plans/2026-09-05-e2e-memory-guard-stops-on-real-headroom.md`)
- **Date**: 2026-09-07

## Context

The `mobile-webkit` browser project stopped producing a complete verdict weeks
ago. Every unfiltered nightly ended the same way: the host-memory guard's
compressor survivability cap fired partway through the navigation phase, exit
71, one or more specs with no result. The best night on record cleared nav chunk
33 of 34 and still lost a spec. All 1412 tests that did run passed.

Every lever inside the old design had been spent. Chunk size was at 3, the cheap
phase had been moved ahead of the expensive one, the host was pre-cleaned, and
runs started cold. The project still never finished.

Two instruments settled it. `host_memory_guard.sh`'s per-boundary process
attribution produced 407 lines over one nightly, and the Mac memory watchdog has
sampled this host every ten minutes since June.

**The run's own processes hold nothing.** Across all 37 boundaries the summed
`phys_footprint` of every process the run owns moved from 7.41 GB to 7.99 GB.
The e2e engine went 938 MB to 949 MB. The Docker VM holding the test Postgres
was byte-identical at 3081 MB from start to finish. No Playwright browser
appeared in any boundary's top eight, which means the per-chunk fresh browser
really does die. Meanwhile the compressor climbed 4.86 GB to 17.11 GB.

**What the compressor pool is.** Physical RAM holding compressed anonymous
pages, and those pages belong to whatever was idle when somebody needed room.
Each chunk's fresh browser is a peak-demand excursion. It squeezes cold pages
host-wide, then exits and frees only its own. macOS never proactively
decompresses, so the pool integrates the excursions rather than measuring the
run. More chunks means more excursions, which is why chunk size never helped.

**The pool ignores demand.** Eighty minutes after a clean teardown, touching
4 GB drove free RAM from 4.70 GB to 0.62 GB. The compressor did not move by a
single page.

**The pool is not a loss either.** Later the same morning it released 12.65 GB
inside one ten-minute sample, at pressure normal, as the machine came back into
use. The pages were live and idle, not leaked.

**The cap was calibrated on the one variable that does not distinguish a
freeze.** The watchdog captured the single recorded freeze: compressor 17.41 GB,
free 0.04 GB, pressure critical, then a six-hour hole in the series and a
reboot. Last night's stop: compressor 17.11 GB, free 4.16 GB, pressure normal,
available 13.74 GB. The two compressor readings differ by 0.30 GB. Nothing else
about the two hosts is comparable.

**The host tolerates the cap.** Over 5749 samples the compressor read above
16.80 GB 156 times, and 29 of those were pressure `normal`, observed as high as
18.95 GB.

**The stop the guard called primary cannot fire here.** `/System/Volumes/VM` is
empty and `dynamic_pager` is not running. Every sample since the reboot five
days ago reports swap 0.00 GB, including samples at free 0.05 GB under pressure
warn. That is why the cap was the only stop that ever fired.

## Decision

Kernel memory pressure replaces the compressor cap as the danger stop, and the
boundary check becomes peak-aware.

1. `kern.memorystatus_vm_pressure_level` at `critical` stops the run. It is one
   sysctl, needs no root, and is the reading that separated the freeze from
   every healthy night.
2. `warn` is reported and never acted on.
3. The compressor survivability cap and its two knobs are deleted. The
   compressor keeps only its runaway backstop at 50% of RAM.
4. A run-scoped sampler ticks every 5 s, and the boundary folds the worst
   observation since the previous boundary over its own instantaneous reading.
5. The available-memory floor and the swap stop are unchanged.
6. `LUCIDOS_E2E_WEBKIT_CHUNKS=<first>-<last>` narrows the nav phase to a chunk
   range, announcing every skipped chunk and restating the range at the end.

`mobile-webkit` stays on this Mac at full coverage: all 110 specs.

## Rationale

**Measure the thing that predicts the failure.** The freeze was low available
memory under critical kernel pressure. The compressor number was a bystander
that happened to be high. A guard calibrated on a bystander stops healthy runs,
and it would miss a freeze that arrived without the bystander.

**The kernel is a better judge than we are.** `memorystatus` already integrates
free pages, page-out rate, wired growth and jetsam proximity. Reimplementing
that from `vm_stat` arithmetic is how the compressor cap happened.

**`warn` is not a stop, and that is a data-backed choice.** It occurred 92 times
in three months with no freeze. It also tracks host load as much as memory: one
recorded warn sample sat at load average 254, with the compressor flat. Stopping
on it would swap one false positive for another, which is the failure this ADR
exists to end.

**Peak-awareness is the honest answer to the sampling objection.** A boundary
reading bounds the host at the boundary and says nothing about the chunk that
just ran, whose observed compressor deltas reach 1.16 GB. Folding the sampler's
window over the boundary's own reading closes that without lowering any
threshold and without letting the run stop anywhere new.

**Keeping the runaway backstop is not inconsistent.** It is not a danger reading
and does not claim to be. It is a net for a host doing something nobody
modelled. It sits above the 25.98 GB high-water mark this machine has survived,
so it cannot fire on a healthy run.

## Consequences

- The unfiltered `mobile-webkit` project is expected to complete. Last night ran
  at pressure normal with swap at zero. Availability never dropped below
  11.30 GB against a 9.60 GB floor, so nothing in the new stop set would have
  fired.
- **Nightly runs will still leave the compressor high**, typically 15 GB or so
  by morning. That is squeezed idle memory. It costs the host nothing at
  pressure normal and drains on its own when the machine comes back into use.
  Reading it as a problem is the mistake this ADR corrects.
- **A genuine freeze is now caught by pressure rather than by a byte count.**
  The unattended-survivability argument the cap made is not lost: it moves onto
  the reading that actually carries it.
- The two `LUCIDOS_E2E_COMPRESSOR_CAP_*` knobs are gone. A caller still setting
  one is named at run start rather than left believing the run is capped.
- A carry-over now has a discharge path. A night that loses nav chunks 30 to 34
  can run exactly those the next day.
- **A ranged run is not a complete run**, and the harness refuses to let it read
  as one. Every skipped chunk says so, and the final report restates the range.

## Alternatives considered

**Lower the cap to about 15.5 GB.** The nightly's own proposal, leaving room for
one worst-case chunk. Rejected: it makes a metric that does not measure danger
fire earlier, costing roughly four more nav chunks a night to buy protection
against nothing. It treats the symptom the measurements just retired.

**Keep the cap and raise it.** The same defect with the opposite sign. More runs
would finish, but the stop would still fire on a bystander. The next host to run
warm would hit it, and the question would reopen.

**Move `mobile-webkit` to Playwright's Linux WebKit in a container.** Rejected
on two grounds. The Linux build is the WPE/GTK port, with a different network,
media and font stack. The macOS port is the one that ships in iOS Safari, and
that is what this project covers.

It also isolates nothing that matters. The container runs in a VM on this same
Mac, on the same RAM. The measurements say the run's own footprint was never the
problem.

**Move it to a hosted macOS runner.** Rejected on repo policy and on fit.
`CLAUDE.md` is explicit that GitHub Actions is release-only, and that no
workflow may compile, lint, type-check or test the tree. Lucidos is not
PR-based, so a `pull_request` trigger never fires and a `push` one reports after
the change has landed. The per-change gate is `/harden`.

**Give the project its own scheduled run on a freshly booted host.** Rejected as
unnecessary once the guard is fixed, and it is expensive: it needs a reboot
nobody is present for. It would also have hidden the real defect behind a cold
start rather than removing it.

**Cut the WebKit matrix to a genuinely WebKit-sensitive subset.** A legitimate
option, and the one to revisit if the full run turns out not to finish.
Rejected now because the measurements say coverage was never what made the run
fail. Cutting tests to fit a broken instrument is the wrong order of operations.

**Reset the engine and database per chunk, as the browser already is.**
Rejected: the attribution killed the premise. The e2e engine moved 11 MB across
33 nav chunks and the Docker VM did not move at all. A per-chunk reset would buy
nothing and cost a restart per chunk.

**Stop on `warn` as well as `critical`.** Rejected on the 5749-sample
distribution, as above. It is the option to revisit if a freeze is ever recorded
that `critical` did not precede.
