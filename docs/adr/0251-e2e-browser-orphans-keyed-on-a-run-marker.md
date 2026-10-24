# 0251: The e2e orphan sweep finds its browsers by a per-run environment marker, never by the shared browsers-cache path

- **Status**: Accepted
- **Date**: 2026-09-23

## Context

The e2e lock sweeps orphaned processes at two moments: before it reclaims a
stale lock, and at the teardown of every run that stops its workspace
(`scripts/lib/e2e_lock.sh`). Its `browser` kind SIGKILLs with no RSS threshold.

That kind matched any process whose argv[0] lay in the Playwright browsers
cache. Every Playwright on the host shares that cache, so the match meant
only "some Playwright launched this". On 2026-09-23 a reclaim SIGKILLed dozens
of live browsers from other tools. One was a `chrome-headless-shell` under a
Python Playwright driver in a user venv; others were "Google Chrome for
Testing" instances from other agent sessions. The sweep then refused to start,
because new foreign browsers kept appearing inside its 15s re-scan window.

The `agent` kind already had the right shape: argv[0] narrows the candidates,
and a kernel fact (the cwd) decides. The browser kind needed a kernel fact too.

## Decision

Each hold of the lock gets a random **e2e run marker**. `acquire_e2e_lock`
writes it to the lock file as `RUN_ID=` and exports it as `LUCIDOS_E2E_RUN_ID`
and `__XPC_LUCIDOS_E2E_RUN_ID`. A process is a browser orphan only when its
argv[0] is in the browsers cache AND its environment holds that run's marker.
Teardown sweeps its own run's marker; a reclaim sweeps the dead owner's.

## Rationale

A probe in the session that made this change settled it. It launched headless
WebKit and Chromium through Playwright 1.63 on macOS and inspected each
process:

| Process | Parent | cwd | Plain variable | `__XPC_` variable |
|---|---|---|---|---|
| WebKit main (`Playwright.app`) | the driver | driver's cwd | inherited | inherited |
| WebKit Networking, GPU, WebContent | pid 1 from birth | own bundle dir | not inherited | inherited, prefix stripped |
| `chrome-headless-shell` and helpers | driver, then main | driver's cwd | inherited | inherited |

The WebKit helpers are XPC services, which launchd starts. They are the
processes that exhausted host memory before (see the header of
`e2e_lock.sh`), so any rule that misses them misses the point. Their parent and
cwd say nothing about who asked for them. Their environment does, as long as
the launcher also exports the `__XPC_` name.

The environment is a kernel fact in the sense ADR 0025 requires. The kernel
records it at exec, and no prompt text can write into another process's
environment. The reader never looks at argv: Linux reads `/proc/<pid>/environ`,
and macOS strips the argv line off the front of `ps -E`.

## Consequences

- A foreign Playwright browser survives both sweeps, whoever launched it.
- A lock file written before `RUN_ID` existed yields no browser orphans on
  reclaim. That errs toward a stacked run over a wrong kill, and only once.
- An environment that cannot be read answers no. macOS hides the environment
  of Apple platform binaries from `ps -E`; Playwright's browsers are not
  platform binaries, so this costs nothing today.
- Every process an e2e run starts carries the marker, including the engine and
  the coding agents its tests spawn. Only argv[0] in the browsers cache makes
  one a browser candidate, so that inheritance is harmless.
- `webkit_reaper.sh` applies the same rule. It kills an over-cap WebKit
  process only when it carries this run's marker, and with no marker set it
  kills nothing and warns. A foreign process starving the host is left to the
  host memory guard, which stops the run rather than kill someone else's work.
- The reader is `proc_env_has_entry` in `scripts/lib/proc_env.sh`, shared by
  both, and tested against real processes in `proc_env_test.sh`.

## Alternatives considered

- **Ancestry**, a browser whose parent chain reaches the Playwright runner.
  The WebKit helpers are launchd children from birth. An orphaned foreign
  browser is re-parented to init just like ours, so "re-parented to init"
  identifies nobody.
- **cwd**, the agent kind's fact. The WebKit helpers run in their own bundle
  directory, not the driver's.
- **Recording browser pids per run.** Playwright exposes only the main browser
  pid, not the helpers. A run killed hard enough to need a reclaim cannot
  finish writing its list either.
- **A per-run `PLAYWRIGHT_BROWSERS_PATH`**, a private directory of symlinks to
  the cache. This was the workaround on the day of the incident. It is still a
  command-line substring. It is also unclear whether launchd reports an XPC
  helper's path through the symlink or resolved, so it may miss the helpers.
