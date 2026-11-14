# 0567: A sandboxed agent session compiles without sccache, so no daemon inherits an OS sandbox

- **Status**: Accepted
- **Date**: 2026-10-24

## Context

ADR 0343 sends every agent compile through one shared sccache daemon on port
4227. sccache starts that daemon from the first client that finds none. The
daemon then outlives the client and runs every later `rustc`.

Codex runs under an OS sandbox (`--sandbox workspace-write`, the macOS
seatbelt). A daemon a Codex session started inherited that sandbox. It could
write only that session's worktree, its git dir, `data/` and `/tmp`. Every other
worktree's build then failed:

```
error writing dependencies to `<worktree>/target/debug/deps/<crate>.d`: Operation not permitted
```

`RUSTC_WRAPPER= cargo build` in the same worktree passed, which pinned the
cause to the daemon.

The reverse direction is a sandbox hole. An unsandboxed daemon runs a Codex
session's `rustc` outside its seatbelt, where a `--out-dir` writes anywhere the
user can.

## Decision

A sandboxed agent session gets `RUSTC_WRAPPER=""` and no `SCCACHE_*` variables.
Its `rustc` runs as its own child, inside its own sandbox. Claude Code sessions
and engine-run background tasks keep the shared daemon.

The backend states the sandbox at spawn, as an `AgentSandbox` enum passed to
`apply_lucidos_env`. Nothing infers it from the environment.

## Rationale

A daemon serves any client but keeps the sandbox of its starter. One daemon
cannot serve a sandboxed and an unsandboxed client correctly, in either
direction. Removing the sandboxed client from the daemon fixes both the
breakage and the hole, with no new process to manage.

## Consequences

Codex builds lose the compile cache and pay a full compile of cold
dependencies. Claude Code builds are unchanged. No sandboxed process reaches
either daemon, so a Codex session can no longer poison them.

## Alternatives considered

**A per-session sccache daemon for Codex.** Rejected. Two daemons may not share
one cache dir (ADR 0343). A per-session cache is cold every time, so it costs
the same as no cache plus a process.

**A second shared daemon for sandboxed sessions.** Rejected. Different sessions
have different sandbox roots, so one sandboxed daemon fails for all but the
session that started it.

**Restart the daemon when it is found sandboxed.** Rejected. It kills another
session's in-flight build, and the next sandboxed client restarts the problem.
