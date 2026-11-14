# 0197: A spoken reply claims no liveness, because nothing can tell the client the reply is over

- **Status**: Accepted
- **Date**: 2026-09-16

## Context

The transcript drew an animated three-bar mark after the talker's words while a
reply was in flight. A reader reported it on a reply that had finished: the turn
header read `Done`, the sentence was whole, and the mark was still moving.

That is not a rendering slip. The live row is retired by the engine's own
`SpokenReplyGenerated`. ADR 0188 writes that row at the next MOVE of the
conversation, and ADR 0191 confirmed a pause is not one. So the row stands with
its finished sentence until the caller speaks again, which on a quiet line is
indefinite.

Asked what the mark should mean, the reader was clear: not when the talker has
finished, not while it is mid-sentence either, only where we know more is
coming. They chose to have it never drawn on the talker's row.

## Decision

The talker's spoken reply carries no liveness, in a mark or in its accessible
name. The live row and the engine's landed row render identically.

The caller's own bubble keeps the mark, through both of its states.

## Rationale

**The client is never told a reply is over.** It IS told the talker paused:
`talker_turn_ended` flips the phase from `speaking` to `listening` and empties
`said`. That is a different fact.

ADR 0191 measured holes of 3.6 seconds inside one reply. A mark gated on the
phase therefore blinks several times through one bubble. A mark gated on the
row's existence stays lit after the last word. Neither is the thing a reader
wants.

**The caller's side is different, and that is why it keeps the mark.** There the
client learns the words are final: the provider ends the turn, `heard` replaces
the partial, and the row settles. The signal is available, so it is honest.

**Two marks for one meaning was the earlier shape, and it was worse.** A
blinking caret stood here before the bars, and a text cursor is a typing
metaphor on a surface where nobody types. One mark, on the side that can support
it, is the end of that.

## Consequences

**The swap from the live row to the engine's row moves nothing**, since the two
now render the same. That is a property worth keeping: the retirement lag ADR
0194 documents costs the reader nothing on screen.

**A reply in flight has no in-flight signal except its own growing text.** That
is the deliberate trade, and it is what the reader asked for. The call toggle's
status region still speaks `Speaking` to a screen reader, off the same phase
flip, so the state is not unannounced.

**Do not re-add a mark gated on the call phase.** It is the obvious repair and it
ships the flicker this ADR rejected.

## Alternatives considered

**Retire the live row when the talker's turn ends.** Then the row's existence
would mean "still speaking" and the mark could stay. Rejected on two counts. A
pause is not the end of a reply (ADR 0191), so the bubble would vanish and
return several times inside one sentence. And the caller is still hearing the
tail of audio the provider has already stopped generating.

**Gate the mark on `said !== ''`.** Same defect, one layer up: `said` empties at
every hole in the words.

**Keep the mark only while the talker holds the floor.** Rejected by the reader
directly, and it is the same phase gate as above.

**Keep it, and accept the lie on a finished reply.** Rejected: that is the
report.
