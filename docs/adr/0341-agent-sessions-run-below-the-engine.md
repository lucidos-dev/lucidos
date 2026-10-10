# 0341: Agent sessions and background tasks run at nice +5, below the engine that serves clients

- **Status**: Accepted
- **Date**: 2026-10-01

## Context

The iOS app felt slow while an Apply rebuild and 11 threads ran. A snapshot of
the 18-core, 48 GB host showed load 88. The build slot was doing its job: two
agent builds held slots at nice +10 (ADR 0210). The Apply rebuild ran at nice 0
on purpose (ADR 0304).

Everything else the agents launched ran at nice 0, the engine's own priority:

- `shellcheck` from `make lint`, at 93% for four minutes.
- `vitest`, `tsc`, and test binaries.
- Hundreds of short hook processes per minute.

The engine and Postgres serve every client, and they competed with all of that
on equal terms. The thread list took 0.4 s.

The build slot shapes only what it admits. It cannot reach work that never asks
for a slot, and most agent work never does.

## Decision

**The engine lowers each agent session's process tree and every background
task to its own nice + 5, right after spawn.** One helper does it:
`runtime::spawn_env::spawn_below_engine`. It covers Claude Code (session, side
question, model probe), Codex (`codex exec`, `codex app-server`), and the
background-task registry.

The engine, Postgres, the Apply rebuild and the Vite preview keep nice 0.

## Rationale

**Priority, not throughput, is what a client feels.** Nice does not reduce the
work agents do. It decides who runs first when the host is full. The engine
answering a phone is short work, and it should never queue behind a linter.

**Three tiers, so +5 rather than +10.** `nice(2)` adds, so a build slot inside an
agent session lands at 15:

| Tier | Nice |
|---|---|
| Engine, Postgres, Apply rebuild | 0 |
| Agent processes, background tasks | 5 |
| Builds inside an agent session | 15 |

At +10 an agent build would clamp at 19 next to a plain agent process, and the
lowest two tiers would merge.

**The parent renices after spawn, so no `pre_exec` hook.** A `pre_exec` hook
forces Rust off `posix_spawn` onto `fork()` (ADR 0075). Raising the nice value
of a same-uid process needs no privilege, so the engine can do it from outside.

**The renice names the process group, not the pid.** Each child already leads
its own group (`isolate_in_process_group`). Linux nice is per thread, so
`PRIO_PROCESS` would miss any thread the child started before the call.
`PRIO_PGRP` reaches every thread and process in the group at that instant.
Anything created later inherits.

**A failed renice never fails a spawn.** The child may already have exited, or
the host may refuse. The engine logs it and carries on, the same fail-open rule
the build slot follows.

**Background tasks go lower whoever asked.** A chat thread's background bash is
background work too. One rule at the registry is simpler than a flag per caller.

## Consequences

- Under contention, agent builds and tests run slower than before. That is the
  trade: the engine stays responsive instead.
- On an idle host nothing changes. Nice only bites when something else wants
  the CPU.
- A process an agent spawns into a new session (`setsid`) before the renice
  lands keeps nice 0. The window is the few milliseconds between spawn and the
  `setpriority` call, and no agent starts that fast.
- An agent cannot raise its own priority back. A non-root process cannot lower
  a nice value.
- Foreground chat bash and python tools keep nice 0. The user waits on them.
- The `--version` probes (`runtime::probe_agent_version`) keep nice 0. They
  exit in milliseconds and spawn nothing.
- On its own this left agent `rustc` at nice 0, under the shared sccache
  daemon, while the rest of the agent waited behind it. ADR 0343 closes that.

## Alternatives considered

**Nice +10, the conventional background value.** ADR 0210 chose it for builds.
Rejected here because it stacks with the slot's +10 and collapses the bottom
two tiers.

**A `pre_exec` hook calling `setpriority` in the child.** No race at all.
Rejected because it costs the `posix_spawn` path, which ADR 0075 and
`spawn_env.rs` both preserve on purpose.

**Wrap the command in `nice -n 5`.** Keeps `posix_spawn` and has no race.
Rejected because the agent binary then becomes an argument. A missing `claude`
binary would surface as `nice` exiting 127, not as a spawn error the engine
reports by name.

**macOS QoS classes (`taskpolicy -b`).** Far stronger on Apple silicon: the work
moves to efficiency cores and its I/O is throttled. Rejected as too strong. An
agent build would slow badly even on an idle host, and it is macOS-only.

**Lower the Thread Queue cap.** Fewer threads means less load. Rejected as the
wrong axis: the cap answers how many threads may run, not who wins the CPU.
Eleven quiet threads cost nothing.
