# 0753: Unattested owner-only acts fail immediately. Use the Lucidos app or run the job through a trigger

- **Status**: Accepted
- **Date**: 2026-11-21
- **Amends**: [0647](0647-only-a-signed-claim-proves-the-owner.md) and
  [0688](0688-every-allow-once-card-is-a-permission-request.md)
- **Supersedes**: [0711](0711-allowing-an-unattested-start-adopts-the-thread.md)

## Context

The unattested caller path parked an exact HTTP request or coding-agent start
on an owner's card. A request with no thread of its own posted to Home.

## Decision

An unattested caller's owner-only act, including a coding-agent start, fails
immediately with HTTP 403. The response directs the caller to the Lucidos app
or to a trigger so the job runs as an agent.

## Rationale

A permission card belongs to the thread whose work waits on it. An external
script has no thread to resume, and replaying its HTTP request requires a
separate authority path. The app and a trigger give the operation a signed
caller through existing entry points.

## Consequences

Reads and ordinary chat remain available. Pairing retains its recovery path.
The engine removes the replay and start-card paths and drops their spent-event
index. The rule applies in every workspace on macOS and Linux.

## Alternatives considered

- **Keep the exact-request card in Home.** Rejected because Home would hold a
  card for work happening outside its thread.
