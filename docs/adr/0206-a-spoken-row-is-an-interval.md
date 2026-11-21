# 0206: A spoken reply covers a stretch of time, so the transcript reads it where the talker BEGAN it

- **Status**: Accepted
- **Date**: 2026-09-17

## Context

A reader watched the talker answer a question. The bubble went up above the
turn's first step, then jumped below two steps as the engine's row landed.
Reported as "I think the speech was before the Recalled step first, then the
order was re-arranged. Should keep same (chronological) order".

The event log for that call:

| Time (UTC) | Event |
|---|---|
| 13:07:42.742 | `SpokenMessageReceived`, the caller's question |
| 13:07:42.759 | `WorkDelegated`, so the doer's turn starts |
| ~13:07:43.3 | the talker STARTS saying `Good idea. Okay, I'll put that to a coding agent. I'm on it.` |
| 13:07:44.282 | `MemoryRecalled`, drawn as `Recalled 25 memories` |
| 13:07:46.923 | `SpokenReplyGenerated`, the row for those words |

ADR 0201 made the write honest. Each provider turn is written down as that turn
ends. So `created` is when the words stopped, and no row waits on a move of the
conversation. It also concluded that one stamp was therefore enough.

It is not. A spoken row covers a STRETCH of time, and every step beside it is
an instant. Ordering an interval by its end files it after everything that
happened while it was being said. The error is bounded by one provider turn,
one to five seconds, which is exactly the window a turn's opening steps land
in.

## Decision

**`SpokenReplyGenerated` carries `spoken_secs_before` again: how long the
talker had been speaking when the row was written.** The transcript places the
row at `created` minus that age, and the live bubble the client draws is placed
by the same rule.

The engine holds the words and their first moment as ONE value, so a row cannot
be written without its stamp.

## Rationale

**The row records two moments and carried one.** When the words stopped is an
append-order fact the whole log depends on. When they started is what a reader
is reading. ADR 0194 said this first and was superseded for a different reason:
its late write, which ADR 0201 removed. The second stamp went with it.

**An age rather than an instant**, for the reason ADR 0194 gave and ADR 0053
requires. `created` is stamped by Postgres and the engine's wall clock is
another clock. The age comes off the monotonic clock and belongs to neither.
Subtracting it keeps the answer on the clock every step is ordered against.

**One value in the engine, so the stamp cannot be forgotten.** ADR 0201's
objection to an age field was that every new late-written row is another chance
to leave it off. `SpokenTurn` answers it structurally: the words and their first
moment are pushed and taken together, and nothing can take one alone.

**A step that lands mid-sentence separates nothing.** The transcript merges two
spoken rows only when they are adjacent, because anything between them means
the reader met something in between. A row now spans time, so a step stamped
INSIDE one is not between anything: the talker never stopped. Without that
reading, one sentence would draw as two bubbles around the step, and the live
bubble would split in two as the rows landed.

## Consequences

- **A spoken row is placed by `happenedAt`, and so is every step it is compared
  against.** One rule, read the same way on both sides of the comparison.
- **The swap from the live bubble to the engine's row moves nothing** on a
  synchronised clock. The live row's own `created` is when the bridge drew it,
  which is the same question.
- **A row with no age reads at `created`**, which is where every reply read
  before this. That covers a provider that streamed no deltas, the session
  marks, and the rows written between ADR 0201 and here.
- **A merged bubble reads where its FIRST fragment began.** The merge advances
  `created` to the newest fragment, so the age grows to span both.
- **A step really can read below words that were still being said as it
  happened.** That is the price of drawing one bubble for one sentence, and it
  matches what the caller heard begin.
- **A live row is still anchored on the browser's clock.** On a skewed device
  the bubble can move by a step as the engine's row lands. Bounded, and it
  corrects to the engine's answer.
- **The caller's own row is unchanged.** `SpokenMessageReceived` is written at
  their turn end and is an exchange BOUNDARY, so re-placing it means reordering
  exchanges. Named a residual by ADR 0194 and ADR 0201, and still one.
- Amends ADR 0201, which stands in every other respect: the write stays at the
  turn end, and `created` stays the one append order.

## Alternatives considered

**Leave it, and let a spoken row read at `created`.** The error is only a few
seconds now that the write is honest. Rejected because the reader reported it:
those seconds are exactly when a turn opens its first steps, and the bubble
visibly jumps as the row lands.

**Back-date `created` to when the words began.** Then one stamp would do.
Rejected for the reason ADR 0194 gave. The client's incremental fold requires
`created` to be monotonic. Every spoken row would force a full regroup, and
would reorder against the caller's row.

**Keep the live bubble where the engine's row will land.** Client-only, no event
change, and nothing re-arranges. Rejected by the reader's own words: the order
they asked for is the chronological one, and this delivers stability by making
the live bubble wrong too.

**Remember, client-side, where the live bubble was drawn.** It covers a call
being watched and costs no event change. Rejected because a reloaded transcript
has no such record, so the same call would read two different ways.

**Merge two fragments whatever lies between them.** It would keep a sentence
whole in every case. Rejected: a doer step between two things said is time
passing, and the reader must see the second half below the work it followed.
