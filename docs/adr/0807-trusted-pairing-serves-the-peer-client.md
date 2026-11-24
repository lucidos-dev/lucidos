# 0807: Trusted pairing lets A serve B's own client, off by default

- **Status**: Accepted
- **Date**: 2026-11-24
- **Amends**: [0723](0723-trusted-remote-workspace-client.md), [0591](0591-the-relay-is-the-one-proxied-hop.md)

## Context

ADR 0723 makes A serve its own client for B's workspaces, and rejected "Trust B
like A". So a user who owns both gateways always gets A's version and features
for B's workspaces. When B is newer, B's features are missing. When B is older,
the client contract may refuse it. The owner of both ends has no way to say
"I trust B".

## Decision

1. Each `RemoteGateway` record on A carries `trusted_pairing`, off by default.
   Only an own device of A's owner can change it, through
   `PUT /~/api/v1/remote-gateways/:name/trusted-pairing`. The guard is the
   same one return access uses. A re-pair resets it to off.
2. With the toggle off, ADR 0723 applies unchanged.
3. With the toggle on, A serves B's own client at A's origin, but only when
   B's workspace-client contract advertises the `peer-served-client`
   capability. A fetches it over the signed relay. The response has no sandbox
   CSP, and A strips `Set-Cookie`, `Clear-Site-Data` and
   `Service-Worker-Allowed`. A stamps B's `index.html` like its own shell, plus
   a `lucidos-peer-client` meta naming B.
4. No B code runs as a service worker in either mode. Worker requests and B's
   `sw.js` stay refused; A's own worker serves B's route.
5. A peer-served client prefixes every storage key and IndexedDB name with
   `peer:<name>:`, so it never reads or writes A's client keys.
6. If B lacks the capability or the fetch fails, A serves its own shell with a
   `lucidos-peer-client-unavailable` meta, and a sticky banner says so.

## Rationale

Pairing proves only that someone held a pairing code. It does not prove who
owns B. So trust stays an explicit, per-pairing choice on A, and the warning
says B's code then runs with A's access. The owner of both gateways is the
only person who can make that claim, and A's settings are where they make it.

The capability comes from B's own contract, not from a build id or a version
guess. So an older B falls back cleanly and starts working when it upgrades.
Keeping workers and origin-wide headers refused limits what a trusted B can
leave behind once the toggle turns off.

## Consequences

- Owners of both gateways can get B's version and features through A.
- B's code runs with A's browser authority while the toggle is on. Turning it
  off does not undo what B's code already did. The confirm dialog names that
  access before the toggle turns on.
- `peer:<name>:` storage stays behind when the toggle turns off. Nothing
  clears it.
- A wire change to the workspace-client surface now fails `/harden` unless
  `VERSION` is bumped or the change is accepted
  (`scripts/check-workspace-client-contract.sh`).
- Existing `relay_isolation_tests.rs` tests pass unedited; trusted-mode tests
  live in `relay_trusted_tests.rs`.

## Alternatives considered

- Keep "Trust B like A" rejected: owners of both ends stay stuck on A's
  client, with no way to opt in.
- Trust every pairing: pairing does not prove ownership of B.
- Serve B's client on B's own origin, or check a signed client inventory:
  needs new hostnames, ports or signing keys. Deferred.
- Let B's service worker run: it would outlive the toggle. Rejected.
- Put the toggle on B: B cannot vouch for itself to A's devices.
