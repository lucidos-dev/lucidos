# 0445: The gateway reaches an engine only over its engine socket, after the kernel names the listener's uid

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

The gateway proxied every request to `127.0.0.1:<registry port>`. A TCP port
does not say who owns it. An engine can die, and a process of another local
user can bind the freed port before the gateway notices. The next request then
handed that process three things:

- the full-authority local token, which mints pairing codes (ADR 0094);
- the paired device's bearer cookie, which the proxy forwards;
- the request body: chat text, uploads, revealed credentials.

A same-user process can already read the token file, so the threat is another
local user: fast user switching, or a shared Mac or Linux host.

Probing harder could not close it. A probe that checks the engine's identity
stops the gateway adopting a stranger, but the port can change hands between
two probes. The user asked for the bad interleaving to be impossible, with no
retry, sweeper, timer or recovery path.

## Decision

Every engine also binds a Unix domain socket, its **engine socket**, at
`~/.lucidos/run/<digest>`. The digest is 16 hex digits of SHA-256 over the
canonical workspace path. The gateway reaches an engine only there, and writes
nothing until the kernel reports that the listener runs as the gateway's own
effective uid.

`lucidos-engine-socket` holds the path, the owner-only bind and the checked
connect, once, for both binaries. In the gateway, `engine_hop` is the one module
that opens a connection to an engine or attaches a credential.

## Rationale

**It authenticates the listener, not the request.** Everything a request
carries is then safe, the cookie and the body included. A narrower credential
would still have sent those to the squatter first.

**The kernel names the peer, so nothing needs a secret.** `getpeereid` on macOS
and `SO_PEERCRED` on Linux report the listener's uid at connect time. There is
no key to mint, store, rotate or pin.

**That is what covers adopted engines.** An engine the gateway did not start
(ADR 0101) derives the same path from its own workspace directory. The gateway
reaches it with the same check, and nobody tells either side anything.

**Two independent layers.** The socket directory is 0700, so another user
cannot create a socket there. Even if a mode were loosened, the uid check
refuses the stranger before the first byte. The engine also drops any accepted
peer whose uid differs.

**A dead engine is just unreachable.** Its socket file is left with nothing
accepting on it, so a connect fails with `ECONNREFUSED`. That is the outcome the
gateway already handled. A refused stranger is a distinct outcome,
`ProbeOutcome::Foreign`, and the proxy answers it 502 rather than the boot
splash, because waiting will not fix it.

**The engine sees the same request.** The hop sets `Host: 127.0.0.1:<port>` and
presents the same tokens, so the engine's door and scopes (ADR 0155) are
unchanged.

## Consequences

- The gateway no longer dials an engine's TCP port. The engine keeps it for the
  CLI, the Python shim, a browser on `localhost`, both e2e suites and
  cross-workspace calls.
- The gateway-to-engine hop has no scheme. `engine_tls` now decides only what
  the engine serves on its TCP port and what its ports file says.
- An engine binds its socket only while it holds its owner lock
  (`engine.lock`). That is what makes removing a stale socket file safe.
- `sun_path` caps a socket path at 103 bytes on macOS. The fixed-length name
  keeps the path short, and a `HOME` long enough to overrun it is an error that
  names the path.
- Each request opens its own connection. A Unix socket connect costs
  microseconds, and every connection is a fresh peer check.
- A busy engine whose socket backlog is full now probes `Unreachable`
  rather than `Slow`. A full Unix-socket backlog refuses a connect at once,
  where a TCP connect timed out. The slowness watch loses that one hint. The
  respawn decision does not change, because it keys on whether the process is
  alive.
- An engine from before this change has no socket, so a newer gateway reads it
  as unreachable until it restarts. That happens once, across the update.
- **This protects the hop, not the engine's own door.** A loopback engine still
  asks no credential on its TCP port (`api::local_auth`), so another local user
  can reach it directly. Locking that door, and moving the other callers that
  send a credential to a loopback port, is a separate design. It is now
  possible because no gateway traffic needs the TCP port.

## Alternatives considered

**TLS with a per-workspace key the gateway pins.** Rejected. It adds key
generation, storage and rotation for a same-machine hop. The engine would need a
second listener anyway, since its TCP port serves plain http to the CLI. The
kernel's uid proves more with none of that.

**A per-spawn secret on the hop.** Rejected. The cookie and the body still reach
the squatter first. An adopted engine was never handed a secret. A challenge
sent before the request is TLS-PSK reinvented.

**The gateway keeps the listening port open across an engine's death.**
Rejected. It covers only engines the gateway spawned, never an adopted one, and
nothing holds the port across a gateway restart.

**Look up the uid behind a TCP connection.** Rejected. Linux can, through
`/proc/net/tcp` or `sock_diag`. macOS has no API for it.

**Linux abstract sockets.** Rejected. They have no permission model, and they
exist on Linux only.

**The socket inside `<workspace>/.lucidos/`.** Rejected. A deeply nested
workspace path overruns `sun_path`, and a workspace path has no length cap.

Plan: `docs/plans/2026-10-12-gateway-reaches-engines-over-a-peer-verified-socket.md`.
