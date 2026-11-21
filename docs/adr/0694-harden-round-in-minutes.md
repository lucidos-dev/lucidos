# 0694: A /harden round runs only what fits in minutes, and the main check runs the slow suites after landing

- **Status**: Accepted
- **Date**: 2026-11-14
- **Amends**: [0463](0463-landing-lane.md), [0662](0662-closed-lane-threads-stay-closed.md), [0502](0502-harden-runs-the-browser-specs-a-change-touches.md), [0536](0536-fast-test-loop.md), [0541](0541-a-busy-e2e-lock-is-given-up.md), [0616](0616-harden-webkit-local-and-carried-verdicts.md), [0658](0658-harden-webkit-github-only.md)

## Context

A `/harden` round took 13 to 39 minutes at p50 and up to 76 at p90. A lane
batch re-hardened after each of the 22 applies to `main` in a day, and one ran
seven rounds without landing. Plan:
`docs/plans/2026-11-14-harden-in-minutes.md`, which holds the measurements.

## Decision

When a main check covers the workspace, the round runs a fast set and the main
check runs the rest after landing. The main-check setting decides coverage:
`here` or `another-host` cover it, `off` does not. An unknown or missing answer
is `off`, which keeps the full round.

The covered round runs:

- the cheap gates, `lint-static` beside the cargo lane, and `lint-rust`
  (rustfmt and clippy) in it;
- `ts`, Vitest, `vite build`, first-paint and knowhow-generated;
- `engine-touched`: the engine's guard tests, the tests of each touched engine
  module, and the tests the engine-filtered table names;
- the own suite of a small crate the diff touches directly;
- `script-libs`: each changed script library's own test;
- the confinement suite, and the path-selected script suites, the release suite
  among them.

The main check gains a Linux and a macOS job for each of three suites. They are
the engine suite, the crate suites and the full script suite. `app-lib` runs on
macOS only, and the main check already ran every browser spec.

A *guard test* reads the source tree at run time, and lives in a module named
`guard_tests`. A meta-test fails on test code outside such a module that reads
the tree.

`harden-suites.sh wait` reports a FAIL at the first failing suite. The rest
keep running; an unfinished suite is never a pass, so the next round owes the
full set.

### The main-check setting

A workspace preference, `main_check_setting`, holds the setting. Unset, it is
`here` in a checkout that can run the main check, else `off`. Only a person
changes it, in Settings. `lucidos main-check setting` prints it for
`harden-suites.sh`.

- Under `here`, a check starts on any new `main`: a lane landing, or a
  fast-forward by follow main, which now records `MainFollowed`. A waiting
  repair admits itself only under `here`. Fix now works under any setting.
- Under `another-host` or `off`, nothing starts here. A landing is owed, and
  switching to `here` checks it.
- Two workspaces set to `here` both check and both repair. Nothing locks
  across hosts; the setting says so.

### A newer main and a running check

A newer `main` cancels a running check whose tests have not started, records
`MainCheckSuperseded`, and checks the newest `main` at once. A test job is an
e2e job that waits for a build: `linux` or `macos`, named `e2e <matrix id>`.
The `suites` and `confinement` jobs need no build and start a minute in, so
counting them would leave almost no window to cancel in. The GitHub wait
prints a mark when the first test job starts. A check whose tests started runs
to its verdict. A restart that reattaches to a run treats its tests as started.

### A repair subscribes to its red

`MainCheckFailed`, `MainCheckPassed` and `MainCheckRedClosed` each name the
repair they report to as `fix_thread_id`. The repair prompt arms one event
wait on all three with that condition, and says what each delivery means.
This amends the idle-time repairs plan, where a repair read findings once
before its final `/harden`.

The repair stands the wait down after its final `/harden`, before it ends
its turn. A live wait withholds the thread's change from Apply, and checks
before the fix lands cannot judge it. Each tests a `main` without the fix,
so a red one is no news. A repair that kept re-arming on those failures held
its own fix back indefinitely. The first check after the fix lands judges it,
and a new red opens a new repair.

### The lane holds clean arrivals while a batch hardens again

ADR 0662 requires a fresh full hardening when a catch-up changes a batch's
head. ADR 0463 lands every clean arrival at once, even while a batch resolves.
Together they can starve a batch: each landing moves `main`, so the next
catch-up needs another round.

Now a batch whose catch-up changed its hardened head opens an *arrival hold*
(`LaneArrivalsHeld`). Until the batch lands or fails, each clean arrival waits
in the queue without a pass. The held arrivals then land in arrival order,
with no re-selection. A cap of 15 minutes (`ARRIVAL_HOLD_CAP`) ends the hold
with the batch still open, and the held arrivals pass at once.
`LaneArrivalsReleased` records the cause. A restart rebuilds the hold, and its
cap counts from the stored `LaneArrivalsHeld`.

The hold covers only the lane's own landings. A `main` that moves another way,
such as a followed `origin`, still forces a catch-up.

## Rationale

### The gate is weaker, said plainly

- **Engine tests outside the touched modules no longer run before landing.**
  By 30 days of data, about 24 branch-specific engine failures a month were in
  untouched modules. They now land, and the main check catches them after.
- **Browser specs no longer run before landing.** 169 of 527 touched-spec runs
  in 30 days ended non-zero; an unknown share were real regressions. They now
  land, and the main check's browser suite catches them.
- **The crates a change reaches through a dependency, not directly, no longer
  run their suites before landing.**
- Guard tests and touched-module tests keep about 22 of the 46 branch-specific
  engine catches.
- A workspace whose main check is `off` keeps today's full round: nothing is
  weaker there.

A main repair owns a red main check (ADR 0464 and the idle-time repairs plan),
so a landed failure has an owner within one check.

### Why these suites stay

They fit in minutes and catch what a reviewer cannot: lint, types, the frontend
build, and the guard tests, which check the codebase's own rules. The release
suite stays because the main check tests the stripped mirror tree, which drops
the release scripts it would test.

## Consequences

- A covered round's p50 target is about 5 minutes, p90 about 11, for an engine
  change. Phase 6 of the plan measures it with `scripts/scoreboard.sh`.
- The busy-slot SKIP of ADR 0541 leaves a covered round, which runs no touched
  spec. It still applies under `off`.
- ADRs 0502, 0616 and 0658 describe the touched-spec suites, which now run only
  under `off`.
- A test that scans the codebase must be written into a `guard_tests` module,
  or the meta-test fails.
- The main check takes longer: the macOS engine suite runs 57 minutes on a cold
  runner.

## Alternatives considered

- **A time box that defers whatever has not finished.** Rejected: a deferred
  suite is a pass by omission.
- **A warm remote runner for Rust.** Not now: it buys speed for the full suite
  but not the shape problem.
- **Run a changed spec file in the round.** Rejected in the grill: the main
  check runs it within one check.
- **Linux-only main-check jobs.** Rejected: the crates hold macOS-only code and
  tests, and the scripts must hold under bash 3.2 and BSD tools.
