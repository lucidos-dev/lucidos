# 0343: Agent compiles go to their own sccache daemon and cache, so agent rustc runs below the engine and the Apply rebuild keeps full priority

- **Status**: Accepted
- **Date**: 2026-10-01

## Context

ADR 0341 runs every agent session at nice +5 below the engine. Right after it
landed, the user found things slower. A live check showed why.

`.cargo/config.toml` sets `rustc-wrapper = "sccache"`, and the engine stamps
`RUSTC_WRAPPER=sccache` into every agent session. The `sccache` client does not
compile. It hands the job to one shared, detached daemon, and the daemon runs
`rustc` as its own child, at its own priority. The daemon's priority is
whatever its starter had.

On the day:

- An agent's `sccache` client sat at nice 15, inside a build slot.
- The `rustc` it asked for ran at 870% CPU at nice 0, under a daemon a nice-0
  process had started hours earlier.

So the heaviest agent load never ran below the engine. ADR 0210's +10 never
reached `rustc` either. Meanwhile the rest of each agent session now yielded to
those compiles, so agents slowed down while the engine gained nothing.

The reverse holds too. The daemon exits after ten idle minutes. If an agent
build restarts it, it runs at 15, and the Apply rebuild then compiles at 15
against ADR 0304.

## Decision

**When `sccache` is on PATH, the agent compile env sends agent builds to their
own daemon:** `SCCACHE_SERVER_PORT=4227` and
`SCCACHE_DIR=${XDG_CACHE_HOME:-$HOME/.cache}/lucidos/sccache-agents`. Both the
agent spawn and an agent's background task take this env from one helper,
`runtime::spawn_env::agent_compile_env`.

The default daemon on port 4226 is left to builds that are not agents': the
Apply rebuild and terminals.

## Rationale

**Only agents carry the port, so only an agent can start that daemon.** It
therefore always runs at 5 (started from a session shell) or 15 (started from a
slotted build). A test confirmed the inheritance: a daemon started from an
agent session ran at 5, beside the default daemon at 0.

**The default daemon can no longer be restarted at a low priority by an
agent.** That keeps the Apply rebuild at full priority, which ADR 0304 wants
and which the shared daemon only kept by luck.

**A separate cache directory, because sccache does not lock a shared one.** Two
daemons on one disk cache each keep their own index and evict on their own
view. An entry one daemon evicts, or writes while the other reads, can cost a
failed or wrong build. A separate directory makes that impossible.

**A TCP port rather than a Unix socket.** In a test, a socket file left by a
killed daemon did not clearly recover. A port leaves nothing behind. A port
already held by another program fails the build loudly.

**No cache root means no separate daemon.** Without `HOME` or
`XDG_CACHE_HOME` there is nowhere to put the agents' cache, so the wrapper goes
alone to the default daemon, as before.

## Consequences

- Agent `rustc` now runs at 5 or 15, below the engine. ADR 0210's +10 reaches
  the compiler for the first time, as long as a slotted build is what starts
  the daemon.
- The agents' cache starts cold. The first agent builds after the change
  compile their dependencies from scratch, once.
- The machine holds up to two 10 GiB caches.
- The daemon's exact priority depends on who starts it: 5 or 15. Both are below
  the engine, which is the property that matters.
- A session that was already running keeps its old env, and its builds use the
  default daemon until it is next spawned.
- A chat thread's background task still gets no compile env, as before.

## Alternatives considered

**Share the existing cache directory.** Keeps the cache warm and costs no disk.
Rejected because sccache gives two daemons no locking on one directory. A
correctness risk in the build cache is not worth a one-time warm-up.

**Start the default daemon niced.** One daemon, one cache. Rejected because
the Apply rebuild uses that daemon too, and it would then compile at low
priority, against ADR 0304.

**Have the engine pre-start the agents' daemon at a fixed priority.** Pins it
at exactly one value. Rejected as more machinery for no gain: the daemon idles
out, and whichever agent restarts it is below the engine anyway.

**Turn sccache off for agents.** Simplest, and nice would then reach `rustc`
directly. Rejected because agent worktrees compile the same dependencies over
and over, and the cache is what makes that bearable.

**Teach sccache to compile at its client's priority.** Not available in
sccache 0.14, and an upstream change is outside our reach.
