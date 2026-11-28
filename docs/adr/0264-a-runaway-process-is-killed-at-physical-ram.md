# 0264: A runaway process is killed at physical RAM

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

A development Mac froze overnight and needed a forced restart. A `grep … | head`
had grown to about 261 GB of footprint on a 48 GB host. Its input was almost all
zeros, so it compressed well and fit in the VM compressor, and it filled it.
macOS then spent hours killing and respawning its own services. The same
full-compressor pattern shows in the jetsam reports of two earlier nights.

The jetsam reports keep a process's name, pid and age, and nothing about who
started it. Every direct agent tool call from that evening returned, so the grep
was a subprocess inside something else, and its origin could not be traced.

## Decision

A launchd agent, the *host memory watch*, runs every 30 seconds. It records any
process whose physical footprint crosses a share of RAM (20 percent by default),
with its command line, working directory and parent chain. It kills a process
whose footprint passes the host's physical RAM, after writing that record.

## Rationale

**Physical RAM is a line only a runaway crosses.** A process's footprint can
exceed RAM only through the compressor, so it must hold far more memory than
the machine has. No workload on this host comes near that: the largest healthy
processes are a Docker VM at about 3 GB and a release `rustc` at about 7 GB. The
runaway was over five times the line, so the kill leaves a wide margin.

**The record comes first, because the evidence was what we lacked.** The kill
contains the damage, but the record is what lets the next incident be traced.
Writing it before any signal means a kill never destroys its own evidence.

**Footprint, not RSS.** A compressed page leaves a process's resident set while
still costing the host. The runaway was almost all compressed, so an RSS
threshold would never have seen it. `top`'s MEM column is the footprint, and
one sample covers every process for about a third of a second.

**Outside Lucidos.** The watch must work while every engine is starved or down,
and the runaway was not visibly a child of any Lucidos process. So it is a
launchd agent running a shell script, with no call into the engine, the gateway
or the CLI.

## Consequences

- A process that legitimately needs more memory than the machine has is killed.
  None exists on this host, and `MEMORY_WATCH_KILL_PCT=0` turns killing off.
- It never kills pid 0 or 1, a pid the host-pid kill guard protects (ADR 0025),
  or another user's process. Those are recorded with the reason they lived.
- A kill share under half of RAM falls back to the default. That reads as a
  typo, not a decision to kill ordinary large processes.
- The agent points at the checkout it was installed from, so the installer
  refuses a coding-agent worktree (ADR 0021).
- It is a development-machine tool. Nothing in the packaged app installs it.

## Alternatives considered

**Log only, never kill.** Offered at plan approval and declined. A runaway that
fills the compressor wedges the host within hours. A record that nobody reads
until morning does not stop that.

**Kill at a share of RAM, such as half.** Rejected. Half of RAM on this host is
24 GB, which a large build or VM could one day reach legitimately. A line only a
runaway can cross needs no judgment about which workloads count.

**Run it inside the gateway.** Rejected. The gateway is shipped product code,
and a watchdog that kills arbitrary user processes is not a product feature. It
would also go down with the gateway, which is exactly when it is needed.

**A per-command memory limit for agent shells.** Deferred. It would bound only
processes started through one path, and the runaway's path is unknown.
