# 0219: A dev launch adopts a healthy engine, and only -b or the Apply switch replaces one

- **Status**: Accepted
- **Date**: 2026-09-18

## Context

Starting `./scripts/tauri-dev.sh -w dev` on a running dev workspace stopped its
engine twice over, one second apart in effect, and interrupted every in-flight
thread. The threads settled as `ResponseAborted{cause: engine_shutdown}` with no
device actor, so none auto-resumed.

Two independent launcher behaviours did it.

`allocate_ports` asked a pidfile who owns the pinned port. `engine.pid` holds
whatever the gateway spawned last, and a spawn that dies at once leaves a dead
pid there while the healthy engine keeps serving. Both pidfile arms then read
the live engine as an orphan, and `_try_reclaim_stale_lucidos_on_port` sent it
the SIGUSR1 that is its legitimate stop.

`start_gateway` then POSTed `/~/api/v1/control/workspaces/<id>/restart`
unconditionally on its gateway-reuse path. That POST exists because a new
workspace defaults to autostart off, so the gateway's own boot spawns nothing.
Sending it to a workspace already running is a restart nobody asked for.

The second half is a regression from the change that moved `tauri-dev.sh` onto
the gateway path. Its predecessor called `start_engine`, which has always
adopted a healthy engine rather than replacing it.

## Decision

A dev launch never tears down a healthy engine. Two gestures still replace a
live one, and no others: `-b`, which rebuilds on purpose, and `--engine-only`,
which is the Apply switch onto a freshly built binary.

Port ownership is decided by asking the engine. `GET /api/v1/health` carries
`workspace_path`, so a listener reporting this workspace's path is ours.
Stale means not answering: a listener that serves health on the port it holds is
never reclaimed, whatever a pidfile says.

## Rationale

The pidfile was the wrong authority for a question the engine can answer
directly. It is written by one process and read by another, it survives the
process it names, and nothing makes a failed spawn clean it up. A health probe
is one bounded round trip, and it is true at the moment it is read.

Keeping the reclaim is still right. An engine that crashed while bound to a port
is real, and without the reclaim `allocate_ports` walks the offset forward and
persists the drift. What changed is its premise, which the name already carried:
*stale*. An engine that answers is not stale, and the gate now says so.

Adopting rather than restarting also matches what the rest of the system
promises. Apply is non-disruptive by design, and a new engine version is taken
through the in-app Switch. A launcher that restarts on every invocation takes
that choice away from the user, and takes it away silently.

## Consequences

- A plain relaunch of a running workspace leaves its engine, its threads and its
  coding-agent sessions alone. The launcher says it is reusing the engine.
- A relaunch no longer moves a workspace onto a newer on-disk binary. `-b` does
  that, and so does the in-app Switch, which is where that decision belongs.
- Every launch still waits for the workspace through the gateway. An adopted
  engine may have no route yet, and `adopt_running_engines` installs one on the
  next supervise tick. The wait cannot hurry that: the gateway lazy-starts on a
  document navigation only, never on an API call.
- Adopting also checks that the gateway KNOWS the workspace. Only the restart
  POST makes a running gateway re-read `workspaces.json`. So an engine answering
  for a slug it has never heard of takes the POST anyway, because otherwise
  nothing would ever route to it.
- `wait_for_workspace_health` now uses `curl -f`. A bare `-s` exits 0 for any
  http response, so it read the gateway's own 404 and 503 as ready. It could
  never observe the gap it is there to catch.
- A launcher-driven restart is still unattributed, so a `-b` still settles
  in-flight threads crash-shaped. That is deliberate: a shell script is not a
  device, and `RestartIntentNotify::Skipped` names the dev launcher as one of
  the teardowns that genuinely is not a user action.
- `scripts/lib/ports.sh` now shells out to `curl`. Its test suite gained a
  `curl` seam to stay hermetic, on the same footing as its `lsof` stub
  (ADR 0025).
- On the legacy direct-engine path (`LUCIDOS_NO_GATEWAY`), a healthy engine
  whose `engine.pid` has gone stale is no longer silently killed by
  `allocate_ports`. It reaches `start_engine`, which refuses with the holder
  named and `stop.sh` as the remedy. Loud beats silent, and that refusal was
  already reachable whenever the reclaim declined for any other reason. The e2e
  harness never gets there: `ensure_workspace_running` health-probes and reuses
  a live engine before `start_engine` is called at all.

## Alternatives considered

**Fix only the unconditional restart POST.** It is the regression, and it is one
conditional. Rejected because it leaves the reclaim killing the engine before
`start_gateway` is ever reached, which is what actually ended the threads.

**Make the gateway's `spawn_engine` write `engine.pid` only once the engine
answers.** It would have kept the pidfile honest for this case. Rejected as the
primary fix because it narrows one way of going stale rather than removing the
dependency. A pidfile is stale the moment after it is checked either way.

**Attribute a launcher restart to a device so the threads auto-resume.** It
would soften the symptom for `-b`. Rejected: there is no device, and inventing
one would make a crash-safety gate lie. The resume gate deliberately requires
`cause = EngineShutdown` AND a device actor.

**Refuse the launch when a pinned port is held by a healthy engine.** Honest,
and it fits the existing "refusing to walk forward off a pinned port" error.
Rejected because the common case by far is the user relaunching their own
workspace. The right answer there is to adopt, not to make them stop it first.
