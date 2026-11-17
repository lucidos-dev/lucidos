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

## Consequences

A user pairs gateways once for outgoing access. Return access is a clearly
separate choice on the gateway granting it, with a single revocable control.
Source and packaged macOS/Linux installs follow the same protocol; no machine
names, default ports or install-specific trust are built into authorization.

[ADR 0723](0723-trusted-remote-workspace-client.md) separately replaces ADR 0591's
browser same-origin trust: A owns the remote-workspace frontend and B supplies
untrusted data/content. Neither the browser relay nor the new operation protocol
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
