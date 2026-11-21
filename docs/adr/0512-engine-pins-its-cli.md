# 0512: The engine pins the lucidos CLI it was published with into the workspace at startup, guarded by an inode check on its own launch path; no CLI build id, no per-call version handshake; amends 0063

- **Status**: Accepted
- **Date**: 2026-10-17
- **Amends**: [0063](0063-launch-binaries-live-outside-cargo-target.md)

## Context

A dev engine runs from `.launch/<profile>/<variant>/lucidos-engine`. Every
subprocess it spawns gets the `lucidos` CLI from the same directory: coding-agent
sessions, triggers, scripts and the permission-prompt MCP server.

After an Apply, the background build publishes a new engine, gateway and CLI
into that directory by atomic rename. The running engine keeps its old inode.
The CLI path now holds the new build. Until the user taps *Switch to new
version*, every subprocess runs a newer CLI against an older engine.

This failed silently once. `lucidos successor offer --at plan-approval` posted to
a route the old engine did not have. Axum answered a bare 404. The CLI failed
open to the plain `Approve` / `Request changes` card, and the user was never
offered a new thread.

## Decision

At startup the engine pins its CLI into `<workspace>/.lucidos/cli/lucidos`.
`lucidos_cli_dir()` returns that directory once the pin exists, and the
resolved launch directory otherwise.

The pin is a hardlink of the published CLI, with a copy as the fallback. It is
written to a temp name and renamed into place. The engine stats its own launch
path before and after. A changed inode, or a path it cannot stat, means a
publish may have raced the pin. Then no pin lands, and the engine logs it.

## Rationale

The skew has one cause: two binaries that ship together are found through one
mutable path. Pinning removes the cause. Detecting the skew after the fact would
leave every caller to handle it.

A build publishes the engine first and the CLI last. So a CLI that is newer than
the engine always comes with a changed engine inode. Checking that one inode
covers the race without any build identity in the CLI.

A hardlink keeps the old bytes after a publish, because a publish renames a new
file over the path and never writes the old inode. Temp plus rename means a CLI
process already running from the pin keeps its inode when a later start re-pins.

`lucidos_cli_dir()` stays the one lookup, so no caller changes.

## Consequences

- Subprocesses run the CLI that was published with their engine, until the next
  engine start.
- A pin failure never blocks startup. The engine falls back to today's behavior.
- Packaged builds are not pinned. They name the CLI through `LUCIDOS_CLI_BIN`
  inside the bundle, and an update replaces the bundle and restarts the engine
  together.
- The pin is one file per workspace under `.lucidos/`, which is rebuildable
  runtime state. It needs no pruning.
- The CLI still reports a bodiless 404 on a handoff checkpoint as a likely stale
  engine. That covers a pin that fell back.

## Alternatives considered

- **Detect the mismatch only.** A 404 message alone leaves every subprocess on a
  mismatched pair until the switch. It stays as a safety net.
- **A build id in the CLI, compared on every call.** It needs a `build.rs` for
  the CLI, and a handshake on every call. The inode guard needs neither.
- **Launch the engine from a private copy of the whole launch directory.** It
  fixes the gateway too, but copies hundreds of megabytes per start. The
  gateway's use of binaries is out of scope here.
- **Pin lazily on the first spawn.** The first spawn can come minutes after
  start, which widens the window for a publish.
