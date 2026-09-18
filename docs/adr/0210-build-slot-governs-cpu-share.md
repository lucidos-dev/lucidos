# 0210: A granted build slot governs CPU share as well as concurrency

- **Status**: Accepted
- **Date**: 2026-09-17

## Context

ADR 0070 made a *build slot* a permit to run a heavy build. It caps how many
run at once and sizes itself in RAM: `max(1, total_GB / 16)`, clamped to 8.

On an 18-core / 48 GB Mac that resolves to 3. Three holders each ran a
full-core clippy tree: two coding-agent sessions on `make lint`, and an engine
build in the third slot. Ten `clippy-driver` processes sat at 90-100%, load hit
44, and the machine was unusable. Memory stayed healthy throughout, so the
guard never had anything to react to. It worked exactly as designed, and its
design did not know about cores.

Renicing the tree to 20 by hand dropped load from 44 to 20.5 within seconds.
That is the measurement the priority half of this decision rests on.

ADR 0070 already saw the second half coming. Its alternatives list carried "the
slot partitions cargo jobs too", exporting `CARGO_BUILD_JOBS = ncpu / N`. It
was deferred, because shaping a build's environment is a larger promise than
gating its start. This is the decision that lands it, with an allocation rather
than a fixed number.

## Decision

**A granted slot shapes the build it admits.** Both halves are imposed by the
broker at the grant, so every call site gets them with no Makefile target and
no script touched.

- **The holder runs niced.** `nice +10` by default, overridable with
  `LUCIDOS_BUILD_SLOT_NICE`, where `0` opts out.
- **The holder gets a share of the cores**, exported as `CARGO_BUILD_JOBS`:
  `ncpu / holders`, floored at `ncpu / capacity` and at 1. `holders` counts the
  slots held at the moment of the grant, this build's own included.

On the machine above that means 18 cores for a solo build, 9 for a second, and
6 for every holder at full contention.

Three rules bound it:

- **An explicit caller value always wins.** A `CARGO_BUILD_JOBS` already set is
  left alone and nothing is exported.
- **A nested acquisition imposes nothing.** `LUCIDOS_BUILD_SLOT_HELD` marks a
  tree that already holds a slot, and it passes straight through.
- **Everything still fails open**, which ADR 0070 already required. No pool, no
  `lucidos` binary, or a core count the host will not report all mean the build
  runs as it did before.

## Rationale

**The two mechanisms do different jobs, and neither replaces the other.** Nice
does not reduce total CPU or heat: it decides who wins when something else
wants the machine. Dividing the cores is what actually lowers the load a build
generates. Nice is the one that keeps the Mac usable, and it costs nothing on
an idle host, because it only bites under contention.

**Nice `+10` is the conventional background value.** A bare `nice <cmd>` means
exactly `+10` on macOS and on GNU coreutils, so a human typing `nice make lint`
already gets this. It is meaningfully stronger than `+5`: under Linux CFS a
nice-10 thread gets about 9.5x less CPU than a nice-0 competitor, against 3.3x
at nice 5. The hazard being defended against is ten compilers at 100%. The
number is also nearly free, because every holder gets the same increment, so it
never changes how two builds compete with each other. It governs only
build-versus-everything-else, and `+20` is left as headroom.

**The floor and the allocation answer different questions, so neither alone is
enough.** A fixed `ncpu / capacity` for everyone is the simple version, and it
makes the common case worse for a problem that is not happening: one `make
lint` on an idle 18-core host would compile on 6 cores. A pure held-based share
with no floor has the opposite failure. It states no guarantee at all. Nothing
then says how small a holder's share may get, if the holder count is ever
measured differently from the slot count. Taking the larger of the two keeps
the solo case whole and still names the guarantee.

**The guarantee is currently an invariant rather than live arithmetic.**
Holders are counted by walking the pool, which walks `0..capacity`, so the
count cannot exceed the capacity and `ncpu / holders` is never below
`ncpu / capacity`. The clamp is what keeps the promise true if a future caller
ever counts holders another way. The floor that does bite today is the 1, on a
host with fewer cores than slots.

**The share is measured after the slot is taken.** Counting then is what makes
a holder count itself, and it narrows the race two simultaneous acquirers run.
Each already holds a distinct lock, so the only open window is before the other
one's lock lands. A build keeps whatever it was granted.

**The nice value goes on the broker process, not on the child through a
`pre_exec` hook.** A nice value survives fork and exec, so inheritance covers
every `rustc` in the tree either way. Setting it on ourselves keeps std's fast
`posix_spawn` path, which a `pre_exec` closure would force back to fork+exec.
The broker only waits for the child afterwards, so its own priority is of no
consequence.

**Policy lives in `lucidos-build-slot`, application in the CLI.** The formula,
the constants and the pool walk are slot semantics, and they have to be
unit-testable against a temp pool. `build_slot.rs::spawn_child` is the only
place that builds a `Command`, and it already owns the `LUCIDOS_BUILD_SLOT_HELD`
export and the "deliberately not its own process group" decision.

## Consequences

- **A solo build is unchanged**, which is the common case and the one that must
  not regress. It keeps every core, and nice costs nothing with nothing to
  contend against.
- **A nice increment cannot be lowered again by a non-root process.** So the
  increment lands once, at the grant. A caller who already niced the tree keeps
  their value on top of ours, because `nice(2)` adds.
- **`make test` runs its engine suite niced.** Under contention that makes a
  timing-sensitive test slower, but those tests already contend for the host
  the slot exists to protect.
- **The core share only reaches `cargo`.** A Gradle or Xcode build in another
  repo gets the priority and nothing else. Teaching the broker one build
  system's parallelism flag per ecosystem is not worth it.
- **It bounds the compile, not a test harness.** `CARGO_BUILD_JOBS` governs how
  many `rustc` invocations run at once, and not how many threads the test
  binary then uses. So `make test` compiles on its share and runs its tests on
  the whole host. Capping `--test-threads` would change what the suite
  exercises, and the compile is the heavy half.
- **There is no rebalancer, deliberately.** A build already running keeps the
  allocation it took, even when the pool empties around it. Re-dividing a live
  compile tree would need the holder to watch the pool, and cargo to accept a
  changed job count. It buys only a wider share for a build that is already
  finishing.
- **Two builds granted at the same instant can both take a larger share.** The
  floor bounds how bad that gets, and the window is the gap between one
  `try_lock` and the other's walk.
- **The wrapper prints one line** naming the slot, the priority and the cores,
  so a build that is slower than the last one says why.
- **A build that bypasses the wrapper is still ungoverned**, exactly as ADR
  0070 left it. A bare `cargo build` typed directly gets the whole machine.

## Alternatives considered

**Lower the capacity instead.** Setting the count to 2 on this host would have
stopped the incident too. Rejected as the wrong axis: it throws away the
concurrency the RAM genuinely supports, and it still lets two builds take 18
cores each. The count answers "how many builds fit in memory", and nothing in
it can answer "how much of the CPU may one build take".

**Nice only, no core division.** Cheapest possible change, and it is what the
manual renice proved. Rejected as half the fix: nice does not reduce the work
done, so the fans stay at full and the host stays hot. It only decides who
waits.

**Divide the cores only, no nice.** Bounds the load honestly, and would have
held this host at 18 total compile threads. Rejected because it reaches only
cargo, and because a host at exactly 100% is still an unusable host. Priority
is what makes the difference between a loaded machine and a stalled one.

**A fixed `ncpu / capacity` share for every holder.** One number, no pool walk,
no race at all. Rejected because it taxes the common case: the solo `make lint`
that runs all day would give back two thirds of the machine to protect against
contention that is not there.

**Measure the host's actual load and adapt.** A load average or a running-build
probe would react to real pressure, including builds that took no slot.
Rejected on three counts. The share becomes non-deterministic and hard to
explain. It needs a rebalancer to be worth anything. And the slot already knows
the number that matters, which is how many builds it admitted.

**Set the nice value with a `pre_exec` hook on the child.** More precise, in
that the broker itself would stay at normal priority. Rejected for no gain: the
broker only waits, and `pre_exec` costs the `posix_spawn` fast path that
`runtime/spawn_env.rs` records the same reasoning about.

**Put the mechanisms in `scripts/with-build-slot.sh`.** It is the wrapper every
`make` target already calls. Rejected because that script is a resolver, not
the broker, and it must keep failing open on a plain `git clone`. Slot
semantics live in one place, and shell would have to reimplement the pool walk
to count holders at all.
