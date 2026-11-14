# 0667: Agents read engine diagnostics through a sanitized per-workspace diagnostic log, never the host log

- **Status**: Accepted
- **Date**: 2026-11-12

## Context

Agents debugging Lucidos grepped `~/.lucidos/gateway/gateway.log`, and every
read carded with no grant offer. The guard was right. The gateway data dir is a
private entry, so `SECRET_PATHS` matches it and no grant may cover it (ADRs 0516,
0641, 0644). The OS sandbox hides the same dir (ADR 0645), so an allowed read
still fails there.

The gap was that no engine-owned way to read runtime logs existed. And the file
the agents reached for is the wrong source on every count:

- It is not a log Lucidos writes. It is the gateway's standard output, captured
  by the dev launcher. Packaged, headless and Linux installs capture the same
  stream elsewhere: a service log, `gateway.out.log`, journald. This ADR calls
  that stream the *host log*.
- Every gateway-spawned engine inherits that output, so it mixes every
  workspace on the machine.
- Its lines carried a local time with no date or zone.
- It sits beside TLS keys, the pairing registry and the database files.

## Decision

Each engine tees its own `log!` lines into a *diagnostic log*: a sanitized,
size-capped JSONL file in `<ws>/.lucidos/diagnostic-log/`, stamped in UTC.
Agents read it through `GET /api/v1/engine/diagnostic-log`, wrapped by the
guard-Safe `lucidos diagnostic-log query`. The route serves only a verified
agent origin token minted by the same engine, and it bounds every query.

The host log stays the owner's capture, hidden from agents. Every host log line
now starts with a full RFC 3339 date and UTC offset.

## Rationale

- **The engine writes the file itself**, so it works whatever its standard
  output points at, on every install shape.
- **Each engine records only its own lines.** So no read can return another
  workspace's lines or a gateway line, which no redactor could filter out.
- **One sanitizer runs at write time and again at read time.** Write time keeps
  a raw secret off disk. Read time applies the current redactor to lines an
  older version wrote. It masks the engine's own credentials, known token
  shapes, Postgres passwords, bearer values and secret-named pairs, and drops
  every URL query.
- **Masking cannot classify free text**, such as a user's search query. That
  residue is acceptable only because the caller is an agent of the same
  workspace, which already reads that workspace's events and artifacts. This is
  why the route is never a generic log endpoint and never crosses a workspace,
  gateway, app or device boundary.
- **The secret-read floor stays exactly where it was.** The change adds a Safe
  CLI verb. It widens no grant and changes no sandbox rule.

## Consequences

- An agent reads diagnostics with no permission card, on any install, for its
  own workspace.
- A workspace on another machine is diagnosed by a thread there: each engine
  answers only for itself.
- Gateway lines, panics and raw stderr stay in the host log only. Agents cannot
  read those.
- The diagnostic log file sits in `<ws>/.lucidos/`, which the sandbox leaves
  readable to agents of other workspaces on the same machine. That is the same
  exposure as each workspace's artifacts, and the file holds only sanitized
  lines. The route is the supported path, because only it applies bounds,
  current masking and refusals.

## Alternatives considered

- **Grant `gateway.log` reads, or drop it from `SECRET_PATHS`.** Breaks the
  secret-read floor and exposes other workspaces and the control plane. The
  sandbox blocks it anyway, and the file does not exist on most installs.
- **A route that reads the host log and masks token shapes.** Masking cannot
  remove other workspaces' text or control-plane lines, and journald has no
  file to read.
- **The gateway pipes each engine's output into a per-workspace file.** A
  gateway crash would break the engine's output pipe, and it puts gateway work
  into an engine concern.
- **An in-memory ring buffer only.** Loses the evening before when the engine
  restarts, which is exactly the case that prompted this.
- **Log lines in Postgres.** A write per line, and it fails exactly when the
  database is the thing being debugged.
- **Sanitize only at read time.** Leaves raw secrets on disk.
- **Keep the diagnostic log in the hidden private subtree.** Offered on the
  plan's approval card and not taken. It closes the cross-workspace raw read,
  but costs orphan cleanup after a workspace is deleted and workspace-id
  plumbing for engines started without a gateway.

Plan: `docs/plans/2026-11-12-safe-diagnostic-log-access.md`.
