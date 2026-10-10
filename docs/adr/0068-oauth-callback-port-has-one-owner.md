# 0068: The OAuth loopback callback port has one process-level owner, and starting an authorization supersedes the abandoned one

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

An OAuth authorization code arrives back on a loopback HTTP listener the engine
binds for the duration of the flow. The port is fixed at 14981 rather than
ephemeral. The redirect URI has to be registered with the provider ahead of
time, and a registered URI cannot carry a port chosen at run time.

A fixed port makes "at most one authorization in flight per engine" a fact about
the world rather than a policy anyone chose. Two places start a flow:
`prepare_oauth_flow` behind the Settings buttons, and `run_oauth_flow`. The
agent's `connect_oauth_account` tool reaches the second one without passing
through the API layer at all.

The waiter used to be a `tokio::spawn` whose `JoinHandle` was dropped on the
spot. Dropping the result `oneshot::Receiver` does not cancel a task. So an
abandoned authorization held the port for its whole 120 second timeout, with
nothing able to reclaim it. Every retry inside that window died at the bind with
a bare `Address already in use`.

## Decision

The callback port's owner is a process-level slot in `core::oauth`
(`ACTIVE_CALLBACK_FLOW`). It holds the waiter's `JoinHandle` plus an atomic flag
saying whether that flow still holds the socket.

Starting an authorization always supersedes the one in the slot. The starter
takes the slot's lock, releases the previous flow, binds, spawns, registers
itself, and only then drops the lock.

Releasing distinguishes two states. A flow still holding the port is aborted
**and awaited**, so its listener's file descriptors are closed before the caller
binds. A flow that has already released the port is detached rather than
aborted: it is not in the caller's way, and killing it would throw away an
authorization the user completed.

## Rationale

**The owner belongs next to the code that binds the port.** Exclusivity here is
imposed by the provider registration, not by any one caller's structure. So the
slot is scoped to the thing that is genuinely singular: the process.

**Awaiting the abort is the whole point.** `JoinHandle::abort` only requests
cancellation, taking effect when the task next yields. Returning straight after
it would let the caller's `bind` race the socket's close, reintroducing the
`EADDRINUSE` the slot exists to prevent.

**Task liveness is not the same question as port ownership.** A flow's task
outlives its hold on the socket. `wait_for_oauth_callback` takes the listener by
value, so the port is free the moment the callback lands or the timeout fires.
The token exchange, the userinfo call and the account write all run with the
port already released. Keying the supersede on the task alone would abort that
tail for a port nobody was waiting on. Worse, it could land between the account
row committing and its `OAuthAccountConnected` event.

**Superseding, rather than refusing the new flow**, follows from what the two
flows mean. The user pressing Connect is stating what they want now. The older
flow is by construction one whose browser tab they walked away from.

## Consequences

- A retry after an abandoned authorization binds immediately, instead of waiting
  out a 120 second timeout it cannot see.
- A superseded caller's result channel closes with no result. Both entry points
  report that with one shared message (`FLOW_SUPERSEDED_MSG`), because a
  supersede is deliberate and must not read as a fault.
- `state` (RFC 6749 §4.1.2) becomes load-bearing beyond CSRF. A redirect from a
  superseded flow can reach the listener of the flow that replaced it, and
  without the nonce the two are indistinguishable.
- An `AddrInUse` at the bind now means a *different* process holds the port,
  most often another Lucidos workspace part-way through connecting an account.
  The error says so.

## Alternatives considered

**An ephemeral port per flow.** Removes the exclusivity entirely, and is what a
web app would do. Not available: the provider only redirects to a URI registered
in its console, and every such URI names a port.

**A field on `AppState`.** Covers the Settings buttons and misses
`run_oauth_flow`, which the agent reaches directly. The Backup page's own "Ask
Lucidos to set this up" button invites exactly that path. A slot half the
callers bypass is worse than none, because it reads as protection.

**Refuse a new authorization while one is in flight.** Correct-looking and
hostile in practice. The common case is a user who closed the consent tab, and
telling them to wait two minutes explains nothing they can act on.

**Abort every previous flow unconditionally.** Simpler by one atomic, and it
cancels redemptions the user already consented to. The window is not small: the
token exchange plus userinfo is a pair of network round trips.

**Reclaim by polling the port instead of tracking the owner.** Cannot tell our
own abandoned flow from another workspace's live one. It would either steal a
neighbouring workspace's authorization, or keep the failure it was meant to fix.
