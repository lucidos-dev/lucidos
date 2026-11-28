# 0821: Engines and the gateway hold IPv6 loopback beside 127.0.0.1, and dev ports move off tool defaults

- **Status**: Accepted
- **Date**: 2026-11-27

- **Preview amendment**: [ADR 0828](0828-frontend-preview-through-the-gateway.md) replaces the preview port, Vite authentication and certificate handoff with a private socket behind the gateway.

## Context

An engine bound only `127.0.0.1:<port>`. Another program could still bind
`[::1]:<port>`, and on macOS `localhost` resolves to `::1` first. So every
caller that dialed `localhost:<port>` reached the other program, while the
engine looked healthy. It happened on a dev Mac: a second Vite took
`[::1]:5173`, the dev engine's port.

Two things made it likely. Dev engine ports started at 5173, Vite's own default.
And nothing held the IPv6 half of loopback.

## Decision

1. The engine and the gateway's main listener hold both halves of loopback on
   their port. In `Loopback` and `Address` mode they bind the required
   addresses, then `[::1]` on the same port, through
   `crates/lucidos-loopback-listener`.
   - No IPv6 loopback on the host: log one line, serve IPv4 only.
   - `[::1]` already held: startup fails and names the port.
2. The gateway's engine port picker skips a port held on `[::1]`.
3. Dev ports move off common tool defaults, below Linux's ephemeral range:
   engine `17173 + offset`, frontend preview engine `+ 1000`, and a new shared
   Postgres container on `19432` with no offset. The 3000-range port is gone.

## Rationale

- Holding `[::1]` closes the gap for every install, packaged and dev, on every
  port the OS picks. Moving the dev ports alone would only make a squatter rarer.
- Starting with `[::1]` held by someone else would recreate the incident
  silently, so a loud refusal is the right failure.
- A host without IPv6 loopback cannot be shadowed there, since nobody can bind
  it. Refusing to start there would break it for nothing.
- One helper crate holds the policy, so the engine and the gateway cannot drift.
- Registry entries store an offset, so moving the base moves every dev workspace
  with no data migration.

## Consequences

- The engine listens on both TCP loopback addresses before boot recovery, so a
  port conflict fails before any thread resumes. During recovery, direct HTTP
  or HTTPS requests receive a 503 with `x-lucidos-engine-booting: 1`. The response
  carries no workspace identity. The live router and engine socket become
  available after recovery, before the session spawn gate opens.
- Direct launchers wait for successful health responses. The port allocator
  leaves a holder alone when it returns the boot response or its probes time
  out without an explicit connection refusal. The CLI can retry a clonable
  request that received the boot response within its existing connect budget.

- Existing dev workspaces keep their offset and land on the new base on their
  next launch, which prints one line naming both ports.
  - A workspace whose engine still runs on its old port keeps that port. The
    gateway refuses to move a running engine, so the launch says to stop it.
  - The dev gateway moves a stopped entry before it spawns the engine. A spawn
    ahead of the launch lands on the new base too.
  - A `lucidos.toml` pin from 5173 to 6172 moves to the same offset on the new
    base, and the launch rewrites the file and names it.
  - `LUCIDOS_VITE_PORT` in that range is translated per launch, with a message.
- A running shared Postgres container keeps its published port.
- `scripts/lib/ports.sh` owns the shell bases. `DEV_ENGINE_PORT_BASE` in
  `lucidos-gateway-constants` is the Rust copy, and a test ties the two.
- Something already holding `[::1]:5252` stops the packaged gateway from booting.

## Alternatives considered

- **Move the dev ports only.** Leaves every packaged engine and the gateway open
  to the same shadowing on their own ports.
- **Hold `[::1]` but carry on when it is taken.** Starts the exact failure we
  are fixing, with a log line nobody reads.
- **Split the `All` mode's one `[::]` socket per family.** On macOS a later
  program with `SO_REUSEADDR` can still bind `[::1]` beside a `[::]` socket.
  Fixing that is a change to an opt-in network mode and needs its own design.
  It stays open, and users who opt in to all interfaces on macOS keep the gap.
- **Move the dev gateway port 5251.** It is no common tool's default.
