# 0604: A plan-only branch whose plan awaits approval is held, never proposed, amending 0397

- **Status**: Accepted
- **Date**: 2026-10-31

## Context

ADR 0397 made a plan-only branch the one exception to the plan floor. When
every landing file sat under `docs/plans/`, a `proposed` marker cleared the
floor, so the plan could land as a record.

In practice an agent commits a plan, records it, and asks for approval. The
user often answers with a question. The turn ends with the marker still
`proposed`, and the engine proposed the branch. The user saw an Apply button
for a plan they had not approved.

## Decision

A plan-only branch with a `proposed` marker is held at turn end, like a mixed
branch. It becomes unproposed work with reason `plan_awaiting_approval`. Apply
refuses it with the same message. Propose and Apply still ask one function,
`change_ops::PlanFloor`.

A plan-only hold sends no re-entry nudge. A mixed branch with a `proposed`
marker keeps its nudge.

## Rationale

The plan is a proposal, and approval is the user's decision. A card with Apply
lets one tap land a plan nobody approved.

A nudge would push the agent to re-ask while the user is mid-discussion. It
would also re-open an unattended child that already ended with
`BLOCKED ON PLAN DECISION:` (ADR 0154). The agent has done its part, so the
next move is the user's.

## Consequences

- An approved (`planned`) plan-only branch is unchanged. It proposes, skips
  hardening (ADR 0416), and Apply consumes the marker.
- A plan lands with the work that implements it, or on its own once approved.
- A rejected or dropped plan never lands. It stays on its branch, and the
  archive net sets it aside on archive (ADR 0328).
- A plan-only change already pending keeps its card. Apply refuses it until the
  plan is approved. There is no boot sweep.
- The unattended blocked lane does not depend on a pending change. The
  orchestrator reads the reply line and the change state.

## Alternatives considered

- **Nudge the agent once for a plan-only hold too.** It would re-ask during the
  user's discussion and re-open a correctly ended unattended child. Rejected.
- **Propose the plan with Apply disabled.** That is a card state for work that
  is not ready, which ADR 0397 already rejected.
- **Keep the exception and fix the agent's flow.** Agents cannot know the user
  is still deciding. The engine owns the gate.
