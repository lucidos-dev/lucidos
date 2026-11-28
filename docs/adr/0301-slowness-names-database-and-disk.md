# 0301: The slowness warning names a stuck database, a used-up pool and a full disk before it blames nothing, amending 0283

- **Status**: Accepted (amends [ADR 0283](0283-slowness-opens-the-warning.md):
  its window, its memory rule and its unclear reason all stand)
- **Date**: 2026-09-27

## Context

A Mac's disk reached 0 GB free. Docker Desktop's VM got write errors and the
shared Postgres container stalled for about fifteen minutes. Restarting Docker
fixed it.

The bar said: "Lucidos is responding slowly. Nothing on this computer stands
out as busy. If it lasts, restart the computer." That advice was wrong, and
every user whose Postgres stalls or stops gets the same advice.

The gateway already had the evidence. The engine kept answering its health
probe in milliseconds, with `database_reachable: false`. The supervisor counted
that as slow, but the slowness watch kept only the workspace id. So the
classifier could not tell a stuck database from a busy processor, and named
the reason unclear. ADR 0283 had recorded the full disk as a known gap.

## Decision

The watch keeps the kind of slowness it saw, and the bar names the cause that
has a fix of its own. It picks the first reason that holds:

| Reason | When | Shows in |
|---|---|---|
| Disk | The lowest free space across the watched volumes is under 2 GB | every workspace |
| Database | At least half the window's samples hold a database sign | the workspaces that reported one |
| Memory | ADR 0283's rule | every workspace |
| Unclear | ADR 0283's rule | the slow workspaces |

- **Sign.** Each slow workspace in a sample carries one: *database not
  answering*, *database pool exhausted*, or *no answer* (the probe timed out).
  Two signs in one sample merge to the greater, and *no answer* is greatest.
- **Pool.** When the pool's `SELECT 1` fails, the engine's DbHealth probe opens
  one direct connection with the pool's own options, bounded to 1 second. If
  it answers, or Postgres refuses with SQLSTATE 53300, the pool is used up.
  `/api/v1/health` gains `database_pool_exhausted`, true only while
  `database_reachable` is false.
- **Disk.** The gateway reads free space on every running workspace directory
  and its app data directory, only while an episode is open. Each read runs on
  its own thread with a 1-second ceiling, and only one is in flight at a time.
- **Remedy.** A stuck database says to restart Docker, or Lucidos on a packaged
  install. A used-up pool says to stop idle coding-agent threads, then restart
  Lucidos. A full disk says how much is free and to free space first.
- **Last resort.** "Restart the computer" stays only on the unclear reason
  with nothing busy, and now says what was ruled out first.

## Rationale

- **A database sign means the engine answered.** That is what separates a
  stuck database from a starved host. On a starved host the engine itself
  times out, and its database probe fails for the same reason. So a timeout
  outweighs a database sign in its sample, and ADR 0283's memory incident
  still names memory.
- **The database comes before memory.** A stuck database blamed on memory
  leaves the user with nothing to do. Memory blamed on the database costs a
  Docker restart, and the Docker VM is often what holds the memory.
- **The disk comes first.** A full disk is what stalls the database, and it
  stops swap from growing. Freeing space is the fix under all three.
- **The engine says why; the gateway only reads it.** ADR 0037 puts a
  dependency's state in the layer that knows it, and ADR 0014 keeps the
  gateway free of database handles. The field rides the probe the supervisor
  already makes, so there is no new request.
- **Nothing can hang.** The health handler still reads one atomic. The side
  probe runs only after a failed pool probe, on the background ticker. The
  disk read cannot block the sampler past its ceiling, and a hung volume holds
  one thread, not one per sample.
- **2 GB, not the engine's 5 GiB cleanup floor.** That floor means "tidy up
  soon". This one claims the disk caused the slowness, which needs a nearly
  full disk.

## Consequences

- The window, the open and close counts, and the memory thresholds are
  unchanged. Every replay in ADR 0283 still opens with the same reason.
- An older engine omits `database_pool_exhausted`, so a used-up pool behind it
  reads as a database not answering.
- An unreadable disk never names the disk.
- The database-down toast follows the same field. A used-up pool no longer
  tells the user to check Docker.
- Disk I/O contention with free space left still shows as unclear.

## Alternatives considered

- **Pool metrics instead of a side probe.** Rejected: `size` and `num_idle`
  say the pool is busy, not whether the database would answer. A direct
  connection answers exactly the question the remedy depends on.
- **The gateway probes Postgres itself.** Rejected: it breaks ADR 0014's rule
  that the gateway holds no database handle, and the engine already knows.
- **Memory before the database.** Rejected: a flapping pressure reading on a
  large Mac would hide a dead database behind "quit Chrome".
- **Read the disk on every sample.** Rejected: process enumeration already
  runs only during an episode, and a quiet host has no reason to statfs.
- **Reuse the engine's disk helper.** Not possible: the gateway does not link
  the engine crate. It calls the same `fs2::available_space` instead.
