# 0259: A thread waiting on the user cannot be archived, by any path

- **Status**: Accepted
- **Date**: 2026-09-23

## Context

Three trigger threads sat archived while waiting on a question. The user never
saw the questions: an archived thread drops out of Current and out of the Needs
attention view.

Nobody archived them. The trigger runs were unattended (`go_to_review` false),
and the lifecycle contract hides an unattended run when it lands in the inbox.
The agent's question moved the thread to the inbox, and the guard at once
rewrote that to Archived. No `ThreadArchived` event was ever written.

A second path existed on purpose. `POST /api/v1/threads/archive` admitted a
parent in `WaitingForUserAnswer`, cancel-stamped its question card, and archived
it. Two tests pinned that as a feature.

## Decision

A thread in `WaitingForUserAnswer` needs attention, so nothing archives it.

- The unattended guard skips the events that park a thread on the user
  (`WAITING_FOR_USER_ANSWER_EVENTS`).
- The archive endpoint refuses a parked parent with 409
  `parent_not_archivable`, as delete already did.
- The EventBus refuses `ThreadArchived` on a parked thread, whoever emits it.
- A migration moves threads already stuck this way back to the inbox.

To drop a question the user no longer wants, they answer it or press Stop. Stop
cancels the card on chat and coding-agent threads alike, and then Archive works.

## Rationale

A question is the one state where the agent cannot move without the user. Any
path that hides it turns a pause into a silent stall. The cost is small: one
extra tap (Stop) before an archive the user really wants.

The invariant lives in the stored `archive_state`, not in the display.
`display_section` is only one of its readers. The Archive count badge, the
drawer's archived window, the voice sections' open-question list and
`available_thread_actions` all read the column directly. A display-only rule
would put a thread in Current while the Archive badge still counted it.

The existing `to_inbox` transition on the question event already moves an
archived thread back out, so no new event is needed. Only the guard was undoing
it.

## Consequences

- An unattended trigger run that asks a question shows in Current and Needs
  attention until the user answers or stops it. It still hides when it
  finishes normally.
- Archiving a parked thread from any client now returns 409. The frontend
  never offered Archive there, since `available_thread_actions` hides it while
  the thread is live.
- A cascade skips a member that parks on a question after the family lock
  committed, instead of cancelling that fresh question for the user.
- The migration makes the projection agree with its events: replaying the
  stuck threads through the fixed contract also yields `inbox`. It needs no
  count rebuild, because `is_blocking` and `is_attention_needing` count a
  parked thread whatever its archive state.
- A failed unattended run still hides in Archive. That is a different rule, and
  the scheduler sends an error notification for a failed trigger.

## Alternatives considered

- **Surface a parked thread through `display_section` alone.** Rejected: the
  other readers of `archive_state` would disagree with the drawer, as the
  Rationale lays out.
- **Keep archive-while-parked, and cancel the question.** This was the old
  contract. Rejected: it answers the question on the user's behalf and hides
  that it was ever asked.
- **A new "unarchive" event for the stuck threads.** Rejected: no user action
  happened, and the question event already carries the transition. Writing an
  event would record a decision nobody made.
