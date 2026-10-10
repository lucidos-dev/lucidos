# 0136: The running engine publishes .lucidos/ports; resolvers never learn a second registry

- **Status**: Accepted
- **Date**: 2026-08-26

## Context

Cross-workspace spawning resolves a target through one file:
`<workspace>/.lucidos/ports`. Two independent readers parse it, the CLI's
`resolve_target` and the engine's `http::workspace_resolver`, and the resolver's
own header requires them to agree.

Nothing wrote that file on a packaged install. The only writers were
`scripts/lib/ports.sh` and `scripts/lib/workspace.sh`, both dev shell. The
packaged gateway keeps its ports in `<app-data>/config/workspaces.json`
(`registry.rs`), which no resolver reads. So `lucidos spawn-thread --to <ws>`
failed on every DMG workspace, with an error that reads like the target is not
a workspace at all.

The second half was silent. `read_ports` defaults a missing `PROTO=` to https,
which is correct for dev, where the engine keeps its TLS. A packaged engine
runs behind the gateway with `LUCIDOS_TLS_*` stripped, so it serves plain http.
A ports file without `PROTO` therefore fails with `record overflow` rather than
anything naming a scheme.

## Decision

The gateway publishes the file whenever it concludes a workspace's engine is
up. `stack::publish_ports_file` writes `API_PORT` and `PROTO` into
`<workspace>/.lucidos/ports`, beside the `engine.pid` already written there. It
runs on the spawn path and on re-adoption. The write preserves keys it does not
own, and the resolvers are unchanged.

## Rationale

The gateway is the only process holding both facts. It assigns `ws.port` from
its registry. It also resolves the engine's scheme once at startup into
`engine_tls`, which is what it proxies and probes over. `spawn_engine` takes
that scheme as an argument instead of re-deriving it from `loopback`, so the
published value cannot disagree with the gateway's own hop.

Publication is keyed on "the engine is up", not on "we spawned it". Re-adoption
returns before the spawn. A gateway restart that finds healthy engines would
otherwise publish nothing, which is the common case on a machine left running.

Writing beside the pidfile also inherits its lifecycle for free. Both files
appear when the engine starts and describe the engine that is running, so
neither can outlive the other.

Key preservation is not defensive coding, it is a real collision.
`swap_ports` in `scripts/lib/workspace.sh` writes `VITE_PORT`, `PG_PORT`,
`PG_DATABASE` and `DATABASE_URL` into this file, and `scripts/status.sh` sources
the result. A clobbering write would strip a live dev workspace's database
details on the next engine start.

## Consequences

- One producer, both postures. Packaged and dev workspaces are now resolvable
  the same way, so a cross-workspace feature cannot work in dev and fail on the
  DMG.
- The file gains an owner it never had. A dev script and the gateway now write
  the same file, which is why the merge is key-scoped rather than a rewrite.
- A stopped workspace keeps a stale ports file, so a spawn aimed at it fails on
  connect rather than on lookup. `engine.pid` already behaves this way and
  re-adoption depends on that, so the two stay consistent. Changing either is
  separate work.
- A bare workspace name now resolves against `$LUCIDOS_WORKSPACES_ROOT` when
  set, else the directory holding `$LUCIDOS_WORKSPACE`, else `~/workspaces`.
  Both resolvers gained the middle step, and both must keep it. Otherwise
  `--to <ws>` and `run_coding_agent(workspace=<ws>)` resolve to different
  places, which is what the resolver's header has always forbidden.

## Alternatives considered

**Teach the resolvers to read `config/workspaces.json`.** Rejected. It gives one
fact two homes and makes every future reader ask which wins. It also couples the
CLI to the desktop app's layout, when the CLI is meant to run anywhere a
workspace does, including the headless tarball.

**Have the CLI ask a running gateway over HTTP.** Rejected. It makes a lookup
depend on a second process being up. The gateway is exactly the component a
packaged install may have restarted underneath you, and the file is readable
when the gateway is not.

**Default `PROTO` to http instead of https.** Rejected as the primary fix. It
papers over a missing `PROTO` line rather than writing one. It also flips the
dev case, which is the posture that has always been https. Writing the value
explicitly leaves nothing to default.

**Let the engine write the file itself rather than the gateway.** Tempting,
since the engine is what serves the port. Rejected because the engine does not
own the packaged bind decision: the gateway makes it, by stripping the TLS env
before the spawn. The engine would have to re-derive a conclusion the gateway
already reached.

**Publish only on spawn, not on adoption.** Rejected. It is the shape that
looks complete and is not. Engines outlive the gateway by design and are
re-adopted on its restart. So the spawn path can go unrun for a long time on a
machine that is never rebooted.

**Push the workspaces root to the engine as a new env var.** Rejected, and this
one was written and then backed out. No env var can be added to a process that
is already running. An engine the upgraded gateway ADOPTS would therefore keep
the old `~/workspaces` fallback until something restarted it. Deriving the root
from `$LUCIDOS_WORKSPACE`, which every engine and subprocess already carries,
needs no restart and computes the identical value.
