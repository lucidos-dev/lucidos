# 0661: Local e2e runs in the e2e root: LUCIDOS_PRIVATE_DIR moves the private subtree, a lucidos-pg-e2e cluster replaces the live one, local runs have no gateway, so a sandboxed run needs no exception

- **Status**: Accepted
- **Date**: 2026-11-11

## Context

Every local e2e run comes from an agent, and every agent now runs in the OS
sandbox (ADR 0645). The sandbox hides `~/.lucidos/private` and makes each
registered workspace's `.lucidos/` read-only. The e2e engine bound its socket in
`private/run`. The e2e workspace sat in `~/workspaces`, which the gateway
registers. The harness read the live shared cluster's superuser password.
So ADR 0645's branch refused a pinned local run inside the sandbox.

## Decision

Every local run, sandboxed or not, uses the *e2e root*
(`~/.lucidos/e2e-root`, `LUCIDOS_E2E_ROOT`, `scripts/lib/e2e_root.sh`):

1. `LUCIDOS_PRIVATE_DIR` relocates the private subtree, in Rust
   (`lucidos_home::private::relocated`), the shell and the test helpers. With it
   set, no older home path is considered. The harness sets it to the root's
   `private/`.
2. The two e2e slots live in the root's `workspaces/`. No gateway registers it,
   so the sandbox leaves it writable.
3. A run reaches its own `lucidos-pg-e2e` cluster on `LUCIDOS_E2E_PG_PORT`
   (default 5511), minted into the root's `private/`. GitHub's macOS runners
   keep their native Postgres and mint the same way.
4. A local run has no gateway. `own` stays the GitHub mode and keeps its data
   under the root.

The sandbox refusal is gone, and nothing in the harness asks whether it is
sandboxed.

## Rationale

- **One override, not a second home.** The port registry, build slots and
  engine cache must stay machine-wide. A second registry would hand an e2e
  engine a port a live workspace holds, and the reclaim path signals whatever it
  finds there (ADR 0025).
- **Not an overridden `HOME`.** cargo, rustup, Playwright's browser cache, git
  config and Claude Code credentials all follow it.
- **An e2e-owned cluster.** The engine needs `CREATEROLE` and `CREATE
  EXTENSION` at every boot, so it needs a superuser. The live cluster's
  superuser password is the one secret a test run should never hold.
- **No sandbox exception.** The root is writable because the policy already
  allows it, so the policy stays as ADR 0645 wrote it.
- **One shape everywhere.** A run that looked up whether it was sandboxed would
  test two harnesses.

## Consequences

- The old `~/workspaces/e2e-test` directories and the `lucidos_e2e_test*`
  databases in the live cluster stop being used. The owner can drop them.
- Gateway-dependent tests run on GitHub only. The restore-origin proofs skip
  unless `LUCIDOS_E2E_GATEWAY=own`.
- The e2e engine's pidfile sits in the e2e root, so the host-pid guard scans
  it there too.
- An engine started under the override hides the moved subtree from its own
  agents too.
- A home path so long that `<root>/private/run/<digest>` breaks the 103-byte
  socket limit is refused with a message naming `LUCIDOS_E2E_ROOT`.

## Alternatives considered

- **Keep the refusal and send runs to GitHub.** Every filtered local run would
  cost a push and a remote round trip, and pinned local runs (`--headed`,
  `@real-claude-code`) would have no path at all.
- **A sandbox opt-out for e2e.** It would weaken the boundary for exactly the
  processes that run the least reviewed code.
- **A `CREATEDB`-only role on the live cluster.** The engine cannot boot
  without `CREATEROLE`.
