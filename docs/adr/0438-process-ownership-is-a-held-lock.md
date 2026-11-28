# 0438: Process ownership is a held flock, not a pidfile or a port probe

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

A race audit found ownership decided by a check followed by an act. In each
case a rival could act in the gap:

- **The local token.** The mint created its file exclusively, then wrote the
  secret. A rival that read the empty file in between unlinked it and minted
  its own, so two processes trusted different secrets.
- **The engine.** The spawner wrote `engine.pid` before the engine owned
  anything. A respawn signalled the old engine and spawned the new one at once.
  The new one ran migrations, listeners and recovery while the old one still
  drained. A Postgres *startup lease* covered recovery only, and failed open
  after 45 s.
- **The gateway.** A second gateway ran `boot_all` before it failed to bind.
  So it could reclaim and respawn a booting engine, then exit and be respawned
  by its supervisor. Its supervisor also wrote `gateway.pid` for a child that
  might lose.
- **Identity.** The gateway adopted, and proxied to, anything answering health
  on a registry port.

Plan: `docs/plans/2026-10-12-process-ownership-by-held-locks.md`.

## Decision

A process owns a resource by holding an exclusive flock on it for its whole
life: the **owner lock** (`lucidos-owner-lock`). Only the holder writes the
matching pidfile, and a reader trusts that pid only while the lock is held. A
file several processes may create is written under a lock they all take, and
published by renaming a whole temp file.

- The engine takes `<workspace>/.lucidos/engine.lock` as the first step of
  `run()`, blocking, then publishes `engine.pid`.
- The gateway takes `<gateway data>/gateway.lock` first, without blocking. A
  loser exits 75, which its supervisor treats as a clean stop.
- Token minters hold a mint lock (`.<token>.lock`, a std file lock) across
  read, mint and publish, and publish by rename.
- The gateway trusts a health answer only when its `workspace_path` names the
  registry entry's directory.

## Rationale

The kernel releases a flock the moment its holder dies, so a held lock means
exactly "alive and owning". Taking it is the check, so no gap remains. A pidfile
says only who owned something once, and the OS reuses pids.

A successor blocking on the engine's owner lock waits for the whole predecessor
process. That covers every overlap the startup lease missed, and it needs no
wait in the gateway: the respawn signals and spawns, and the kernel orders the
two engines.

## Consequences

- The startup lease, its 45 s fail-open and the gateway reclaim's 300 ms sleep
  are deleted. The token mint loses its blank-file branch and retry loop.
- A fail-closed lock would let a hung predecessor block its successor forever.
  So a stopping engine exits after a 20 s teardown deadline. This bounds a
  process's lifetime and replaces the fail-open, rather than adding a timer to
  a race.
- Spawners (the gateway, `engine_supervisor.sh`, `gateway_supervisor.sh`,
  `lucidos-eval`, the e2e gateway) write no pidfiles. `start_engine` waits for
  a pid its own supervisor's child published.
- A forked child holds a copy of every descriptor until it execs. A release is
  therefore observed promptly, never early. A freshly copied binary can take
  macOS a noticeable time to exec.
- `reload_gateway` re-execs in place and carries the lock's descriptor across
  the exec, so the new image takes the lock back through the same open file.
  No rival launch, and no engine child mid-exec, can take it in between.
- A port is free before its gateway's lock is. `start_gateway`'s reclaim
  therefore waits for the old process to exit, not for the port.
- A blank token file, left by an earlier release's interrupted in-place mint,
  is replaced under the mint lock, where no current writer is ever part-way.
- An engine from an earlier release takes no lock. Until a lock-aware engine
  has run in a workspace, readers fall back to its pidfile, guarded as before
  (`docs/temporary-measures.md` § "Pre-lock engine pidfile fallback"). The
  switch onto this release cannot wait for that old engine either. So it
  overlaps the old drain once, as respawns did before the lease. A thread
  caught in it shows Continue instead of auto-resuming.
- The proxy can still hand the local token to a stranger that binds a dead
  engine's port between two probes. That needs the gateway to authenticate the
  engine, a separate design.

## The shell side

The scripts follow the same rule, through one helper over `perl` flock
(`scripts/lib/flock.sh`), since macOS ships no `flock(1)`. Plan:
`docs/plans/2026-10-12-shell-scripts-take-real-locks.md`.

- **Every script reader of `engine.pid` trusts the pid only while
  `engine.lock` is held** (`engine_live_pid`), and no script deletes the file.
  A script that signals also checks the pid still runs `lucidos-engine`
  (`engine_pids_to_signal`), as the Rust signal paths do.
  `service.sh` carries its own copy, because `uninstall.sh` fetches it alone. A
  test pins the two copies to the same answers.
- **Port allocation is one critical section.** `allocate_ports` holds an
  exclusive flock on the port registry across lookup, next offset, the walk and
  the save. The walk refuses an offset another entry owns. The `mkdir` lock and
  its stale reclaim are gone.
- **Launchers register through the gateway.** They start the gateway first,
  then POST `adopt` with the workspace's directory and port. Only the gateway
  writes `workspaces.json`, so it never re-reads the file at runtime. A blank
  registry is a parse error, and each save uses its own temp file.
- **The build-watch singleton is three locks.** The watcher holds `watch.lock`
  for its whole life. A launcher decides under `start.lock`, which passes to
  the watcher once it owns `watch.lock`. Every engine serving the checkout's
  `dist/` holds `consumers.lock` shared, and the teardown takes it exclusively
  without waiting. This supersedes ADR 0226's process-table vote and the
  `frontend.pid` ref-count.
- **A lock is probed only where its owner acquires blocking.** The engine and
  the watcher do. The gateway does not, so nothing probes `gateway.lock`.
- **The engine registers as a consumer before it waits for its predecessor.**
  The predecessor holds its own consumer lock until it exits, so a respawn
  never leaves the build-watch without one.
- **The watcher's build children inherit `watch.lock`.** Node passes the
  descriptor on, so a watcher that dies mid-build reads as running until that
  build exits. Its teardown kills the build first, so only a crash shows it.
- **`ensure_npm_deps` refuses while the watch runs, solo launches included.**
  The old ref-count let a lone `-b` install under a live watch. The refusal is
  loud and names the fix: stop the checkout's workspaces, then relaunch.

## Alternatives considered

- **Keep the lease and make it fail closed.** Rejected: it serialized recovery
  only. The new engine still ran migrations, listeners and the Vertex relay
  beside the old one, and an advisory lock needs a database connection first.
- **Make the gateway wait for the old engine before spawning.** Rejected as the
  mechanism: it blocks the supervisor loop for every workspace, and other
  launchers (the shell scripts, the desktop app) would each need the same wait.
  The successor waiting on its own lock covers every launcher at once.
- **Publish the token by hard link, with no lock.** It removed the window
  between current minters, but a blank file left by an earlier release then
  had no safe owner: unlinking it races a second minter, and refusing it kept
  the gateway from booting. The mint lock makes the replacement safe.
- **Write `engine.pid` only once the engine answers.** Already rejected in ADR
  0219, because it narrows how the file goes stale. The holder writing it
  under its lock removes the staleness instead.
- **Bind the gateway's listeners before `boot_all`.** Not needed for
  ownership, and it would make clients hang in the backlog during boot.
- **Keep the build-watch process-table vote beside the consumer lock.**
  Rejected: the vote existed only because no lock said who serves. Keeping it
  would leave its unknown cases to reason about, for an answer the lock already
  gives exactly.
- **Have `stop.sh` exclude its own engine from the teardown.** That is what the
  process-table vote did. With a lock, the draining engine still holds its
  consumer lock. So `stop.sh` waits for the engine lock instead, and the
  engine's own teardown deadline bounds that wait.
