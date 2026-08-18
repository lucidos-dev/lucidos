# 0080: A submit rests the reader at the live edge, armed or not

- **Status**: Accepted
- **Date**: 2026-08-14

## Context

ADR 0064 gave a submit two resting places, picked by whether the follow toggle
was armed. A rider glided to the live edge. Everyone else got a one-shot landing
anchored on the turn they acted on, `landOnOwnTurn`, with two branches of its
own: the turn's top on the landing line where the turn had a screenful under it,
and otherwise the modest ask, SHOW THE TURN.

The modest ask is the one an ordinary send takes, because nothing sits under the
newest turn. It rested the turn's bottom on the container's bottom edge, which
is `--prompt-fade + --nav-focus-reach` short of the live edge. That distance is
the transcript's own bottom padding, and it exists to hold the last turn clear
of the composer dissolve.

So the row a reader waits for after sending, the agent's `Requesting` line, was
the row the dissolve ate. An armed rider never saw it, because the live edge is
past the padding by construction.

## Decision

A submit has ONE resting place, the live edge, whichever of its five shapes it
takes and whether or not the follow is armed. It still arms nothing.

It is a HOLD rather than a one-shot, for the four submits that ask the agent
for something. The landing re-aims at the live edge on every growth round. It
lets go once the turn it was made on has drawn a response row it did not have.
It also lets go on a QUEUED follow-up, which will never draw one.

A CANCEL is the exception, and it is a one-shot. It asks the agent to FINISH,
so no first row is coming to release on.

## Rationale

**The two halves of one act must agree.** Arming the follow is a request about
what happens NEXT, as a reply streams. It is not a statement about where a
submit should come to rest. Reading it as one gave the same act two answers.

**The live edge is the only bottom that clears the chrome.** The transcript
reserves its bottom padding for exactly the dissolve band the composer paints.
Any rest short of the live edge parks content inside that band. This is the
second time the landing has parked something there: `.response-header` once
carried a `scroll-margin-bottom` for the same reason, and it was deleted rather
than tuned.

**The landing is still DEFERRED, and that is what makes the target safe.** A
send and a Continue create the turn they land on, so the glide waits for it to
render. Without the wait the glide would aim at the transcript's bottom as it
stood BEFORE the submit. That is the blind jump the deferral was written for.

**A tween tracks its target, which covers one instalment and no more.**
`animateScroll` re-reads the target every frame (ADR 0065), so a glide lands on
the bottom the transcript has when it ENDS. That is why the hold above exists:
the agent's opening arrives in several instalments, and only the ones inside a
glide were ever caught.

The hold's LAST glide is the exception, and freezes instead. Tracking there
carried the reader past the very row that ended the hold.

**A submit still arms nothing.** The hold ends itself, and nothing writes the
live edge after it. A reply that streams on leaves the reader where the landing
put them, and the chevron is their way back down.

## Consequences

- `landOnOwnTurn` and its reach machinery are gone. The two branches, the
  reachability test and `LANDING_REACH_SLACK_PX` all described a target that no
  longer exists.
- `turnLandingClearancePx` survives for turn stepping, which still rests a turn
  on the landing line.
- ADR 0064's "One landing line" now covers two navigations, a deep link and turn
  stepping, rather than three.
- The two held glides are told apart by WHOSE motion they are, `'ride'` versus
  `'landing'`, rather than by where they aim. Both aim at the live edge. The
  ride still outranks the landing, and a reader gesture still cancels the
  landing.
- A tall question or permission card now rests its BOTTOM at the live edge
  rather than its top on the landing line. The reply streams below the fold
  there, which is the cost taken knowingly below.
- A QUEUED follow-up is taken to the bottom too, which undoes the reveal-only
  landing added for one. That was itself a report: a second message fired while
  the first reply ran took the reader off the reply they were watching. The
  reader has asked for the opposite here, and a queued bubble already ABOVE
  them still moves nobody, since a landing never scrolls backwards.
- A submit made from the live edge writes nothing on its first round, and the
  hold is what carries it from there. Every submit now waits for content its own
  submit causes, the two card shapes included.
- "Will never draw" is recognised POSITIVELY, by the remove button a queued
  follow-up's status carries. Inferring it from a missing `.response-panel`
  reads true for two turns that are about to draw: a row whose panel mounts a
  commit later, and an unanswered card divider, which renders no panel at all.
  The second is the turn a card submit acts on, so the inference abandoned the
  hold on exactly the case it exists for.
- A landing that has FOUND its turn no longer swallows the reader's next
  submit. The floor is for the composer's two calls, which both fire before any
  row renders. Past that, a hold on a turn that draws nothing would have cost
  them the next landing for the whole backstop.
- The backstop is reached by more than a dead request. A turn the reader has
  collapsed draws no body, and neither does a coding-agent turn running tool
  calls with the step log off. None of them grows much, so the hold moves the
  reader little while it runs.
- The landing outlives its first glide, so the reader's gesture has more ground
  to cover. It still ends the whole hold rather than one glide.
- A lapsed landing still moves nobody. Lapsing to the live edge stays rejected,
  for the reason ADR 0064 gives.
- CANCEL is the fifth submit, and the only one that never holds. One prompt-row
  control covers two acts and both END the turn: `cancel_chat` resolves a
  pending question as Canceled before firing the cancel token, and a Stop ends
  the turn outright. The reader asked for one reaction to both. A cancelled
  queued UPLOAD is not a submit at all, having sent nothing.
- The two cancel shapes differ only in the turn they land on. A cancelled
  QUESTION card grows no boundary exchange, since its own button carries the
  attribution, so its landing resolves the card. Everything else opens one and
  waits for it, as Continue waits for its continuation.
- A cancel does not CLAIM the thread is live. That claim's premise is that the
  agent will respond, which a cancel denies. Claimed anyway, an armed reader
  who pressed Stop would be carried by every growth round for its whole length.
- The hold's LAST glide freezes its target. Every earlier round tracks the live
  edge, which is what catches the opening instalments. The releasing round must
  not. Otherwise a second row landing inside its tween carries the reader on to
  that one as well, reported as scrolling past the first step.

## Alternatives considered

**Keep the landing line for a turn tall enough to reach it.** Weighed and
rejected. It preserves the one case where the old branch was better, a long card
whose reply then streams into view. The price is a submit having two answers
again, and the reader asked for one.

**Rest at the turn's bottom, and give the turn a `scroll-margin-bottom` to clear
the dissolve.** This is what the code did before ADR 0064, and the property was
deleted with that landing. It restates the transcript's bottom padding in a
second place, so the two can drift, and it answers only for the last turn.

**Arm the follow on a submit instead.** Already tried and reverted (ADR 0064).
The reply then drags the reader down through itself, which is a standing request
they never made.

**Glide immediately, with no deferral, and let the tween track the turn.** It
works for a reader far from the bottom, and fails for one already ON it. There
the distance is zero, the glide writes nothing, and the turn then renders below
them. That is the bug this ADR exists to fix.

**A ONE-SHOT landing for the four submits that ask the agent for something.**
How this shipped, and it is what the hold above replaced. A CANCEL keeps the
one-shot on purpose, for the reason the Decision gives: nothing is coming. The reader reported both halves
within the hour. A card is addressable the instant it is tapped, so a reader at
the bottom got no write at all. Everything their answer caused arrived after.
A send was spent when its row rendered, so the agent's opening rows landed below
the fold, "almost down, like before".

The reasoning for it was that a bounded ride is still a ride. What ENDS the hold
answers that. It lets go at the agent's first drawn row. A turn that will never
draw one (a queued follow-up renders no response panel) lets go at once, rather
than running to the backstop.

**End the hold on a fixed time window instead.** One number and no content
signal, which is simpler to reason about. It has no right length: short enough
not to hold through a fast stream is too short for a slow first token, and the
gap between those is seconds. Reading what the turn has DRAWN answers for both
without a guess.
