# 0824: The gateway lists each workspace's engine build and switches all running ones; nothing restarts an idle workspace

- **Status**: Accepted
- **Date**: 2026-11-27
- **Related**: [0734](0734-version-builds-wait-for-coding-threads.md) (unchanged), [0461](0461-dev-window-restarts-with-the-switch.md)

## Context

Every workspace a gateway starts runs one engine binary, `engine_bin`.
A build replaces that file in place, so a stopped or new workspace already starts on the newest build.
A running engine keeps the build it started on until its own **Switch to new version** restarts it.

With many workspaces on one gateway, nobody could see which ones still ran an older build.
The only way to move them all was to open each one and press Switch.
Some of them are eval or demo workspaces that sit idle on purpose.
Restarting those for every build would interrupt nothing useful and churn their state.

The settings split (design decision 8) asked the gateway to offer a build and each workspace to adopt it.

## Decision

1. **Gateway → Updates lists every workspace with its running build.**
   The newest build comes from `engine_bin --build-id`.
   A running engine's build comes from the `build_id` field of its `/api/v1/health`.
   Route: `GET /~/api/v1/control/engine-builds`.
2. **"Switch all running" restarts only running workspaces behind the newest build.**
   Route: `POST /~/api/v1/control/engine-builds/switch-running`.
   Each restart goes through the same gateway restart as a workspace's own Switch, with the caller's device as actor.
   In-flight coding threads therefore resume as after a Switch.
3. **Nothing restarts an idle workspace for a new build.**
   A new build only replaces the file. The switch skips any workspace with no running stack.
   It checks under the workspace's operation lock, so a Stop that lands first is never undone.
4. **Deferral stays a workspace preference.**
   `defer_version_builds` decides when a workspace's automatic build starts, from that workspace's coding threads.
   ADR 0734 stays as it is.

## Rationale

**Ask the engine, not a record.** The running engine is the authority on its own build.
A record made at spawn would race a build landing between reading the id and exec, and would be lost on a gateway re-exec.
The health probe survives both.

**Behind means newer, not merely different.** The comparison reuses `disk_id_is_upgrade`, so a stale binary never reads as an upgrade.
An engine too old to report a build counts as behind, the same fallback as the gateway's own update check.
An engine that does not answer is never behind. It may be busy or already switching, and a restart would interrupt it for nothing.

**One switch at a time, checked again per workspace.** Overlapping requests queue behind one lock.
Each target is probed again just before its restart, so a workspace that already switched is left alone.

**Explicit action only.** The owner decides when running workspaces move.
That keeps idle eval and demo workspaces untouched, which the design asked for.

**Deferral belongs to the workspace.** It reads that workspace's coding threads.
A gateway-wide deferral would hold builds while any workspace on the checkout codes, which is a different rule nobody asked for.

## Consequences

- One click moves every running workspace, each with Switch semantics.
- A workspace stopped between the listing and its turn is left alone and stays stopped.
- When a newer gateway is on disk, the whole batch goes to the new image in one hand-over (ADR 0461).
  A Stop that lands before the re-exec takes its workspace out of that hand-over.
  The new image skips a handed-over workspace that was stopped while it waited.
- The admin hop (phase 3 of the settings split) must allow both routes for a remote gateway's Updates page.
  The control hop does not reach them.
- A packaged install lists one build per workspace too. Its engine binary changes only on an app update.

## Alternatives considered

- **Restart every workspace when a build lands.** Rejected: it restarts idle eval and demo workspaces for nothing.
- **Start stopped workspaces on the new build.** Rejected for the same reason. A stopped workspace starts on the newest build whenever it next starts.
- **Record the build id when the gateway spawns an engine.** Rejected: it races a build landing at spawn, and a gateway re-exec loses it.
- **Move deferral to the gateway.** Rejected: see Rationale.
