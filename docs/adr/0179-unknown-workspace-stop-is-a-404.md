# 0179: An unknown workspace slug gets 404 from the gateway stop route, so a caller can tell which gateway it reached

- **Status**: Accepted
- **Date**: 2026-09-10

## Context

More than one gateway runs on a developer machine. The dev one listens on 5251
and serves source-checkout workspaces. The packaged `Lucidos.app` one listens on
5252 and serves the workspaces under its own app-data dir. Each holds its own
registry, and no workspace is in both.

`POST /~/api/v1/control/workspaces/<slug>/stop` answered 202 for every slug,
including one no gateway could ever have. `GatewayState::stop_workspace` removed
a stack if it had one and returned `Ok(())` either way.

That made a wrong-port stop indistinguishable from a right-port one. It hid a
real bug for as long as it existed: `scripts/stop.sh` posted to a hardcoded
5251, so every packaged workspace's gateway stop went to a process that had
never heard of it. The 202 read as success, the stack was never dropped, and the
packaged supervisor respawned the engine that the stop had just signalled.

## Decision

A stop for a slug the receiving gateway does not have in its registry answers
**404**. A stop for one it does answers **202**, running or not.

The registry is resynced from disk first, exactly as `restart_workspace` already
does. The refusal then describes the registry file, rather than gateway memory
that a direct write by the dev launcher may have left stale.

## Rationale

A caller cannot pick the right gateway if every gateway says yes. The status is
the whole signal a shell caller gets, and 404 is the ordinary HTTP word for
"this server does not have that resource".

The alternative reading, that a stop is idempotent so an unknown workspace is
harmlessly already stopped, conflates two different facts. Not running and not
mine call for different actions, and only the second means look elsewhere.

Keeping 202 for a registered but stopped workspace preserves the idempotence
that actually matters. Nothing has to know whether an engine was running.

## Consequences

- `scripts/stop.sh` reads the status. It resolves the owning gateway's port from
  the engine it is stopping, and reports a 404 rather than treating it as fatal:
  a workspace launched with no gateway is legitimately unknown to all of them,
  and its SIGUSR1 is the real stop.
- The picker surfaces a 404 in its error slot, reachable only when a workspace
  disappears from the registry between the list and the Stop click.
- `lucidos-eval`'s teardown ignores the status already, and only reports a
  transport failure.
- `restart_workspace` keeps its 400 for an unknown id. Its error has other
  causes, so a blanket 404 there would say the wrong thing.
- The running gateways predate this, so the answer only changes once each is
  rebuilt. Until then a wrong-port stop still answers 202, and what protects the
  reclaim in the meantime is `stop.sh` no longer asking the wrong port.

## Alternatives considered

**Leave it at 202 and have `stop.sh` verify by other means.** It could read the
gateway's workspace list and check the slug is there. That is two round trips
where one will do, and it puts the ownership rule in the caller, where each new
caller would reimplement it.

**Broadcast the stop to every running gateway.** No status change needed: post
to all of them and let the owner act. Rejected as unsafe. Two gateways can hold
the same slug for different directories, so a broadcast can stop a workspace
nobody asked about.

**404 for `restart` too, for symmetry.** Rejected. `restart_workspace` fails for
spawn and provisioning reasons as well, and a single 404 would report those as a
missing workspace.
