# 0188: One thing the talker said is one row, closed by the conversation moving

- **Status**: Superseded by ADR 0201
- **Date**: 2026-09-15

Corrects [ADR 0187](0187-a-talker-turn-ends-when-its-words-do.md), which is
right that the output STREAM is not the signal and wrong that the word stream
is one. Extends the caller-side rule in
[ADR 0185](0185-a-caller-nobody-answers-reaches-the-doer.md) to the talker.

## Context

ADR 0187 moved the Live talker's turn end onto a 700 ms hole in its WORDS. It
shipped, and the holes are everywhere inside one reply. Measured on one call:
1.0 s, 2.0 s, 1.3 s and 3.6 s. The provider paces its transcript with its
audio, and a speaker pauses.

So one sentence became several rows. `Nothing is waiting on you, and I have no
unread notifications` and its own full stop were two. A later reply was eleven.

**No bound length works.** The previous pass proved it never expires while it
reads the output stream. This one proves it expires mid-sentence while it reads
the words.

Three more things the reader saw followed from it. The talker's answer read
below the doer's steps. A Lucidos Agent header came and went between the speech
bubbles. Every bubble carried its own call mark.

A fourth sat beside them: a live caller bubble reading `bit` under a
"Requesting" header, standing until the hangup swept it. The full trace is in
`docs/plans/2026-09-15-one-thing-the-talker-said-is-one-row.md`.

## Decision

**A persisted reply row is one thing the talker said**: everything it says
between two moves of the conversation. The same sentence the caller's row
already follows.

**A move is the caller's finished words, a new thing handed to the talker to
say, or the call ending.** Nothing else, and no timer.

**The row is built from the raw deltas**, so a stretch spanning several pauses
joins with no seam. The last turn's own transcript stands in when no delta
arrived.

**The pause keeps everything else `TalkerTurnEnded` did.** It flushes the
caller's held row, releases the floor, and resets the per-turn delegation. It
also drains the queued answer, and offers the talker's words to a running doer
turn.

**Usage is one row per reply**, spent at the flush rather than at each pause.

**The talker's live row is drawn inside the block its persisted row will land
in**, as a step rather than a boundary.

**A persisted caller row retires every live row whose words are a strict PREFIX
of it.** The frame carrying those words carries the whole stretch too.

## Rationale

**The two sides of a call now read alike.** ADR 0185 made a caller row one
thing the caller said, between two moves. Nothing about the talker's side
argued for a different unit, and the divergence is what produced eleven bubbles
for one reply.

**The pause is a real fact and a useful one; it is just not a turn.** It is the
only moment that says the talker has had its say, which is what lets the
caller's held row go down promptly. Moving that to the next move would leave
their bubble shimmering "Requesting" for a whole reply, which is a defect this
project already fixed once.

**The deltas carry their own spacing and the per-turn transcripts do not.**
Each is trimmed, so joining them puts a space before a full stop, or loses the
one between two words.

**A live row that draws its own header is a header that comes and goes.** It is
appended past the fold for safety, so it sat at the bottom as its own block.
Filing it where its persisted row will land is what makes the swap invisible.

**Prefix, never containment, for the claim.** The engine appends as it
accumulates, so an earlier row of the stretch is a strict prefix by
construction. A containment test would let a short fragment retire a bubble
from a different sentence.

## Consequences

**A reply is written down late.** It lands when the caller next speaks rather
than when the talker stops. The live row covers the gap on screen and both
render identically, so the reader sees no delay. A trigger watching
`SpokenReplyGenerated` sees it late.

**A stall and the answer relayed after it are two rows**, because handing the
talker something new is a move. That also keeps the doer from being offered its
own answer back as something overheard.

**`interrupted` now means the caller cut in.** It reads the floor at the close
rather than being set for every last reply of every call.

**The Realtime path is unchanged in practice.** One response is one reply
there, so nothing accumulates across two.

## Alternatives considered

**Lengthen the word-idle bound.** Rejected: the measured holes inside one reply
reach 3.6 s, and a bound above that delays the caller's row by the same amount.

**End the talker's turn on the caller's words, below the seam.** Rejected: it
gives correct rows and takes the caller's row flush with it. The pause is the
only signal that the talker is not about to delegate.

**Add a `TalkerPaused` event beside `TalkerTurnEnded`.** Rejected: it puts one
provider's quirk in the shared vocabulary, which ADR 0181 rules out. The
distinction stays inside `voice/call.rs`.

**Coalesce the reply rows on the client instead.** Rejected: the engine's own
row would still be shattered, and every other reader of the event would see the
fragments. The caller's side refused the same split for the same reason.

**Claim a live row by containment rather than by prefix.** Rejected: `a` or
`it` appears inside almost any sentence, so a bubble somebody is still speaking
would vanish under an unrelated row landing.
