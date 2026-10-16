# 0493: A plan card's new-thread pick is the owner's consent to the successor: amends 0387 and 0474

- **Status**: Accepted
- **Date**: 2026-10-16
- **Amends**: [0387](0387-an-owner-approval-authorizes-one-act.md), [0474](0474-successor-threads.md)

## Context

A child coding-agent thread, spawned by a watch thread, finished its plan. The
owner picked `Implement in a new thread: Sonnet 5.5, high` on the plan card.
The agent then ran `lucidos successor spawn` and got a 403. A successor of a
child sits under the parent, so ADR 0474 gates it as `spawn-successor` on that
parent. The watch thread, not the owner, had opened the turn.

The agent raised an owner approval card (ADR 0387). The owner tapped Allow
once. Claude Code answered "The user did not answer the questions." Two faults
were stacked:

1. **The answer was lost.** `build_hook_answers` keyed each answer by the
   trimmed question text. The owner approval card's question ends in a
   newline, because every quoted reason line gets one. Claude Code looks each
   answer up with `Object.hasOwn` on the exact string it sent. So every owner
   approval card from a Claude Code thread was lost, for every verb.
2. **The pick was already consent, but nothing read it.** The owner had named
   the act on the plan card. A second card asked them for the same act again.

## Decision

- The hook's answer map keys on the raw `question` string. The card still
  shows the trimmed text.
- A registered device's `Selected` pick of an option whose label starts with
  `Implement in a new thread` is a live owner approval for one
  `spawn-successor`. `live_owner_approval` reads it beside the ADR 0387
  approvals. The spend goes through the same `OwnerApprovalSpent` and its
  unique index on `tool_use_id`.
- The answer route refuses a pick of such an option unless the owner makes it
  from a registered device. It does the same for any answer to an owner
  approval card. The lookup reads only the answer's actor kind, so the route is
  what makes "device" mean the owner. Every other answer to the plan card stays
  open.

## Rationale

ADR 0387 refused magic labels, because a vague card ("Keep going?") could lift
a thread to every verb. This label names one verb, and the grant covers only
that verb. The owner read the act they allowed.

Everything else ADR 0387 holds still holds. The grant is one spend, for the
asking thread, in the turn that offered the card. Only a device's `Selected`
answer counts. A typed reply, an agent's answer, a parent thread's answer, a
cancel and a supersede grant nothing.

The pick matches `spawn-successor` with any target. A successor always sits
under the caller's own parent, and the successor route computes that target
itself. The pick never matches another verb.

The CLI offers the label and cannot import the engine. So the engine owns the
prefix constant, and a pin test reads the CLI's option text.

## Consequences

- A child thread's plan card hands the build off with no second card.
- A typed reply naming another implementer still meets the ADR 0387 card. That
  card now works.
- The agent words the option, so it could word one outside a plan card. The
  grant stays one successor for the asking thread, and only the owner can pick.
- The grant is not bound to the implementer the label names. The agent passes
  the implementer flags itself, and the route accepts any implementer the
  backend offers. A pick consents to a successor, not to one model.

## Alternatives considered

- **Key the answer map by both strings.** A second key invites a lookup that
  matches the wrong one. Claude Code echoes the exact string, so one key is
  enough.
- **Trim the question text when the engine words the card.** That fixes this
  card and leaves every agent question with stray whitespace broken. The
  engine cannot know what an agent will send.
- **Have the planning agent always raise an owner approval card.** It asks the
  owner for what they just chose.
- **Let any option grant any verb.** ADR 0387 rejected this, and this decision
  keeps the line: one verb, named by the label.
