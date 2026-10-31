# 0584: A filtered e2e run on a busy host goes to GitHub instead of waiting, amending 0536, 0351 and 0541

- **Status**: Accepted
- **Date**: 2026-10-26
- **Amends**: [0536](0536-fast-test-loop.md),
  [0351](0351-e2e-harness-owns-memory-recovery.md),
  [0541](0541-a-busy-e2e-lock-is-given-up.md)

## Context

ADR 0536 keeps a filtered run on this host. A warm local spec run takes 1.5
to 4 minutes. A GitHub run took a median of 10. That holds only on a calm host.
On a loaded one the run waits:

- The build memory gate waits up to 15 minutes.
- The host-load gate waits up to 5.
- The memory recovery after a stop waits 30 minutes, three times (ADR 0351).

A busy e2e slot gives up at once, and `/harden` reads that as `SKIP` (ADR
0541).

In one `/harden` round the phone Chromium leg sat in the recovery wait for
about an hour. The same specs then passed on GitHub in about 5 minutes.

## Decision

A filtered run that nothing pinned to this host never waits. When the host is
busy, the run goes to GitHub at once.

- **Eligible:** filtered by default, no `--local`, no local-only flag or
  variable, no inherited `LUCIDOS_E2E_LOCAL=1`, and GitHub available (ADR
  0386's preconditions).
- **The leg:** `e2e_github_handoff` records HEAD, then runs the same script as
  a child with `LUCIDOS_E2E_LOCAL=1` and every wait at zero:
  `LUCIDOS_BUILD_MEMORY_WAIT_SECS`, `HOST_LOAD_MAX_WAIT_SECS` and
  `LUCIDOS_E2E_MEMORY_RESUMES`.
- **A busy exit hands over.** Exit 71 (memory stop), 72 (build memory gate) and
  75 (host load, or every e2e slot held) rerun the same selection with
  `e2e-github.sh --commit <recorded HEAD>`. The lean `--no-webkit` rides along.
  One `[e2e] host busy` line says why.
- **Any other exit is the run's own verdict**, returned as is. A failing test
  is never retried on GitHub.
- **A pinned or unreachable run keeps today's behaviour,** waits included. The
  codes live in one file, `scripts/lib/host_busy_exits.sh`.

## Rationale

The user prefers local for speed and never wants a stall. A zero-wait local leg
costs only the time to detect a busy host. The whole selection reruns on
GitHub, because a partial local verdict plus a partial GitHub verdict would
need a merged report. GitHub runs the shards in parallel, so the specs that
already passed cost little.

ADR 0541 gave up on a busy slot because a queue would park threads. Handing the
selection to GitHub parks nobody, and gives `/harden` a real verdict instead of
a `SKIP`.

## Consequences

- `/harden`'s `e2e-touched` suite gets a GitHub verdict, not a `SKIP`, on a
  busy host with a clean tree.
- A run under `--commit` has no local leg, so a `@real-claude-code` spec in the
  selection does not run on the handed-over run. When every selected spec is
  such a spec, nothing runs and the driver exits 76 (no verdict), never 0.
- A Playwright `--project` pins the run here, since GitHub mode refuses it.
- The memory waits of ADR 0351 now apply only to a run the caller pinned here,
  to the nightly, and to the main check.
- A dirty tree cannot hand off, so it keeps the waits.

## Alternatives considered

- **Rerun only the unrun specs on GitHub.** It needs a merged report and a
  record of which specs ran. The whole-selection rerun is simpler and cheap.
- **Probe host memory before the run.** The gates already measure. Stopping
  them from waiting is enough.
- **Keep the wait, shorten it.** Any wait is a stall when GitHub is ready in
  minutes.
