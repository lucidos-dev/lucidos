# 0105: A change spanning the engine and the gateway degrades on the older one, never fails closed

- **Status**: Accepted
- **Date**: 2026-08-22

## Context

The engine and the gateway are separate processes with separate lifetimes, and
only one of them restarts when work lands. Apply merges a branch and restarts
the workspace engine. The gateway is machine-global, fronts every workspace at
once, and keeps running: it adopts a rebuilt binary only when the user reloads
or restarts it (ADR 0014, ADR 0021).

So the normal state right after an Apply is **an engine newer than the
gateway**. On a dev machine that gap is minutes, and in practice it is days.
That is not a broken install. It is the default.

The one-device-identity change met this and broke. Its design has the gateway
strip a client's `x-lucidos-device-id` and re-inject the authenticated one. The
engine's hand-over endpoint then refuses a caller that names a target it is not.
Both halves are correct. The engine shipped first, and the running gateway was
nine days old and still forwarded the client's header verbatim.

Every boot then asked for a hand-over the engine refused with a 400. No device
adopted its gateway id, nothing joined, and the merged device list rendered each
device twice. The failure was swallowed as a `console.warn`, so it reached the
user as a confusing list rather than an error. Worked through in
`docs/plans/2026-08-22-device-hand-over-must-not-need-a-fresh-gateway.md`.

## Decision

A change spanning the engine and the gateway must **degrade** on the older of
the two, never fail closed. The new behaviour has to be an improvement the newer
process can offer alone, so the pair still works while the versions differ.

A migration is the sharp case: it must complete against the gateway that is
running, not the one on disk.

## Rationale

The alternative is a coordinated restart, and nothing in the product performs
one. Apply's own restart detection is per workspace and its file list
(`files_require_restart`) names engine sources, migrations and bundled assets. It
cannot restart the gateway, because that would drop every other workspace on the
machine for a change none of them asked for.

Version skew is therefore not an edge case to be engineered away. It is a
property of the topology, and the only place it can be handled is in the design
of each change.

The fix that closed this one shows the shape. The client now asserts the id it
is adopting rather than the one it still stores. An up-to-date gateway replaces
that header with the authenticated id, which is the same value, so nothing
changes there. An older gateway passes it through, and the request is coherent
on its own. The strict engine guard is untouched, and the migration no longer
depends on which gateway happens to be running.

## Consequences

- **A cross-process change carries a skew argument.** State what the older
  process does with it, and why the result is degraded rather than broken.
- **A client request stays coherent without the gateway's help.** A header the
  gateway is expected to correct must already be right when it is sent, so
  passing it through unchanged is safe.
- **A silent fail-closed path is a defect on its own.** This one presented as a
  confusing list. A failure that strands a migration leaves a breadcrumb the
  engine records, rather than a console line nobody reads.
- **We give up the simplifying assumption** that a gateway invariant is available
  the moment the engine relies on it. Some designs get harder for it.
- **Restarting the gateway stays the user's action**, from their own checkout
  (ADR 0021). This decision exists so that action is an upgrade rather than a
  repair.

## Alternatives considered

**Make Apply restart the gateway too.** Rejected: the gateway is machine-global,
so restarting it for one workspace's change drops every other workspace's
connections. It also cannot be done from a worktree at all (ADR 0021), which is
where coding-agent work lives.

**Version-negotiate between the two.** The gateway would advertise a capability
and the engine would branch on it. Rejected as too much machinery for the
problem: it puts a compatibility matrix in the hot path of every request, and
each new branch is a state nobody tests. Degrading needs no negotiation.

**Refuse to serve while the versions differ.** Rejected outright. It kills a
working system over a difference that is usually irrelevant, and the user has no
way to act on it mid-session.

**Relax the engine guard so it accepts the old id as well.** Considered for the
hand-over specifically, and rejected: it widens a security boundary to work
around a deployment fact. The client-side fix costs one header and weakens
nothing.

**Leave it, and tell the user to restart the gateway.** Rejected: it is a repair
step nothing surfaces, on a failure that presents as a confusing screen. It also
does not generalise, since the next such change would strand a different user
who never reads this.
