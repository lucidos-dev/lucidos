# 0200: A breath is not a turn, and a barge-in is its own signal

- **Status**: Accepted
- **Date**: 2026-09-16

Corrects one clause of [ADR 0188](0188-one-thing-the-talker-said-is-one-row.md),
which makes every finished caller turn a move of the conversation. Extends
[ADR 0198](0198-a-live-talker-is-prompted-into-delegating.md), whose named
residual is the case this closes. Nothing in
[ADR 0191](0191-a-pause-spends-nothing.md) is withdrawn.

## Context

One call is the whole evidence, and the plan traces it:
`docs/plans/2026-09-16-a-breath-is-not-a-turn-and-a-reply-is-not-chopped.md`.
The caller said "Status, please" with a breath before the last word. The thread
recorded four rows.

```text
SpokenMessageReceived    Status
SpokenReplyGenerated     Still
SpokenMessageReceived    , please
SpokenReplyGenerated     in it. I'm pulling the current threads together ...
```

The same call did it twice more. "Hey again, just testing some more" and "here"
are one sentence. "Go on, I'm right here with" and "you." are one sentence, and
their rows are seven seconds apart.

The provider endpointed on a comma and answered into the breath. We cannot
configure that: a Live session takes no turn-detection settings (ADR 0198). What
we own is everything after it, and three things went wrong there.

**A caller turn closed the reply it landed inside.** `call.rs` reads any
finished utterance as a move, so the tail of the caller's sentence cut the reply
at the word it arrived on.

**The client cut the talker off for a caller who never stopped.** Its gate
opening over the talker sent `barge_in` and threw the speaker's queue away. A
Live talker cannot be cancelled, so it carried on, and the caller heard the rest
of the same sentence land after their own words.

**A Live delegation reason was the caller's own transcript.** The frame carries
no words, so the engine filled the gap. `WorkDelegated` renders a reason under
the TALKER's speaker label. So the caller watched it say their sentence back at
them, above their own row, and the doer read it twice.

## Decision

**A caller turn landing inside a reply is not a move of the conversation.** The
words join what is already held, and the reply goes on filling one row. Nothing
is lost by waiting: the next real move writes both, in the order they were said.

**A barge-in is the exception, and it is the client's signal to send.** There
the caller took the floor, so the reply is over and its row is owed at once.

**A barge-in cuts the reply off outright.** While it is in force the engine
drops the rest of that reply: no audio reaches the caller, and no delta reaches
the row. That is what a cancelled reply already looks like on Realtime, where
the provider stops generating.

**One barge-in cancels once, and a barge-in with nobody speaking cancels
nothing.** There is no reply to stop, and a cancel for a response that does not
exist is refused.

**The client asks for a cut only when the caller has given the floor up.** A
barge-in needs a quiet stretch of `BARGE_IN_QUIET_MS`, measured in captured
audio frames rather than off a clock. Below that the caller is finishing a
sentence the talker talked over.

**A cut reply owes exactly one row.** The turn end reports the whole reply,
tail included. So a cut whose row already went down is read for nothing. Taken
as the next stretch's fallback, it wrote the reply a second time, in full,
saying nobody cut it off.

**The two held rows go down in the order they were said.** The caller's question
leads the reply that answered it, which is the ordinary case. Words they got in
OVER a reply follow it, because filing them first puts one breath above the
sentence it landed inside. Both sides carry their own start for that.

**A caller still making words keeps a call the talker is ringing off.** Their
intent is the only thing that ends one (ADR 0170), and a partial says they are
using it. Nothing else reaches that moment: a Live talker reports no
interruption of its own, and the client sends no cut for somebody who never
stopped.

**A Live delegation carries no reason.** It composed none, so it claims none,
and `build.rs` writes no row for a blank one.

## Rationale

**The two signals answer different questions, so they cannot share an
answer.** Turn end is "what did the caller say", and the provider owns it.
Barge-in is "did the caller take the floor", and only the client hears the voice
that decides it. Deriving either from the other is what produced both halves of
this defect.

**A held turn costs nothing, and a written one cannot be taken back.** Events
are immutable, so a row split at the wrong moment stays split. Waiting for the
next move is free: the live caption already covers the gap on screen.

**The engine is the only thing that can make a Live cut real.** The client can
silence its own speaker and the provider will not stop, so the two together are
a hole rather than a cut. Dropping the tail in the engine is what makes them one
action.

**Quiet is measured in frames because the frames are the evidence.** A wall
clock measures the machine, and a slow render would stretch a breath into a
handover. A captured frame is 40 ms of the caller's own silence.

**A reason nobody composed is not a reason.** The prompt is the only lever on a
Live talker (ADR 0198), and no prompt can stop the engine pasting a transcript
in. Leaving the field empty is the honest report, and the caller's words still
travel as the utterance.

## Consequences

- A reply is written down later still: it now waits for a move the caller's
  own breath no longer counts as. The live row covers it on screen.
- A caller row can carry two of the provider's turns joined with a space, which
  is what `append_spoken` already does for a split transcription item.
- `interrupted` is now reachable on a Live call. Its provider emits no
  `Interrupted`, so before this the barge-in control set nothing.
- A caller who cuts in loses the tail of the reply they cut. That is the point,
  and it is what makes the cut hearable.
- The first second of a reply is still interruptible. The quiet stretch is the
  caller's own, and the talker taking the floor does not reset it.
- A `ClientControl::BargeIn` now carries weight in the transcript, not just in
  the speaker. A client that stops sending it would merge rows it should split.
- `interrupted` means the caller TOOK the floor, and no longer that they spoke
  over the reply. The two were one thing before this.
- A running doer round is no longer offered a cut reply. The transcript covers
  words the caller was never played, so a round told they heard it would make
  the claim the cut refuses.

## Alternatives considered

**Cut the caller's turn once per talker reply, below the seam.** Written and
measured against the recorded call. It stops the provider manufacturing a
boundary, and it makes the merge WORSE: the tail is then held until the talker's
next reply, by which time a reply row sits between the two halves. The engine
fix above merges them instead.

**Let the client keep deciding the cut, and wait for the engine's
`interrupted` frame before stopping playback.** Rejected: it buys the
distinction with a round trip on every barge-in, which is the latency the
client-side gate exists to avoid.

**Drop the client's `stop-playback` and silence the reply in the engine
alone.** Rejected for the same reason. The buffered audio is already on the
caller's device, and only the client can stop it now.

**Tell a barge-in from a breath by how long the talker has been speaking.**
Rejected: it makes an early interruption unreachable for a fixed window after
every reply starts, whatever the caller was doing before it.

**Read the caller's words and classify the tail as a continuation.** A second
model call on the hot path of a live conversation, to recover a fact the speech
gate already measures.

**Keep the reason and mark it as the caller's words rather than the talker's.**
Rejected: the doer then reads the same sentence twice, once labelled and once
as the message. The row that carries it is `MessageReceived`, and it is enough.
