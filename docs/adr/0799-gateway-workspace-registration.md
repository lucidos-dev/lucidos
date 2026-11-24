# 0799: The gateway registers an existing workspace directory without provisioning or starting anything

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0101](0101-gateway-adopts-but-never-replaces-a-foreign-engine.md)

## Context

A workspace directory sometimes exists before the gateway knows it. Examples
are the context-mode eval's arms, a tree copied off another machine, and a
workspace the dev launcher boots itself.

`create_workspace` provisions a new directory under app-data and slugs a
display name. That does not fit a directory somebody else owns.

This record restates the design comments of
`crates/lucidos-gateway/src/server.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- `adopt_workspace` registers a directory that already exists. It spawns no
  engine, provisions no Postgres and writes no byte inside `dir`.
- It slugs the directory's basename, the name a caller already addresses it by.
- It is idempotent for the same path and never silently repoints.
  `address_points_elsewhere_message` says what a mismatch means.
- A caller that allocates its own ports, such as the dev launcher, may pass
  `port`. It may move a stopped workspace, never a running one, and never onto
  a port another entry holds.
- `adopt_running_engines` runs on each supervise tick. It adopts every
  registered workspace with no runtime stack whose engine already answers.
- That pass starts nothing. What it finds is an `EngineKeeper::External`
  engine.
- `start_workspace` starts a registered workspace that is not running and
  returns at once. The caller's health poll sees the engine come up.
- Starting a running workspace is a no-op. An unknown slug is a 404, as for
  `stop_workspace`.

## Rationale

The caller boots the engine on the port the registry handed back. Without the
adoption pass the entry would report "not started" for as long as the gateway
lives. It would have no route to proxy over and could not be stopped, because
every one of those reads the stack map.

`bring_up` provisions a database and spawns an engine. It runs only when
somebody asked for the workspace to run, and nobody asked here.

The port rule lets the launcher register without writing `workspaces.json`
behind this gateway.

`start_workspace` is a no-op on a running workspace because a respawn would
interrupt the threads of an engine another window started a moment earlier.

## Consequences

- After adoption the workspace is healthy in the picker, proxied, stoppable and
  restartable like any other.
- A caller that boots nothing leaves a listed, stopped workspace that
  lazy-starts on open.
- A path that is already registered at another address is reported, never
  overwritten.

## Alternatives considered

- Using `restart_workspace` to start a workspace is rejected. Its respawn would
  interrupt a running engine's threads.
- Letting the launcher write `workspaces.json` directly is rejected. The `port`
  argument replaces it.
- Silently repointing a registered path is rejected. A mismatch is reported
  instead.
