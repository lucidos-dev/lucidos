# 0263: Stopping a background task ends its whole process group; a natural exit still signals nothing

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

A *background task* ran `bash -c <command>` in the engine's own process group.
Every way of ending one (a stop, the watchdog timeout, Discard or Archive, the
engine's teardown) sent SIGKILL to that one pid. When bash runs a single simple
command it `exec`s it, so the kill landed on the real work. A command list does
not exec. There, bash stays the parent, and the kill took only the shell.

The e2e suite is exactly that shape: `./scripts/e2e.sh > log 2>&1; …`. In one
nightly run a `lucidos background-task stop` reported the task killed twice.
Both times `e2e.sh` went on compiling under init and held the e2e lock, and
Playwright runners stayed alive. The agent had to find the processes and kill
them by hand. A timeout would have done the same, silently, at 3600 s.

The limit was documented as a known gap in ADR 0257 and in `runtime/python.rs`.
ADR 0100 had earlier refused a group kill for foreground `run_bash`.

## Decision

A background task runs in its own process group. An explicit ending signals
the whole group: SIGTERM, then SIGKILL after `GROUP_TEARDOWN_GRACE`, for a stop,
a timeout, and Discard or Archive. The engine's teardown sends SIGKILL to the
group at once. A task that exits on its own signals nothing.

## Rationale

- **A stop is a decision to end the work.** Whoever calls it wants the suite to
  stop, not its wrapper shell. The same holds for a timeout, which is the
  budget the caller chose, and for Discard, which throws the thread away.
- **ADR 0100 still holds where it applies.** It refused the group kill because
  detaching is sometimes deliberate, and a foreground timeout is not a choice
  to end that work. Here only an explicit ending reaches the group. A task that
  exits on its own leaves a deliberate `nohup … &` running, which keeps ADR
  0100's point.
- **Graceful first, for the reasons the coding-agent drivers already use it.**
  SIGTERM lets `e2e.sh` run its trap and lets a Playwright runner close the
  browsers it detached. Those browsers `setsid` out of the group, so only the
  runner's own teardown reaches them. The registry shares
  `GROUP_TEARDOWN_GRACE` with the Claude Code and Codex drivers, so tuning it
  tunes all three.
- **The teardown skips the grace because it has no time for one.** It reaps
  inside `REAP_WAIT` (3 s) under a supervisor that force-kills at 15 s. A 3 s
  grace would spend the whole reap budget and leave every task unrecorded.
- **`killed` comes from whether our signal reached a live child**, not from
  the outcome. A graceful stop can end in a trap's `exit 143`, or `exit 0`, so
  an exit code no longer proves the task ran to completion. `end_task` asks
  `try_wait` first. A child that already exited keeps its verdict and is not
  signalled, since a reaped pid must never be named again.

## Consequences

- A stopped or timed-out task usually reports `signal: 15` rather than `9`. It
  reports `9` when the command ignored SIGTERM through the grace, and a trap's
  own exit code when one ran. `killed` and `timed_out` still say who ended it.
- A stop now takes up to `GROUP_TEARDOWN_GRACE` (3 s) to reap, because the
  grace is a fixed wait: the leader sits unreaped until then.
- The e2e lock a stopped run held is released by its trap, or left stale with
  a dead holder, which the next run reclaims. Before, the holder stayed alive
  and the next run refused to start.
- A background task no longer shares the engine's process group, so a signal
  aimed at the engine's group does not reach it. That is the isolation
  `spawn_env::isolate_in_process_group` exists for.
- **Still out of reach**: a process that detached into its own session. So is
  every task of an engine that died without its teardown (a SIGKILL, an OOM, a
  panic). The boot sweep's note still says to check before re-running.
- A teardown that lands while a stop or a timeout is in its grace cuts the
  grace short (`cut_grace`). The kill channel is already spent by then, and
  sitting out the grace would use up `REAP_WAIT` and lose the record.
- The foreground paths (`run_bash`, `run_python`, a scheduled `.sh`) keep
  `kill_on_drop` alone.

## Alternatives considered

- **SIGKILL the group at once on every path.** Simpler, and it keeps
  `signal: 9`. Rejected: no trap runs, so `e2e.sh` leaves its lock and its
  worktrees, and Playwright's detached browsers pile up. That pile-up is what
  made the coding-agent drivers graceful in the first place.
- **Fix all four exec paths at once**, as the `runtime/python.rs` comment
  proposed. Rejected for now: the foreground paths are short-lived, ADR 0100
  argues against it for `run_bash`, and the background registry is where the
  orphans cost real work.
- **Refuse `; echo $? > file` and other command lists.** Rejected: a syntactic
  guess about intent, the same reason ADR 0100 refused to reject `&`. The
  guidance in `run-e2e` and `system-knowhow/lucidos-cli.md` steers instead.
- **Signal the group after a natural exit too**, to sweep up anything the task
  left behind. Rejected: it would kill a deliberate detach, which is the case
  ADR 0100 exists to protect.
