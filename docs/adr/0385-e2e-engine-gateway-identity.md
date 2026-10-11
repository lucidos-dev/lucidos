# 0385: The e2e engine takes its gateway identity from a gateway that lists it, never from its caller

- **Status**: Accepted
- **Date**: 2026-10-07

## Context

`lucidos-menu-desktop.spec.ts` failed on chromium on every full GitHub-mode run
(ADR 0382). It asserts the Workspaces row of a direct-port engine that has a
gateway: an `<a>` to the picker. The engine offers that link only when
`LUCIDOS_GATEWAY_PORT` is set.

Locally the spec passed, but not because a gateway fronted the run. The harness
starts the engine directly, and `start_engine` passes the caller's environment
through. A coding-agent session carries the dev engine's environment, so the
e2e engine started with `LUCIDOS_GATEWAY_PORT=5251` and
`LUCIDOS_WORKSPACE_ID=dev`. With that pair it:

- reported its boot phases to the dev gateway as `dev`, and a boot failure
  report would have stopped the gateway respawning the dev workspace;
- would have restarted the dev workspace from `/api/v1/restart`;
- answered its label as `dev`, which `api/workspace_label.rs` already guarded
  against, alone.

The GitHub runner has no parent gateway, so there the same engine had none.

## Decision

The harness decides the e2e engine's gateway identity. It sets both
`LUCIDOS_GATEWAY_PORT` and `LUCIDOS_WORKSPACE_ID` from a live gateway that lists
the e2e workspace at the engine's port, or it clears both
(`scripts/lib/e2e_gateway.sh`).

`LUCIDOS_E2E_GATEWAY` picks the gateway:

- `auto`, the default, uses a gateway already running. On a dev machine that is
  the dev gateway, which lists `e2e-test` once a launch has registered it.
- `own`, which GitHub browser shards set, starts a session-scoped gateway. It
  has its own data dir inside the e2e workspace, its own loopback port, and no
  hook socket. Its registry lists the workspace at the engine's port with
  autostart off. It adopts the engine the harness starts and stops with the
  workspace.

The browser suite still addresses the engine's own port.

## Rationale

- **An identity is a fact about a gateway, so only a gateway can supply it.**
  Copying it from whoever ran the script made the engine claim to be another
  workspace. Requiring the listing at the engine's port is the same pairing
  `workspace_label.rs` checks, applied before the engine boots instead of after.
- **A real gateway on the runner, not a set variable.** The variable alone
  would flip the row while the link pointed at nothing. The own gateway really
  lists and adopts the engine, so a spec that follows the gateway gets one.
- **Adopt only.** An adopted engine is `EngineKeeper::External`: the gateway
  releases it when it exits and never respawns it (ADR 0101). So the harness's
  engine restarts between projects stay the harness's, and the gateway never
  provisions a database. Nothing navigates to `/<slug>/`, so nothing lazy-starts.
- **The direct port is what the spec asserts.** The menu spec pins the
  direct-port shape on purpose. Serving 300 specs under `/<slug>/` with pairing
  is a different test topology, and it would contradict that assertion.

## Consequences

- A local run attaches to the dev gateway only when it lists the e2e workspace
  at the engine's port. Without one, the engine runs gateway-less, says so in
  one line, and the menu spec fails there. That is the honest result.
- `own` cannot run from a coding-agent worktree, because the gateway refuses a
  worktree engine binary with no opt-out (ADR 0021). The harness says so up
  front. GitHub runners are not worktrees.
- GitHub browser shards now also exercise boot reports, label lookups and
  adoption against a real gateway binary.
- The API suite keeps a gateway-less engine on GitHub. Its gateway coverage is
  the chain test.

## Alternatives considered

- **Set `LUCIDOS_GATEWAY_PORT` on the runner to a made-up port.** It flips the
  row and fakes the topology. Rejected outright.
- **Leave the local inheritance alone and fix GitHub only.** Smaller, but local
  runs keep claiming to be `dev` to the dev gateway, with a boot-failure report
  able to stop dev's respawns.
- **Run the whole browser suite through the gateway at `/<slug>/`.** Closest to
  a packaged install, but it needs pairing or a token on every request. It
  changes every spec's base path and drops the shape the menu spec asserts.
- **An own gateway locally too.** Refused from a worktree by design (ADR 0021).
  Outside one it would start a gateway on a developer's machine during every
  run, which nobody asked for.
