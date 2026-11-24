# 0725: Gateway pairing stays one-way; return access is a separate scoped grant

- **Status**: Accepted
- **Date**: 2026-11-16
- **Extends**: [0590](0590-gateway-pairing-is-a-deliberate-grant.md), [0591](0591-the-relay-is-the-one-proxied-hop.md), [0647](0647-only-a-signed-claim-proves-the-owner.md), [0656](0656-remote-gateway-manage-hop.md)

## Context

A device can open workspaces across its gateway's pairings. An agent on one
of those gateways cannot read a thread on another through that gateway. The
browser relay requires a device; an engine's origin token verifies only at that
engine. Pointing the CLI at another gateway cannot prove that source identity.

There are only gateways and directional gateway pairings. A gateway that routes
a request is not a new kind of gateway. Pairing A to B and C must not grant B
access to A or C merely because A can open both. This is especially important
when the paired gateway belongs to a different group of people.

## Decision

1. Pairing stays one-way. Agents hosted by A may use A's outgoing workspace access
   for thread reads and spawning. A pairing alone gives B no return authority.
2. A's owner may explicitly enable a **return access grant** on A's existing
   outgoing pairing to B. It is off by default, including for existing records.
   This grants B's agents thread reads and spawning through A to its workspaces
   and outgoing workspace pairings. One grant per source gateway, not per request
   or workspace. A local token, source agent or B itself cannot enable it.
3. Reads use a closed operation allow-list: thread summaries, messages/timeline,
   liveness, event queries/counts, ladder and minimum workspace/thread discovery.
   Spawning uses the existing source spawn gate and target chat/stream flow.
   No other write, device identity, management or secret-reveal access is granted.
4. Each operation is bound to the selected routing gateway, target, original
   caller gateway/workspace/thread, canonical arguments and source spawn verdict.
   Reads and spawns sign under separate claim kinds, distinct from device and
   legacy workspace-call claims. Never upgrade the caller's source verdict.
5. For B returning through A, A authenticates a one-use request receipt by calling
   B through their existing pairing. B verifies that current incoming pairing
   and releases only its locally attested pending operation bound to A. A checks
   its current return grant before dispatch. No reverse pairing credential or
   exported origin/stamp key is required. A's own engines use local stamps.
6. Unpairing either side or revoking the return grant cuts the next operation.
   Grants/discovery are durable; receipts belong to active bounded requests and
   die on restart. An uncertain spawn outcome is not automatically replayed.
7. Select a gateway and workspace explicitly when ambiguous. No fallback after
   denial, browser cookie reuse, claimed device identity or arbitrary HTTP proxy.

Protocol 5 implements the typed operation routes. Each ordinary pairing hop
also obtains a one-use challenge from its destination before signing the
complete operation. A spent challenge, or one issued before a destination
restart, cannot dispatch. Source and target engine stamps additionally bind
unique operation nonces. Return grants acquire a new generation on each enable;
an earlier receipt cannot survive revoke and re-enable. Check current authority
again after network preflight, immediately before dispatch.

Before issuing a return receipt, B reads A's current process identity through
the registered callback origin, using verified transport without credentials.
That identity lives inside B's authoritative stored receipt payload. A compares
it with its current process after redemption. Editing outer callback hints
cannot revive a receipt across A's restart. Request completion or cancellation
also drops any unredeemed receipt immediately.

### Amendment: the remote start setting

Return access (and an ordinary pairing) lets an agent *ask* to start a thread.
Whether the receiving gateway lets it is a second, per-pairing choice there:
its **remote start setting**, `allow`, `ask` or `deny`. Default `deny`, for
new and existing pairings alike.

- The receiving gateway decides, because only it knows its own pairing. It
  refuses a `deny` spawn with a message naming the setting. It signs `allow`
  or `ask`, with the pairing's id and label, onto the operation as the
  `x-lucidos-remote-start` header. The engine verifies that claim at arrival.
- An `ask` thread starts parked on a `StartRemoteThread` permission card.
  The engine keys the hold on the thread's first message, never the turn's
  origin, so a Continue or boot resume is held too. Nothing is spent before
  the owner answers, not a title, not an image description.
- Only a device tap answers the card. Allow once runs the parked turn. Deny
  ends the thread refused. **Always allow** and **Always deny** also set the
  pairing's setting on the gateway. Stop closes the card, and nothing runs.
- A Remember pick saves its setting before the answer is recorded. A failed
  save refuses the pick and leaves the card open.
- A start the owner allowed whose turn never began, because the engine
  stopped in between, begins at boot.
- A legacy relayed workspace call cannot carry the header or park on a card.
  The receiving gateway passes it only under `allow`.
- The sending agent's spawn result carries `remote_start`: started, or
  waiting for the owner.
- Both mixed builds fail closed. A gateway posts every routed spawn to
  `/api/v1/workspace-operations/receive-start`, which only an engine that
  enforces the setting serves. An older engine answers 404 and starts nothing,
  and the gateway refuses the start with that reason, under `allow` too. An
  older gateway sends no header, and a newer engine refuses a spawn without
  one. Each route takes one operation kind.

Rejected: deciding in the engine, which would have to mirror the pairing
list. Also rejected: asking the sending gateway's owner, who does not own the
receiving workspace. Also rejected: probing the engine's health for the
capability before a spawn. That costs a hop, and the engine can change between
the probe and the spawn. The route itself is the capability.

## Consequences

A user pairs gateways once for outgoing access. Return access is a clearly
separate choice on the gateway granting it, with a single revocable control.
Source and packaged macOS/Linux installs follow the same protocol; no machine
names, default ports or install-specific trust are built into authorization.

[ADR 0723](0723-trusted-remote-workspace-client.md) separately replaces ADR 0591's
browser same-origin trust: A owns the remote-workspace frontend and B supplies
untrusted data/content. [ADR 0807](0807-trusted-pairing-serves-the-peer-client.md)
lets A's owner opt in to B's own frontend per pairing. That *trusted pairing*
is a separate toggle and grants no return access. Neither the browser relay nor the new operation protocol
confers automatic sibling-agent authority.

## Alternatives considered

- Reuse a paired device identity for agents: loses provenance and grants the
  wrong authority.
- Treat a gateway's pairings as automatic reciprocal or transitive agent access:
  violates the user's explicit directional boundary.
- Require another pairing for every return path: confers broader authority than
  the intended thread operation scope and complicates the workflow.
- Forward an engine origin token to another gateway: that gateway cannot verify
  it, and the token must remain inside its source-engine boundary.
