# 0452: Postgres and the agent browser's DevTools admit only the owning OS user: CDP over a pipe, a minted dev password, a socket-only packaged cluster

- **Status**: Accepted
- **Date**: 2026-10-12
- **Amends**: [0014](0014-multi-workspace-redesign.md)

Record: `docs/plans/2026-10-12-other-local-users-cannot-reach-postgres-or-devtools.md`.

## Context

The threat is another OS user on the same Mac or Linux host, through fast user
switching or a shared machine. Three listeners let that user in:

- **The packaged Postgres cluster** ran with trust auth on `127.0.0.1` and on a
  world-connectable socket in `/tmp`. Anyone could connect as superuser
  `lucidos`, read every workspace's credentials, and run commands as the user
  through `COPY ... FROM PROGRAM`.
- **The agent browser** ran Chrome with `--remote-debugging-port`. CDP has no
  auth, so anyone could read the profile's cookies and drive logged-in sites.
- **The dev shared cluster** published loopback TCP with the public password
  `lucidos`/`lucidos`.

## Decision

- **The agent browser speaks CDP over `--remote-debugging-pipe`.** Chrome
  reads fd 3 and writes fd 4, so no socket exists for anyone else. chromiumoxide
  0.7 has no pipe transport, so it is vendored with one
  (`crates/vendor/chromiumoxide`, a registered temporary measure). On unix the
  launcher refuses a caller's own debugging port.
- **The dev cluster takes a minted password.** `~/.lucidos/pg-shared-password`
  (0600) holds it. The gateway and `scripts/lib/pg_secret.sh` mint the same
  file, and each start converges the role on it. The secret never enters argv
  or a file another user can read.
- **The packaged cluster is socket-only.** It has no TCP listener and one 0700
  socket in the *cluster socket directory* `~/.lucidos/run/pg-<digest>`.
  `peer` auth maps the current OS user, and nobody else, to role `lucidos`.
  The gateway rewrites `pg_hba.conf` and `pg_ident.conf` before every start, so
  an old trust cluster converges with no migration step. `pg_ctl -w start` is
  the readiness check, and the port is fixed at 5432.
- **The directory comes from the engine socket's crate.**
  `lucidos_engine_socket::owner_only_subdir` reuses its run directory,
  owner-only refusal and socket path limit. A successor plan renames the crate
  `lucidos-owner-socket`.
- **One Postgres URL parser** (`lucidos-pg-url`) reads TCP and socket URLs for
  the engine and the gateway, so libpq's `PGHOST` can name a socket directory.

## Rationale

- **The kernel is the cheapest gate.** A pipe and an owner-only directory need
  no secret, nothing to store and nothing to rotate. Another user cannot open
  them at all.
- **Dev cannot use the kernel gate.** Docker Desktop cannot hand a Unix socket
  from its VM to the host. A long random secret over loopback is the next best
  thing, and host traffic reaches the container's scram rule rather than its
  loopback trust rule.
- **No recovery paths.** Each fix makes the bad interleaving impossible rather
  than detecting it. An old trust cluster or an old dev password converges on
  the next ordinary start.

## Consequences

- **Rolling out the dev password restarts running engines once.** A role holds
  one password. Engines started before the converge keep their pooled
  connections, but new connections fail until they restart.
- **The first start after an upgrade restarts the packaged cluster once.** An
  older build's postmaster answers on TCP and in `/tmp`, never on the new
  socket. The adopt probe misses it, and the existing stop-and-start path
  takes over.
- **A headless reinstall stops the running stack first.** A gateway stop
  leaves its engines and the old cluster running, and the new gateway would
  adopt engines that still hold the old TCP URL. So `install.sh` stops the
  service, waits for the engines to exit, then stops Postgres. A running
  workspace that is not autostart stays stopped until someone opens it. The
  desktop app already tears the whole stack down on a restart.
- **A long `HOME` refuses the packaged cluster by name.** The socket path must
  fit 103 bytes on macOS. That is why `e2e-packaged.sh` puts its temporary
  `HOME` in `/tmp` rather than in macOS's long `$TMPDIR`.
- **Upstream prose is not ours.** The prose gate skips `crates/vendor/`.
- **Two related gaps are tracked separately.** Workspace folders are readable
  by other users on macOS, and Chrome runs with `--no-sandbox`. Each has its
  own design thread.

## Alternatives considered

| Alternative | Why not |
|---|---|
| A TCP debugging port with a short life | A window is still a window, and no API names a TCP peer's uid on macOS. |
| A local WebSocket bridge in front of a piped Chrome | The bridge itself listens on TCP, so it reopens the door. |
| Writing our own CDP client on `chromiumoxide_cdp` types | Rewrites the whole browser runtime to change one transport. |
| scram over TCP for the packaged cluster | A secret to store and hand out, and a port left open to guessing and squatting. |
| The packaged socket in the data dir | The macOS data dir path is already 95 of 103 socket bytes on a real machine. |
| A new role for the minted password, keeping the old one | The old public password would stay valid until someone removed it. |
