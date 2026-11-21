# 0749: Hardening gates finish before compilation, with exclusive observable startup

- **Status**: Accepted
- **Date**: 2026-11-19

- **Amends**: [0694](0694-harden-round-in-minutes.md)

## Context

Independent failures must surface before compilation consumes the build slot.
The longer startup barrier also needs exclusion and observable liveness.

## Decision

Complete independent gates and selected independent suites before compilation.
Hold an exclusive startup lock and publish startup status outside suite evidence.
Record the commit the gates checked. Verdict reruns gates after any commit change,
including safe-path docs, and refuses a tree that changes during verification.
A selector failure stops and joins this shell's background jobs before returning.
Static lint retains knowhow checks alongside the startup barrier.

## Rationale

A failure should consume no compiler capacity. Publishing only after the barrier
lets concurrent starts race and lets wait return an older verdict.

## Consequences

Static checks no longer overlap the Cargo lane. Startup refusal preserves prior
evidence, while wait and verdict report failure. Stop reaches the startup tree.

## Alternatives considered

Publishing only the suite PID after gates leaves startup invisible.
Deleting evidence before gates loses useful retry obligations on refusal.
