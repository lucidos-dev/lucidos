# 0226: The build-watch teardown asks what is running, not what a marker file says

- **Status**: Accepted
- **Date**: 2026-09-19

## Context

The build-watch is a checkout-level singleton (ADR 0014). Every workspace of a
checkout serves the same `crates/lucidos-app/dist/`, so exactly one watch
republishes it and the last workspace to stop tears it down.

`teardown_shared_build_watch_if_idle` decided "is anyone still serving" by
scanning `$HOME/workspaces/*/.lucidos/frontend.pid`. Only `start_frontend_built`
writes that marker, and it is a `web-dev.sh` path. The gateway starts engines on
three other paths: a lazy start on a proxy hit, the restart control API, and
every in-app *Switch to new version*. None of them writes a marker.

So a workspace whose engine came up any of those ways was invisible to the
ref-count. On 2026-09-19 an unrelated workspace stopped, the scan returned
nothing, and the teardown killed the watch out from under a live `dev`
workspace. `dist/` then froze for five hours while the workspace looked
healthy.

## Decision

The teardown asks the process table: does a live `lucidos-engine` have a
`LUCIDOS_STATIC_DIR` inside this checkout? `engines_serving_checkout_dist`
(`scripts/lib/workspace.sh`) answers it. Two further rules make that answer
safe:

1. An answer that could not be established is **unknown**, and unknown never
   authorizes the kill. That binds at **both** probes: the process listing, and
   the per-engine environment read.
2. The marker scan stays as a second keep-alive vote. Both checks vote only to
   spare the watch, so neither can cause a wrong kill.

## Rationale

This is ADR 0219's principle, which settled the same question for engine ports:
**ownership is what a process answers, never what a pidfile says.** A marker one
of four spawn paths writes cannot answer a question about all four, and the gap
is silent rather than loud.

On the unknown rule, the two costs are lopsided. A wrong "still serving" leaks
one node process until the next launch. A wrong "nothing serving" is the
incident, and it is invisible: the watch dies, `dist/` stops moving, and the
stranded toast reads the status file of the last build the watch completed,
which reports success.

**That rule has to bind at every probe, not just the outermost one.** Guarding
the listing alone left the same hole one level down. A live engine whose
environment `ps -E` will not hand over then read as "not serving", and the
teardown proceeded.

Three outcomes are now told apart. A probe that FAILS means the pid left
between the snapshot and the read, so it serves nothing and skipping it is
right. A probe that succeeds but carries no environment is a live engine we
cannot characterize, which is unknown. A probe that succeeds with an
environment holding no `LUCIDOS_STATIC_DIR` is a real answer: that engine
serves no `dist/`. The test between the last two is `PATH`, which every
readable environment has.

Selection is on `argv[0]`, never the whole command line (ADR 0025). Here that is
not merely the standing rule. A coding-agent session is forked BY an engine, so
`LUCIDOS_STATIC_DIR` is genuinely in its environment. It also carries the thread
transcript in a roughly 22 KB argument. Matching the command line would count
every such session as an engine, and the watch would then never be torn down at
all.

An engine's `dist/` that will not resolve is compared lexically rather than
dropped. The atomic publish renames `dist.staging` onto `dist/`, so the
directory is briefly absent. Dropping the engine there would open a window in
which a concurrent stop tears the watch down.

## Consequences

- The teardown is correct for a gateway-started workspace, which is the common
  case after any Switch.
- Every refusal prints which vote spared the watch. A silent decision is what
  made the incident take hours to read back.
- The teardown costs one `ps -ax` plus one `ps -E` per engine found, at stop
  time only. The listing deliberately omits environments, which a `ps -E` over
  every process would dump in full.
- `scripts/lib/workspace_test.sh` gains a process-table seam, stubbed for the
  whole file so its verdict never depends on the host (ADR 0025).
- A leaked watch is now the failure direction. It costs one node process, and
  the next `web-dev.sh -b` reclaims it.

## Alternatives considered

**Make the gateway write `frontend.pid` too.** Rejected: it gives one file two
owners, in two languages, and the marker is a script-side ref-count with its own
lifecycle (`release_frontend_marker`). It also fixes only the paths we thought
of, which is how the original gap arose.

**Replace the marker scan outright with the process query.** Rejected as
needless churn. The scan still covers the legacy per-workspace Vite dev server,
and `deps-state.sh dev-server-running` depends on its exact semantics through
the `exclude_pid` argument. Keeping it as a second keep-alive vote costs one
cheap check and moves no existing behaviour.

**Have the engine restart a dead watch.** Rejected: the watch is a
checkout-level singleton and every peer engine would race to respawn it. The
engine reports; the operator relaunches.

**`pgrep -x lucidos-engine` to find the engines.** Rejected, and this is the
subtle one. On macOS `pgrep` EXCLUDES ancestors of the calling process, and a
coding-agent session runs under the very engine whose workspace must be counted.
So a `stop.sh` invoked from a session would miss its own workspace and reproduce
the incident. `preflight_reclaim.sh` uses `pgrep` safely because there the
exclusion protects the calling workspace; here it would expose it.

**Treat an unreadable process table as idle.** Rejected by the cost asymmetry
above. It is the same judgment `.claude/rules/rust.md` records for an
unresolved git probe: a probe that could not run is unknown, never a no.
