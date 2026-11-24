# 0761: Prefer the sub-thread read rule and treat successor handoffs as work in flight

- **Status**: Accepted
- **Date**: 2026-11-22

## Context

[ADR 0607](0607-sub-thread-read-decision-knows-the-parent-acts.md) made routine
reports from sub-threads that wake their parent a read no. Coding agents still
received a system prompt telling them to say yes after a review. The rule in
their session context had lower priority.

The fallback judge saw awaited sub-threads and event waits as work in flight.
It could miss that a coding-agent turn was handing off to a successor session.

## Decision

The generic coding-agent rule defers to the sub-thread read rule when present
in the session context. That rule treats a review verdict as routine and says
the parent asks the user for approval.

The judge gets an in-flight note when the turn recorded a handoff that starts
automatically and remains valid. It checks for a withdrawal and counts events
after both the previous idle and the session's own start. The note tells the
judge that a successor session carries the work on.

## Rationale

A fixed pointer gives the session rule precedence and keeps each prompt flavor's
cache prefix shared. The handoff event survives the successor starting, which
can happen before the judge reads the turn.

## Consequences

The prompt guidance applies to Claude Code and Codex in every workspace. The
handoff note also applies to top-level coding-agent turns. The judge uses its
existing in-flight criteria from [ADR 0424](0424-read-decision-knows-work-in-flight.md). An agent's recorded
decision remains authoritative.

## Alternatives considered

A thread-specific system prompt would split the shared cache prefix. Checking
for a pending handoff would miss a handoff after its successor starts.
