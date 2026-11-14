# 0691: Plan cards reserve an other-backend option and distinguish plan-only approval

- **Status**: Accepted
- **Date**: 2026-11-14

## Context

The approval checkpoint exposed only this thread's coding-agent backend, even
when another installed backend could build the plan. Landing a plan also looked
like landing code to event consumers. This amends ADRs 0486, 0497, 0499 and 0604.

## Decision

Read the other installed backend's ladder against the calling thread's reference.
Every other-backend rung includes backend, model and effort flags. Multiples are
comparable only when both references are present and equal. Automatic checkpoints
name other-backend floor and risk rungs, but take them only when the user, plan
or approval pick named that backend. The risk rung excludes the top.

Agents offer relevant choices through normal user questions, with backend
and rung split across follow-ups when useful. No exhaustive list is required.
Choices sharing a card reserve at most four slots. Priority is A, P, X, R, F, T.
A approves here; P commits the plan for later; X names the other backend.
R recommends a fresh session; F names a fork; T names this thread's rung.

Display order is R, X, T, A, P, F. Merge X into R when the recommendation
names the other backend. P exists only at plan approval.
A typed reply requests changes without consuming a slot.
Name an unallocated fork in the question as a typed reply.

`Commit plan for later` grants `plan_only`, permitting a plan-only landing while
keeping the source edit gate closed. Design approvals always have this scope.
Implementation approval is separate and must be obtained afresh after a plan-only
landing. A backend-only successor pick is an explicit pick.

The engine records plan start, proposal and approval facts, with explicit kind,
path, committed blob revision and approval scope. Proposal and approval marker
mutations commit atomically with their events. Replay never restores edit
permission. Change content is classified from proposed files or actual landed
Git trees, never approval state. Handoffs bind plan paths to the session's
repository and branch. These facts let consumers distinguish planning, ready to
build, implementation and code landing.

## Rationale

A common reference makes alternatives useful to compare. Reserving the other
backend and plan-only slots keeps those decisions visible instead of letting
optional recommendations crowd them out. Separating content from permission
prevents a plan-only Apply from being mistaken for implemented code.

## Consequences

Single-backend installations omit X. An installed CLI with no login may still
fail loudly when spawned; detection does not probe authentication. Explicit
binary-path preferences and PATH govern detection across supported platforms.
Existing events deserialize without backfill. The marker table remains Silent;
the append-only lifecycle facts announce user decisions. The Priority Board's
projection remains its own consumer's responsibility.

## Alternatives considered

- Keep only the current backend: hides an available implementation choice.
- Use approval state as change content: permission cannot prove what landed.
- Approve a plan for later with implementation permission: opens an edit gate
  the user did not authorize.
- Restore markers in event projections: replay could recreate permission after
  Apply consumed it. Live transaction mutations preserve atomicity without that.
- Reserve a request-changes button: duplicates the card's typed-reply path and
  displaces a concrete decision.
