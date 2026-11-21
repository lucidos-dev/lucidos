# 0409: Blocked, Review and In flight; Review lists read requests and lands on its target

- **Status**: Accepted
- **Date**: 2026-10-10
- **Amends**: [0394](0394-drawer-ongoing-grouping-fixed-order.md)

## Context

A thread leaves the Ongoing grouping the moment its turn ends, unless it
failed, asked a question or holds a proposed change. So a chat or trigger
thread that finished with a report the user has not read is listed only under
Folders, among every quiet thread. The user wanted those threads in Ongoing.

Not every finished reply needs reading. An acknowledgement or a plain "done"
does not. Only the thread knows whether its reply holds what the user asked
for.

## Decision

- **A thread asks to be read.** Its agent calls the `request_read` tool, or a
  coding agent runs `lucidos request-read`. That sets a **read request** on the
  thread, stored on its summary and shared by every device.
- **Review lists it.** Review widens from "a change to apply" to "your turn to
  look": a ready change, or a read request, once the turn has ended.
- **The request clears** once the end of the latest reply has been on screen
  for the seen dwell. The user's next message or an archive also clears it.
- **Needs attention is renamed Blocked.** Its members and badge stay.
- **The tiles read Blocked, Review, Drafts, In flight.** Drafts moves from
  first to third.
- **Opening from Review lands on what to look at**, as Blocked already did. A
  ready change lands on the change's turn, a read request on the newest turn.
  The target follows the tile the row was tapped in.
- **A read request's row wears the filled dot, and a ready change a diff
  glyph.** A filled dot reads as "unread" almost everywhere, so the change gave
  it up.

## Rationale

The thread decides because only it can tell a report from an
acknowledgement. A rule per thread type would flood Review with every trigger
run that found nothing.

Inside Review, not a fifth tile: a reply to read and a change to apply ask the
same thing of the user, a look. The three live groups then answer one question
each. Blocked: it cannot go on without you. Review: it finished, and it is your
turn. In flight: it goes on without you. "Needs attention" blurred the first
two, since a finished report also needs attention.

The new order puts what others need from you first. The grid's top row is
Blocked and Review. Its bottom row is your own draft, and work that needs
nothing from you. ADR 0394 led with Drafts as the cheapest thing to finish. But
you wrote the draft, so you already know about it, while a blocked or finished
thread is news.

A read request clears on seeing, not on opening. Opening a thread lands on the
start of the reply, and the end has to come into view before it counts as read.

## Consequences

- A read request never counts as attention. The engine's
  `is_attention_needing` and `attention_descendant_count` keep their names and
  their set, which already differed from the tile's.
- A read request makes a trigger run attended, as "Send directly to Archive"
  off does. Its turn end then moves it to the inbox, so a trigger thread that
  found something is not hidden in Archive.
- A device that stored the Needs attention group opens Blocked.
- A read request is not a notification. It neither pushes nor toasts. An agent
  that also wants to interrupt the user calls `send_notification` as well.

## Alternatives considered

- **An Unread tile for every finished thread the user has not seen.**
  Rejected: most finished replies need no reading, so the tile would fill with
  noise. It is also close to the Idle group ADR 0394 removed.
- **Keep a finished thread in In flight until it is seen.** Rejected: In
  flight means "not finished", and the row would lie about its state.
- **Fold read requests into Needs attention.** Rejected: every report would
  light the badge, which then stops meaning "blocked on you".
- **The agent asks a question to be read.** Rejected: a question parks the
  thread and demands an answer that carries no decision.
- **A separate To read tile.** Rejected for now: Review already means "look at
  this", and four tiles keep the 2x2 grid of ADR 0401.
