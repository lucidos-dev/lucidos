# 0834: Route integrated sub-thread attention through the parent

- **Status**: Accepted
- **Date**: 2026-11-27
- **Amends**: [ADR 0607](0607-sub-thread-read-decision-knows-the-parent-acts.md) and [ADR 0465](0465-integrated-sub-threads.md)

## Context

An integrated sub-thread hands its branch to its parent. Its own read request or missing hardening marker could still put it in Review or Blocked, splitting attention between the parent and child.

## Decision

An integrated child's yes on an agent-opened turn records the decision without projecting a read request. A yes on a user-opened turn can still request a read. An integrated child's hardening hold alone stays out of Blocked. It prevents hand-up and wakes the parent with a failed report and retry instructions, including in no-wake mode.

## Rationale

The parent owns integration and can act on the failed handoff. A direct user interaction or a permission request still belongs in the thread where it happened.

## Consequences

The drawer uses the durable integration flag in both hydrated and live summaries. The child retains its hardening strip. Existing unread requests are left in place and previously delivered reports are not sent again.

## Alternatives considered

Hiding every integrated child in the drawer would also hide direct replies, permission requests, and explicit stops. Clearing unread after the parent receives the report would allow a transient Review entry and could clear an unseen reply to the user.
