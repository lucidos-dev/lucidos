# 0750: An agent edits an intelligence ladder only by spending the owner's edit-intelligence-ladder permission: Allow once or for this thread, never Always allow.

- **Status**: Accepted
- **Date**: 2026-11-20

## Context

The intelligence ladder binds every agent's model pick
([ADR 0500](0500-coding-agents-pick-from-an-intelligence-ladder.md)). So
only the owner wrote it: the override preferences are human-only, and the
edit routes took any caller that identified itself.

Claude Code can swap a model it cannot run for another one. The engine used
to drop that model from the picker and the ladder by itself, with no word to
the owner. An outage can be temporary, so the owner now decides. The agent
that saw the swap asks them, and should then make the edit they chose.

## Decision

A new permission action, `edit-intelligence-ladder`, covers every ladder
edit on every surface: set the rungs that are on, remove a model, sort, set
the default rung, and copy from the Lucidos Agent. Each edit route passes
the owner. An agent passes only by spending that permission. Its card offers
Allow once and Allow for this thread, never Always allow.

## Rationale

- **The owner stays the author.** A ladder edit changes what every later
  agent may run, so it needs the owner's say-so each time, on a card.
- **One action, one breadth.** The owner answers "may this thread edit my
  ladders", not one card per surface. Allow once still limits it to one edit.
- **No standing grant.** Always allow would let any thread rewrite the
  ladder without asking, which is the silent edit this decision removes.
  So nothing is stored past the thread, and the Permissions panel gets no row.
- **The established door.** A route with no live permission prepares the
  card and answers `permission_required`, as the event hook routes do (ADR
  0746). The CLI already reads that answer, so `lucidos intelligence-ladder`
  edit verbs print the request id to ask with.

## Consequences

- An agent with the permission may edit any surface fully. That is what the
  owner allows, and the card says so.
- Sort, copy from the Lucidos Agent, and `on_chat_ladder` on
  `PUT /api/v1/models` now refuse an agent with no permission. Before, any
  identified caller could use them.
- The override preferences stay human-only through `PUT /api/v1/settings`.
  An agent edits through the ladder routes alone.

## Alternatives considered

- **Keep the silent removal.** Rejected: it hides a decision from the owner
  and treats a temporary outage as permanent.
- **An ordinary question card, then the engine edits.** Rejected: an
  ordinary card's answer is the agent's to report, and no route could tell
  a real "yes" from an agent that skipped asking.
- **One action per edit verb.** Rejected: four cards for one decision, and
  no extra safety, since each verb is an edit to the same artifact.
- **Offer Always allow.** Rejected, as above.
