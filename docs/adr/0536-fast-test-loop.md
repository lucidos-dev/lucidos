# 0536: A spec run over 5 minutes is unfit for iteration: filtered e2e runs stay on this host, GitHub runs reuse an engine and poll adaptively, and /harden retries only what failed

- **Status**: Accepted
- **Date**: 2026-10-20
- **Amends**: [0382](0382-e2e-runs-on-github-from-a-stripped-temp-branch.md),
  [0386](0386-e2e-defaults-to-github.md),
  [0501](0501-main-check-names-failures-and-flaky-tests-do-not-block.md)

## Context

The maintainer set a bar: **a spec run over 5 minutes is not fit for
iterating on a feature or a bug fix**, and that includes `/harden`. Measured
over seven days before this change:

| What | Count | Median | p90 |
|---|---|---|---|
| e2e runs on GitHub | 60 | 15 min | 36 min |
| e2e runs on this host | 112 | 4 min | 12 min |
| `/harden` round, fix time included | 249 | 19 min | 35 min |

- **A GitHub single-spec run was mostly compile.** Every e2e run branch is a
  fresh orphan, and GitHub scopes caches per branch. So the workspace crates
  compiled from scratch each time: 12 min on Linux, 18 min on macOS, for a
  spec that ran 40 s.
- **Without the compile, a run still spent about 3 minutes on overhead**:
  hand-overs, runner setup, a 10-14 s engine boot, browser downloads and the
  driver's 30 s poll.
- **Local runs paid avoidable costs**: `npm ci` every time, mobile WebKit for
  every `-mobile` spec, a release engine, and one global lock.
- **`/harden` re-ran everything after any failure.** 35% of rounds failed,
  and a failed touched spec re-ran the whole set on GitHub.

Plan and measurements: `docs/plans/2026-10-20-fast-test-loop.md`.

## Decision

**Iterate here on a debug engine, send full verdicts to GitHub, and never
compile what a previous build already compiled.**

- **A filtered run stays on this host.** Filtered means `-f`, or after `--` a
  Playwright `--grep`, `-g`, `--grep-invert` or a positional argument. Full
  suites, `--webkit`, `--no-webkit` and the main check still go to GitHub.
  `--github` forces GitHub. This amends ADR 0386, which sent filtered runs to
  GitHub too.
- **A local filtered run is lean.** It builds a debug engine, which compiles
  incrementally. It runs no mobile-webkit project unless `--webkit` or a
  Playwright `--project` asks for it, and then it uses the release engine. It
  skips `npm ci` when the deps fingerprint matches.
- **A Chromium-only local run takes an *e2e slot*, not the global lock.**
  Slot 1 is the `e2e-test` workspace, slot 2 is `e2e-test-2`, each with its
  own database and ports. A WebKit run or the umbrella takes both slots, which
  is the old exclusive lock.
- **A built engine is reused wherever its inputs match.** Each build records
  an *engine input manifest*: every file cargo read, with its git blob hash,
  plus toolchain, profile, features and target. On GitHub the plan job finds
  the newest uploaded engine whose manifest matches the tree. On this host,
  builds file their binaries under `~/.lucidos/e2e-engines/<manifest id>/`.
  The workflow token gains `actions: read` and nothing else (ADR 0382's
  limits stand).
- **A GitHub build that misses compiles incrementally.** The `e2e` cargo
  profile builds workspace crates at `opt-level = 0`, incremental on, with
  dependencies at `opt-level = 3`. The build restores the newest uploaded
  `target/` state and resets the mtime of every unchanged file, so cargo redoes
  only what changed.
- **The driver polls its own run adaptively.** Every 30 s until a test job
  starts, then every 5 s. It falls back to 30 s when fewer than 500 API calls
  remain this hour. No webhook: an agent runs the driver as a polled
  background task.
- **The shards warm up in the background.** At job start the warm-up begins
  apt, the Postgres image, the browser, `npm ci` and a first `gcloud` call. A
  step waits only for what it needs. The e2e database skips the commit flush.
- **`/harden` runs each touched spec where it is fastest.** Chromium projects
  run here in a slot. Specs that run on mobile-webkit go to GitHub as the
  suites start, pinned to the start commit.
- **A failed `/harden` round records what failed in `.lucidos/harden-retry`.**
  The next round, a *retry round*, runs only that: engine-lib tests and specs
  by name, any other suite whole. Its pass reads RERUN, so the full set runs
  once more. Only a full round can pass.
- **The cheap gates run first.** `scripts/harden-gates.sh` runs the 13 prose,
  dash, ADR, context and similar checks side by side, before the suites start.
- **Every run records its timings.** Each GitHub run writes push to verdict,
  per-job queue and run time, and which engine it reused, to
  `~/.lucidos/e2e-github/timings.jsonl`. `HardenRunCompleted` carries each
  suite's start, end and exit.

## Rationale

**Measured after the change**, `drafts-all.spec.ts`, push to verdict:

| Run | Before | After |
|---|---|---|
| GitHub, Linux only, engine reused | 15 min median | 2m51s |
| GitHub, with WebKit, engine reused | 15 min median | 4m33s |
| GitHub, one Rust constant changed | 20 min cold | 10.8 min, 33 s Linux compile |
| This host, Chromium, debug engine warm | 4 min median | 1.4 min of spec, 18 s incremental build |

- **Reuse is the big lever.** A reused engine downloads in 5 s. A shard then
  spends most of its time on the spec itself.
- **Opt-level 0 is what makes a miss incremental.** At opt-level 1 any change
  to the engine crate redid about 210 s of codegen on Linux, because a new
  commit changes the build id. At opt-level 0 the same edit cost 26 s. The
  engine did not run slower: mobile-webkit ran 19 tests in 2.0 min, against
  2.1 min on the release engine.
- **The engine boot waited on `gcloud`, not on migrations.** A runner's first
  `gcloud` call took 5-20 s. Boot fell from 10-14 s to 1.3-1.5 s once the
  warm-up made that call.
- **A filtered run is iteration, a full run is a verdict.** Iteration wants the
  shortest loop, and this host's incremental debug build is shorter than any
  GitHub run. A verdict wants breadth and fresh machines, which GitHub gives.
  ADR 0386 weighed the same split and chose GitHub for both. The 5-minute bar
  reverses that half.
- **A retry round costs what failed, not the whole set.** Prose and engine
  tests caused most failed rounds, and each fix re-ran every suite.

## Consequences

- A filtered run competes for this host's memory again. The slots cap it at
  two runs, and WebKit still runs alone.
- A WebKit miss on GitHub still takes 10-13 min: the macOS build and the
  WebKit spec set the pace. Queues for the five macOS runners add more.
- An e2e run uploads about 1.9 GB of `target/` state on a miss. Artifacts
  expire on GitHub's schedule, and a missing one falls back to building.
- `harden-suites.sh stop` leaves a GitHub WebKit run going. It ends on its own,
  and the driver's sweep deletes its branch a day later.
- ADR 0501 already reruns a failed engine test once in the main check. This
  ADR adds no main check change, and `/harden`'s retry reuses its parser.

## Alternatives considered

- **A webhook to end the wait.** It cut the notice to under a second, but it
  needed a public funnel into this host. The 5 s poll adds at most 5 s and
  needs nothing exposed.
- **A self-hosted runner.** The mirror is public, so a self-hosted runner would
  run whatever an e2e run branch pushes.
- **Paid larger runners.** More cost for a compile that reuse removes.
- **Opt-level 1 for workspace crates.** Measured above: it defeats incremental
  codegen.
- **Packing only the files a build changed.** The restored files went missing,
  and every second build compiled all 646 crates. The state is the whole
  profile directory.
- **Widening the documentation-only fast path to `.claude/**/*.md`.** Nine
  source files read those, so a change there can break a test.
