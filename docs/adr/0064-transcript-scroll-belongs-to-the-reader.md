# 0064: The transcript's scroll position belongs to the reader: one standing follow request, armed by the toggle alone

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

The chat transcript used to pin the reader to the bottom while a reply streamed.
Most of `components/chat/scrollState.ts` was the machinery for deciding *when*
to pin:

- an 80px stickiness window,
- two ResizeObserver modes behind a 500ms suppression window,
- a 16ms re-pinning loop with a frame budget,
- a per-caller set of "was the user at the bottom" reads.

Every one of those terms was an inference about what the reader wanted. A reader
who had deliberately scrolled 79px up was still pinned. A reader who happened to
sit at the bottom was treated as having asked to stay there.

## Decision

Nothing moves the transcript on its own. The app moves it only as the direct
result of an explicit user action that asks for it, and that list is exhaustive:

- the two chevrons (`scrollToTop` / `scrollToBottomAnimated`),
- the follow toggle (`setFollowLiveEdge`),
- the four submits, all through `followSubmit`: sending a message, answering a
  question card, deciding a permission card, and Continue after an abort,
- turn stepping with the arrow chords (`stepThreadTurn`),
- a notification or Changes deep link (`scrollToEventAndPulse` /
  `scrollToChangeAndPulse`),
- `useScrollMemory` returning a reader to the position they left.

Everything else leaves the reader exactly where they are: a streaming reply, a
question or permission card arriving, a thread sync, a thread opening.

Exactly one of those asks is STANDING rather than one-shot. The follow toggle
means "take me to the live edge and keep me there". It arms a flag, and that
flag plus a measured position is the whole condition for following. There is no
proximity term and no timing term.

## Rationale

**A position is not a request.** The retired stickiness and suppression windows
both tried to infer an ask the reader never made. Recording the ask instead
makes two questions separate and answerable: did they ask, and where are they.

**Armed and carrying are two states.** ARMED is the request, and it survives an
idle spell untouched. The toggle stays lit and the per-thread reading position
keeps recording it. CARRYING is whether the app acts on it this instant, which
on a quiet thread it does not. A reader who scrolls away from a finished reply
keeps the lit toggle and is picked back up when the thread runs again.

**Where the reader is decides which half answers.** A reader ON the live edge is
kept there, running thread or quiet one. The app's own rendering must not slide
the edge out from under someone who never left it. A reader SCROLLED AWAY is
carried only while the agent is live, because a quiet thread has nothing to be
carried toward.

**Four things retire a standing follow, and all four are the reader**: their own
scroll gesture, a chevron or turn-nav press, opening another thread, and a
deep-link landing. A gesture and not merely a scroll, because the platform
scrolls the container too. The last three are presses rather than gestures, so
each retires the ride at its own call site.

**A scroll the platform made retires nothing and is undone.** The iOS soft
keyboard, a PWA resuming onto a restored offset and a restored session all move
the container with no gesture. Leaving the follow armed there is only half the
rule: the reader is put back on the live edge they asked for.

**A submit arms nothing.** It is one ask with one reaction, whichever of the
four shapes it takes. A reader already riding the live edge is taken there,
because that is the toggle's standing ask being served. Everyone else gets a
one-shot landing. It brings the turn they acted on as far up the viewport as the
transcript can reach.

**One landing line.** A turn comes to rest in the same place whatever put it
there, a submit, turn stepping or a deep link. The line is `scroll-margin-top`
below the container's top edge, clear of the chrome stacked there. Three
navigations sharing one number is what stops the same turn resting in three
places.

**The deep link is the one navigation exempt from the liveness term.** The
others describe a moment, where the reader happens to be looking. A link names
one event and expects to still be on it later, so the ask has to survive the
thread waking.

## Consequences

- `awayFromBottom` carries the whole weight of the live edge. An unarmed reader
  is routinely away from the bottom while a reply streams, and the down chevron
  is their only way back.
- The armed flag is a signal, because the toggle renders it. It has to go off by
  itself when a scroll retires the follow underneath it. It is exported
  read-only, so reading the state cannot become a way of setting it.
- The request belongs to a thread and outlives leaving it. It is written down
  per thread as one of the two forms a reading position takes, and re-armed on
  re-entry. Only a toggle press can ever be recorded, so a resume can only
  replay a request the reader made in that thread.
- The reader's last toggle press is also seeded across threads and reloads,
  device-scoped. A brand-new thread with no reading position of its own can
  still ride.
- The transcript's height is a function of its content and nothing else. No
  layout rule reads the follow flag.
- What the old narrowness protected still holds: a lull between two tool calls
  must never cost the reader their ride. A lull is not idle, and a submit claims
  liveness by itself for a bounded window before any status can say so.

## Alternatives considered

**Force-pin behind a stickiness and suppression window.** The original design.
It inferred the request from proximity and timing. So it pinned a reader who had
deliberately scrolled up, and never pinned one who wanted it from further away.

Anchor preservation survives from that era. It holds a reader on the same
content when layout shifts around them, and is the opposite of a pin.

**A send and an answer arm the follow.** Tried and reverted. The reply then
dragged the reader down through itself. That is a standing request the reader
never made, granted on the strength of one one-shot action.

**The down chevron arms the follow.** Tried and reverted. It left the mode with
no visible state and no way off but scrolling. It also left no way ON for a
reader already at the live edge, since the chevron is hidden exactly there.

Putting the state on the chevron does not rescue it. Go-to-bottom, then arm,
then disarm is a three-step cycle with no state left for a plain jump to the
bottom. Two controls is the answer, and the chevron is the one-shot half.

**Reserve a screenful of `min-height` under the newest turn.** Held for a day so
the newest turn could reach the landing line and the reply could grow into the
room. It is reserved air, and air below the last turn lies about how much thread
there is. Three separate reports followed:

- a rider was carried into the air,
- withholding it from a rider made every arming bug show as a blank screenful,
- it appeared mid-turn for a queued follow-up.

Nothing replaced it. The landing asks for the line only when the turn can reach
it. Otherwise it asks the modest question: show the turn.

**Gate the follow's write on the thread being live, not just the disarm.** This
answered a real report about a reader who had SCROLLED, by applying the fix to
every reader. A rider who never moved was then left above an arriving question
card with the toggle lit. The position term puts the line back where the report
drew it.

**Gate the deep link's retirement on the thread being live.** A thread parked on
a question card is quiescent, so it reads as idle. A "needs your answer"
notification points at exactly such a thread by construction. The reader landed
on the question, answered, and was carried off the event the notification
existed to show them.

**Read "did the reader take over" from the position alone.** Content growth
changes `scrollHeight` and never `scrollTop`, so the position test is exact for
growth and only for growth. The keyboard, an app resume and any other
platform-driven scroll move the container with no gesture. Each retired a follow
the reader had armed and never touched.

**Retire nothing on a platform scroll, and leave the reader where it put them.**
Half a rule. The ride survived while the reader sat off the edge, waiting for
the next growth round to notice. On a thread inside one slow tool call that is
tens of seconds of a lit toggle following nothing.

**Predict at submit time whether there is anywhere to go.** Two branches tried
this and both got the ordinary case wrong. Both asked about the transcript as it
stands, to predict where a turn that has not rendered will sit. The surviving
test measures the landing instead: a target at or behind the reader writes
nothing.

**Tween the snap after an anchored mutation.** Reasoned from "one click can add
the height of the whole transcript, so an instant write teleports the reader".
Turning the steps on grows every turn ABOVE the reader too. The transcript
arrived a screenful short of the live edge and then scrolled itself down. The
snap is the smaller motion, and it is invisible rather than instant, because it
lands before the paint the mutation causes.

**Lapse a deferred landing to the live edge.** Correct only while a send armed
the follow. With no arming there is nothing to honour. The case the deadline
covers is a turn with no box, so there is nothing to show the reader anyway.
