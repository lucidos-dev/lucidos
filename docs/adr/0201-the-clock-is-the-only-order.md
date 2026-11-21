# 0201: The transcript orders every row by when it happened, and a call writes each turn down as it ends

- **Status**: Accepted
- **Amended by**: [0206: A spoken reply covers a stretch of time](0206-a-spoken-row-is-an-interval.md), which restores `spoken_secs_before`. The write stays at the turn end and `created` stays the one append order.
- **Date**: 2026-09-16

## Context

The reader's rule is that every event renders chronologically, whatever its
source: caller speaking, talker speaking, doer stepping, user typing. Three
changes in a row failed to deliver it, and each failed in the same way.

The transcript did not sort by the clock. It grouped rows by *exchange*, keyed
on `request_event_id`, and sorted by time only INSIDE one exchange. The glossary
said so outright: the turn anchor "is a grouping key, **not** a clock". So
`spoken_secs_before` (ADR 0194), `callRowIndex` and `callRowTarget` each moved a
row within its box, while the boxes themselves stayed out of order.

A call also wrote its rows LATE. `write_down_the_reply` ran when the
conversation MOVED: the caller's next finished words, a new thing handed to the
talker, or the call ending (ADR 0188, corrected by ADR 0191). The hold had a
real reason. The provider ends a speaker's turn after 700 ms of silence, and a
row per turn drew one sentence as eight bubbles.

One reported call shows both defects at once. The talker's `Still` and `in it.
I'm pulling the current threads...` were said 180 ms apart and drawn as two
bubbles under two headers, with the caller's `, please` wedged between them.
That `, please` was said in the same breath as `Status`, and its row was written
twenty seconds later, at hangup.

## Decision

**The clock is the only ordering authority, and every row carries the truth.**

A call writes each provider turn down as that turn ends, so `created` is when
the words stopped. No accumulator, no second clock, and `spoken_secs_before` is
retired. The caller's words are always a `SpokenMessageReceived`, and the
talker's `WorkDelegated` is what starts a delegated turn.

In the transcript, a caller's utterance takes the running turn's continuation,
so every step reads under the words it followed. Rejoining the transcriber's
fragments into a sentence becomes a READING of the rows. One rule serves both
readers, the transcript and the doer's history, through a generated fixture.

## Rationale

**Sorting cannot fix a lie about the clock.** Two mechanisms were competing: a
grouping key that decided reading order, and a written-at stamp that was not a
happened-at. Each previous change corrected one row's placement and left both
mechanisms standing, which is why the defect kept coming back in a new shape.

**The hold broke a rule we already had.** Twenty seconds of speech living only
in `Call::spoken_so_far` is critical state in memory with no row behind it. A
crash mid-call loses the words outright, since no audio is kept. `CLAUDE.md`
§ Engine Statelessness forbids exactly that.

**A merge at render can use a bound; a turn-end decision cannot.** ADR 0187 and
ADR 0188 each proved a silence bound cannot decide when a speaker has finished.
That question is asked live, with no idea whether more words are coming. The
same number is safe here: the join runs afterwards, holding both timestamps, and
it only decides how words are grouped on screen.

**Starting a turn and holding one are two facts.** The caller's words start
nothing, because the talker decides whether the doer is wanted, and a row
written before that decision could not know. The card the caller's words open
still holds the turn once it continues there, which is why `Exchange.tookTheTurn`
exists.

## Consequences

- `created` is the happened-at for every row, so the transcript sorts by one
  field and no reader consults a second clock.
- A turn the caller talks across is drawn in several cards. The earlier one
  reads `interrupted`, which draws the continuation arrow rather than a Done
  check it has not earned.
- A tool RESULT still follows its call, by `tool_called_event_id`. That is
  pairing rather than placement: a step row and its outcome are one row.
- One breath is several rows in the store. That is the price of an honest
  clock, and the join is what the reader and the model both see.
- The gap bound (`MERGE_GAP_SECS`, five seconds) is a number, so a speaker who
  pauses exactly there gets two bubbles where they meant one. Survivable: it
  decides grouping only, and no words are lost either way.
- Rows written before this change keep their late `created` and read there.
  **Amended by ADR 0206**: the client applies `spoken_secs_before` again, so a
  row from the ADR 0194 window reads where it was said rather than at its late
  stamp.
- Supersedes ADR 0188, ADR 0191 and ADR 0194, which together built the late
  write and the second clock it needed.

## Alternatives considered

**Keep the late write and stamp every row with its age.** The talker's reply
already carried `spoken_secs_before`; extending it to the caller's row and to
the delegated `MessageReceived` was the smaller change. Rejected on three
counts. The second clock stays. Every new late-written row is another chance to
forget the stamp, which is precisely the reported defect. And the words still
live only in memory until the conversation moves.

**Keep the grouping key and keep patching placement inside each box.** This is
what the last three changes did. Rejected because a row whose turn anchor names
an earlier exchange cannot be placed correctly inside it, however good the
index rule is.

**Glue the fragments in the engine and write one row per sentence.** It keeps
one row per thing said and needs no merge in two languages. Rejected on the
undecidability above: knowing a sentence has ended is exactly what ADR 0187 and
ADR 0188 each failed at, and the hold it needs is the memory-only state.

**Merge only for the transcript, and let the doer read fragments.** Half the
work, and no contract test. Rejected because the model would read `Status` and
`, please` as two turns, which is a worse input than the reader's bad screen.

**Anchor a spanning utterance at its start and never cut it.** Considered for
the case where the caller interrupts mid-sentence. Rejected by the reader:
interleaved IS chronological, and with true timestamps nothing needs holding
whole. The two halves of that reply are 180 ms apart and simply land adjacent.
