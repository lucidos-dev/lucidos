# 0741: An integrated sub-thread reports decisions and options to its parent

- **Status**: Accepted
- **Date**: 2026-11-19
- **Amends**: [0465](0465-integrated-sub-threads.md)

## Context

Integrated sub-threads parked on owner question cards for decisions their
parent already owned, including which thread should fix a finding. A parked
child could not receive the parent's answer as a regular message.

## Decision

The shared Claude Code and Codex question endpoint refuses ordinary owner
cards for integrated sub-threads. The child runs `lucidos wake-parent yes`
and ends its turn with the decision and options in its report. The parent
can resolve it by messaging the child. The Claude Code Stop hook skips its
prose-question redirect for these sessions.

## Rationale

The integrating parent owns the combined work and can answer in the context
of the whole change. It can ask the owner when a decision calls for their
input. For a new plan, the parent obtains implementation approval for the
committed plan on its branch. The child inherits that approval when it marks
the same plan revision again.

## Consequences

Independent sub-threads retain owner question cards. Prepared permission
requests retain their owner approval flow, since only the owner can grant
that authority.

## Alternatives considered

Forwarding a child's card to its parent would require a second blocking
question flow and an answer relay. The report and reply use the existing
parent wake and messaging paths.
