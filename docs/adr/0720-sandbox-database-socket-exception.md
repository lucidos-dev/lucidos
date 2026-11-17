# 0720: Sandboxed scripts keep the password-authenticated workspace database socket

- **Status**: Accepted
- **Date**: 2026-11-16
- **Amends**: [0645](0645-agent-subprocesses-run-in-an-os-sandbox.md), decision 1

## Context

The bundled Postgres listens only on a Unix socket under the private run directory.
ADR 0645 hid that socket alongside its folder.
Sandboxed scripts on macOS therefore lost the documented `LUCIDOS_DATABASE_URL` handle.
TCP development databases did not expose the regression.

## Decision

Allow connection to the exact socket named by the agent database URL, after the hidden-root denials.
Keep the folder, sibling sockets, password files and passfiles hidden.
Resolve the directory and port from the URL, including relocated private storage.
TCP URLs add no socket exception.

Admit Unix-database agent processes only for databases provisioned by the gateway's bundled password-only backend.
The gateway passes `LUCIDOS_MANAGED_DATABASE=1` for those provisioning results and removes inherited markers for every other launch.
The engine pins this guarantee with its boot database before user environment overlays, reserves the variable, and scrubs it before spawning children.
Dotenv loading preserves the launcher's marker; project files cannot grant managed provenance to an external database.
An external cluster can retain loaded trust for another role even when the engine-admin probe rejects and disk requires passwords.

As defense in depth, query `pg_hba_file_rules` through the engine pool.
Every local rule must require `scram-sha-256`, `md5` or `password`, or reject access.
Disk rules do not prove which rules Postgres loaded. Also attempt a fresh connection
as the engine's admin role with a random incorrect password. Only PostgreSQL's
`28P01` password rejection passes. Successful connections, transport errors,
query failures and the five-second verification timeout refuse the entire spawn.

The admin and agent URLs must name the same socket.

The gateway permanently writes password-only bundled-cluster authentication,
including when adopting an engine. Convergence always reloads running clusters,
even when disk files already match after a failed reload. Remove the adopted-engine
peer fallback: an immutable sandbox exception cannot remain safe if the gateway
later weakens authentication beneath it.

## Rationale

Database credentials remain the boundary, as ADR 0645 already requires.
The agent role retains its restricted grants and read-only transaction default.
A socket exception grants no filesystem access.
Landlock does not gate pathname-socket connects, so Linux also needs the effective
authentication gate before spawning agent processes.

## Consequences

Packaged and headless macOS installations regain script database access without reopening private files.
Legacy, relocated and current private layouts use the same URL-derived rule.
TCP installations and explicit owner sandbox opt-out keep their existing behavior.
Externally managed Unix clusters refuse sandboxed agent spawns, even when their current authentication appears safe.
Their administrators can change loaded rules after admission; the gateway cannot maintain the password boundary there.
Linux gains the same refusal, despite needing no socket policy exception.

Existing authenticated sessions survive an HBA reload. Older adopted engines
whose URLs lack a password or passfile need the owner's **Switch to new version**
action before opening new database connections. The gateway warns about this
transition and never automatically kills or restarts adopted engines.
Older engines without the managed-database handoff also need the owner's Switch action before sandboxed Unix-database spawns work.

Confinement tests exercise real socket connections, neighboring denials and read-only `psql` access.
An environment-handle guard checks the child environment against its sandbox policy.

## Alternatives considered

- **Loopback TCP:** breaks ADR 0452 by making the packaged database reachable by other OS users.
- **Another socket directory:** `unix_socket_directories` requires a Postgres restart; adopted clusters keep running across upgrades.
- **Engine socket proxy:** adds forwarding, protocol filtering and another protected directory where one exact connection rule suffices.

## Environment-handle audit

The script builder (`engine_owned_script_env`) and coding-agent builder (`apply_lucidos_env`) stamp the database URL, workspace and executable search paths.
Compile-cache paths remain outside hidden storage; sandbox shims live under the workspace runtime directory.
The database socket was the only engine-provided handle pointing into hidden storage that scripts were instructed to open.

Three inherited variables intentionally describe storage agents cannot open:

- `LUCIDOS_PRIVATE_DIR`: the private boundary itself, inherited from the gateway.
- `LUCIDOS_GATEWAY_DATA`: gateway-owned state, with only `workspaces/` reopened.
- `FASTEMBED_CACHE_DIR`: the engine's model cache, inherited from the gateway.

No script instruction promises access through these variables.
The reachability guard names these exceptions with their reasons rather than ignoring all inherited paths.
