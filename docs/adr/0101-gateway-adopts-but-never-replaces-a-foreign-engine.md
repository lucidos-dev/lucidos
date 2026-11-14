# 0101: The gateway adopts an engine it did not start, and never replaces it

- **Status**: Accepted
- **Date**: 2026-08-22

## Context

`POST /~/api/v1/control/workspaces/adopt` registers a directory as a workspace
and starts nothing. Its caller boots an engine on the port that came back. The
eval harness does exactly this for each arm, and the e2e workspace lands in the
same shape.

Such an entry had no runtime stack, and the gateway reads everything off the
stack map. It reported "not started" for as long as the gateway lived, had no
route to proxy over, and could not be stopped. Only two things ever installed a
stack: a gateway restart, which probes each registered port, and a document
navigation. So a live engine was invisible until somebody opened it in a
browser.

The gateway now probes the stackless registry entries on its supervise tick and
adopts the ones that answer. That raises a question the old code never had to
answer: what should happen when an engine the gateway did not start exits?

## Decision

The gateway adopts an **external engine** (see `docs/glossary.md`), and it never
replaces one. When that engine exits, the stack is released and the workspace
reads as stopped. The next open lazy-starts an engine the gateway owns, which is
the existing path for a stopped workspace.

Three guards go with it. A stop is recorded, so the engine draining out of a
Stop is not adopted as though somebody had started it. A Restart or a Retry
transfers ownership, because that is a person asking the gateway to take the
workspace over. And a release waits out a longer silence than a respawn does,
since nothing improves by dropping a stack quickly.

## Rationale

The gateway cannot reproduce how somebody else's engine was launched. An eval
arm runs with a mock model, its own TLS pair and a seeded database. The e2e
workspace binds the port its own harness allocated. A gateway-spawned
replacement would carry the gateway's environment and its own provisioned
database instead. It would answer on the same address while being a different
engine, and for an eval arm that corrupts the measurement it exists to produce.

The eval harness states the expectation from the other side: autostart stays
off, so "the gateway never spawns an arm engine of its own accord"
(`crates/lucidos-eval/src/gateway.rs`). Respawning a dead arm engine would break
that promise once per run, and leave a stray engine behind every time.

Releasing is also the honest report. The engine is gone, so "stopped" is what is
true. The alternative shows a workspace as running while the thing running is
not what the user started.

The distinction is intent, not parentage. `bring_up` runs because somebody asked
for this workspace to be running: the auto-start flag, a restore record, an
open, a Retry. The gateway is then responsible for keeping it running. Adoption
runs because an engine appeared that nobody asked the gateway for.

Gateway startup applies that same split rather than a rule of its own.
`boot_all` STARTS the workspaces the asks name, and merely ADOPTS one that is
healthy and nothing more (`boot_action`). Taking those over would hand every
eval arm a gateway-spawned replacement on the next gateway restart, which in dev
is every `web-dev.sh -b`. The cost is that a lazy-started workspace loses
automatic crash recovery across a gateway reload, and the next open starts it
again.

## Consequences

- An adopted workspace behaves as a regular one while its engine lives: healthy
  in the picker and the switcher, proxied, stoppable, restartable, and reporting
  its unread count.
- A crash of an externally-launched engine no longer gets automatic recovery.
  The workspace goes back to stopped, and the next open starts it. It cost
  nothing the adopt contract promised, since nobody asked the gateway to keep
  that engine alive.
- The state is modelled rather than inferred: `EngineKeeper::External` against
  `EngineKeeper::Gateway` on the stack. `engine: Option<Child>` cannot carry it,
  because a re-adopted engine of our own holds no handle either.
- A stop must be recorded for the guard to work, so `stop_workspace` keeps an
  in-memory note per workspace. It is dropped when a stack is installed again,
  and never persisted: a stop the gateway did not outlive has no draining engine
  left to guard against.

## Alternatives considered

**Respawn a dead external engine, exactly like a reclaimed one.** The
instruction this work started from. Rejected for the reasons above: it produces
a stray impostor engine after every eval and e2e run. A reclaimed engine is a
different case, because the gateway had already undertaken to run that
workspace.

**Install a supervised stack at registration time.** The adopt endpoint would
create the stack itself. Rejected on three counts. A supervised stack with no
engine spawns one within about seven seconds, which breaks the endpoint's
"registration only" contract. That spawn races the caller booting its own engine
on the same port. It also provisions a database the caller does not use.

**Probe the registered port from `list_status` instead.** Cheaper to write.
Rejected: it fixes the badge and nothing else. The workspace stays unroutable,
unstoppable and unsupervised, and it puts a network probe on the picker's
two-second poll.

**Guard the stop with a timer rather than a start time.** Ignore discovery for
N seconds after a Stop. Rejected: the drain has no bound worth guessing, since
the engine sweeps its agent sessions before it stops serving. Comparing the
engine's own `started_at` needs no guess.

**Skip discovery for any workspace the gateway stopped.** Simple, and wrong for
the caller this exists for. The eval harness stops an arm and then boots its own
engine on that port, so a durable "never again" would keep the arms invisible.
