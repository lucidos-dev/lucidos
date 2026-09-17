# 0194: A spoken row reads where it was said, not where it was written

- **Status**: Superseded by ADR 0201
- **Date**: 2026-09-16

## Context

A reader watched the talker say `I'm on it, give me a sec.` as a doer turn
started. The transcript then drew that bubble under fifteen seconds of the work
it had promised. The event log explains it exactly:

| Time (UTC) | Event |
|---|---|
| 12:03:04.349 | `MessageReceived` `What is it waiting an answer for` |
| ~12:03:05 | the talker SAYS `I'm on it, give me a sec.` |
| 12:03:19.325 | `SpokenReplyGenerated` `I'm on it, give me a sec.` |

ADR 0188 made one reply one row, and ADR 0191 kept that row from being spent at
a pause. Both are right, and together they mean a reply is written at the next
MOVE of the conversation: the caller's next finished words, a new thing handed
to the talker to say, or the call ending. So a row's `created` is the moment it
was recorded, which can be a whole turn after the caller heard it.

The transcript sorts an exchange's steps by `created`. A stall therefore reads
under every step the turn emitted while the row waited.

## Decision

`SpokenReplyGenerated` carries `spoken_secs_before`: how long before the row the
talker began the stretch. The transcript places the row at `created` minus that,
and the live bubble the client draws is placed by the same rule.

## Rationale

**The row records two different moments and only ever carried one.** When it was
written is an append-order fact the whole event log depends on. When it was said
is what a reader is reading. Conflating them is the defect, so the row states
both.

**An age rather than an instant**, and ADR 0053 is the reason. `created` is
stamped by Postgres, and the engine's wall clock is a different clock. A
comparison across the two is a coin flip that lands wrong exactly when the
machine is loaded. The age is read off the monotonic clock and belongs to
neither. Subtracting it from `created` puts the answer on the same clock as
every step it is ordered against. The first draft of this change used an
absolute instant, and its own test caught the skew within the hour.

**The field is optional, and absent means today's placement.** A session mark
was never said, and a row written before the field existed cannot say when it
was. Both keep the end of the steps, so no historical call re-reads itself after
an upgrade.

**The live bubble takes the same rule, not a second one.** Two rules drift, and
the drift a reader sees is a bubble jumping between steps as the engine's row
lands.

## Consequences

**A turn's header timestamp gets more honest for free.**
`exchangeResponseTimestamp` reads the last step's `created`, and a late spoken
row was that step. It no longer is.

**A call row is spliced into an exchange's steps rather than appended.** Every
backward walk over those steps is predicate-based, and none matches a spoken
reply, so nothing else reads position. A new reader of `Exchange.steps` must not
assume the array is seq-ascending.

**The placement rule reads every step by itself.** A spoken row already filed
carries its write time in `created`. A walk reading that raw would make it a
wall the next row stops at. It also refuses to land inside a run of streamed
text: `TextStreamed` persists per delta, the renderer merges only adjacent ones,
and a row dropped mid-run cuts one answer in two.

**A live row is still anchored on the browser's clock**, which is a third one.
Nothing on the wire carries a moment during a call, so there is no better
anchor. The error is the device's clock skew, NOT one step: a phone minutes
behind files the bubble minutes early, until the engine's row corrects it.

**The caller's own row is still written late** by the same rule, and is not
fixed here. `SpokenMessageReceived` is an exchange BOUNDARY rather than a step,
so re-placing it means reordering exchanges. The caller's live row already draws
in place, so nobody has reported it.

## Alternatives considered

**Back-date `created`.** The bus can stamp a historical `created`, so the row
could simply be dated when it was said. Rejected: `created` is the append order
the client's incremental fold requires to be monotonic, so every spoken row
would force a full regroup. It would also reorder the row against the caller's
row, which `call.rs` deliberately writes first.

**Write the reply down when the talker stops speaking.** Then `created` would be
close enough to the truth. Rejected: that is exactly the rule ADR 0188 removed. A
Live talker pauses for 700 ms mid-sentence, and a row per pause drew one sentence
as eight bubbles.

**Fix only the live bubble, client-side.** It covers the screenshot and costs no
event change. Rejected: the persisted row would still read below the steps. The
bubble would jump the moment the engine wrote it down, and a reloaded transcript
would be wrong for good.

**Order the row by the client's own arrival sequence.** The client knows which
events it had already received when it drew the bubble. Rejected: it answers
only for a call being watched live, and history has no such record.
