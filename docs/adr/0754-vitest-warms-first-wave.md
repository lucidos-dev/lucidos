# 0754: Vitest pre-transforms the first wave's module graph instead of capping workers

- **Status**: Accepted
- **Date**: 2026-11-21

## Context

The frontend suite (`npm test`, about 1,611 files) lost 2 to 15 files per full run.
They hit the 5000 ms test timeout or the 10000 ms hook timeout, and each passed alone.
The failures were never random: every one started within 3 s of the run's start.

Vitest runs one worker per core, and every worker fetches modules through the one
Vite server. That server's transform cache starts empty on every run. The first
`maxWorkers` files start together and pay every cold transform. The heavy ones pay
it inside a timed test body (`await import('./store')` in the first case).
Alone, a cold store import took 1,020 ms. With the server warm it took 106 ms.

The default sequencer runs failed files first, then the slowest. A failure also
inflates a file's recorded duration. So the same files opened every run, met the
cold server again and failed again.

## Decision

`vite/warmFirstWave.ts` keeps Vitest's own order. Before the run starts, it
transforms every project module the first `maxWorkers` files reach. It follows
static and dynamic imports, in the Vite environment each file runs in. Worker
count stays at Vitest's default.

## Rationale

Measured on a 32-core host with other jobs running (load 15 to 80):

| Setup | Warm-up | Wall | Files failed |
|---|---|---|---|
| Default, no warm-up (6 runs) | none | 110 to 150 s | 0, 5, 5, 9, 13, 15 |
| 16 workers, no warm-up (2 runs) | none | 171 s | 3, 8 |
| 8 workers, no warm-up | none | 159 s | 1 |
| Whole graph warm-up (3 runs) | 17 to 22 s | 125 to 140 s | 0, 0, 1 |
| First wave warm-up (3 runs) | 2.5 s | 85 to 90 s | 0, 0, 0 |
| First two waves | 3.6 s | 101 s | 0 |
| 8 workers, first wave warm-up | under 1 s | 150 s | 0 |
| 8 workers, no warm-up (same session) | none | 181 s | 0 |
| 24 workers, first wave warm-up (2 runs) | under 3 s | 75, 95 s | 0, 0 |
| Shipped config (5 runs) | under 3 s | 75 to 130 s | tests: 2, 0, 0, 2, 0 |

The shipped config's residual failures were first-wave component tests. One
spent 3.9 s on a store import that took 106 ms alone, with the server idle.
That is CPU starvation inside workers, not transforms. A failure no longer
repeats every round.

The cause is a burst of cold transforms concentrated on the files that start
together. Warming exactly those files removes the burst and costs a few seconds.
Later files start one at a time as slots free up. Their cold transforms are
staggered, and most hit modules the first wave already warmed.

## Consequences

- A filtered run warms only what it runs. `sort()` receives the specifications
  after CLI filters and `--shard`, so a single-file run warms one file's graph.
- A small host warms a smaller first wave, because the wave is its own worker
  count. An unset `maxWorkers` mirrors Vitest's pool default: one core spare,
  or half the cores in watch mode.
- The warm-up is best-effort. A module that fails to transform is skipped, and the
  test that imports it reports the real error.
- The sequencer is set in `vitest.config.ts`, which merges `vite.config.ts`.
  Vite bundles every relative import of its config, even one behind a
  `process.env.VITEST` check. So `vite build` would load `vitest/node` if
  `vite.config.ts` named the sequencer.

## Alternatives considered

- **Cap `maxWorkers`.** The server thread was never saturated during a run (30% to
  70% CPU), and the host's other load dominates whatever share the suite takes.
  Without a warm-up, 16 and 8 workers still failed and ran slower. With it, 24
  workers ran as fast as the default and as cleanly. Two runs cannot separate
  them, and a fixed fraction of cores has no basis on another host.
- **Warm the whole graph.** It removes every cold transform, but costs 17 to 22 s
  on the single server thread before any test runs, even for a one-file run.
- **Drop the Preact preset's Babel passes in tests.** The whole-graph warm-up went
  from 28.7 s to 22 s, inside the host's noise. The cost is Vite's own pipeline.
- **Raise `testTimeout` or retry.** That hides a scheduling defect. The next heavier
  first wave would cross the new limit.
- **Move each test's cold import to the file's top level.** It moves the cost out
  of the timed body, but only for files someone edits. A new test with a dynamic
  import would reintroduce the defect.
