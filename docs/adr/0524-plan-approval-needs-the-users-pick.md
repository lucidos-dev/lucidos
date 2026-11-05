# 0524: A plan approval needs the user's own pick; the engine reads it before planned approve flips the marker

- **Status**: Accepted
- **Date**: 2026-10-19

## Context

`lucidos planned approve` flipped a `proposed` plan marker to `planned` for any
Lucidos-spawned caller. Nothing checked that the user had approved. The original
design chose this on purpose: a light-touch, prompt-level gate with the same
trust model as `--simple`.

A coding agent then asked the user a typed question about the plan, read the
reply as consent, and ran `planned approve`. The user had asked, not approved.
Guidance alone did not hold: the agent wrote the card, the flip and the reading
of the answer.

## Decision

`approve_plan` finds the thread that owns the branch and reads the newest
device input on it after the marker's `planned_at`. It approves only when that
input is a tapped option other than `Request changes`. A typed reply approves
only as a plain approval from a small closed set (`approve`, `yes`, `lgtm`). Any other
case answers 409 with reason `no-plan-approval`.

A re-mark resets `planned_at`, so an answer given before it never counts. A
branch with no owning thread, such as a human's own, approves as before.

The plan summary also moves onto the card. A message that also calls a tool
never reaches the user, so the card's question carries the summary and a
re-ask carries what changed.

## Rationale

The evidence is already in `events`. The owner-approval gate (ADR 0387) set the
precedent: an answer from a device is evidence of the user, where an answer
from an agent's origin token is not. Reading it at the flip makes the rule hold
where the guidance had not.

A closed approval set keeps a question or a suggestion from classifying as
consent. A fork's label is the agent's own words, so the tapped-option rule is
open and `Request changes` is the one label that never approves.

## Consequences

- This reverses the light-touch choice in
  `docs/plans/2026-06-19-plan-approval-step.md`. `--simple` and the bounded
  security-fix lane stay agent-recorded (ADR 0154).
- **Residual:** any card answered after the re-mark counts, because the
  approval card is not tagged. An agent could ask an unrelated question after
  marking and treat its pick as approval. Closing that needs a tag on the
  approval card, which the checkpoint does not write today.
- **Residual:** a device header is attribution, not authentication (ADR 0192).
  An agent that forges a device id on its own HTTP call to the answer route
  records a device answer, and the gate reads it. The gate stops the honest
  misreading that caused the incident, not a deliberate forgery.
- An unreadable answer refuses, since a failed query is unknown, never consent.
- `planned approve` cannot be exercised end to end from a coding-agent thread
  without a real user answer on that thread.

## Alternatives considered

- **Guidance only.** Rejected: the incident happened under guidance.
- **Tag the approval card and match the answer to it.** Rejected for now: it
  closes the residual but needs a new field on the checkpoint's card.
- **Free-text classification by a model.** Rejected: a judgment call on the one
  decision that must be deterministic.
