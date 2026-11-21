# 0198: A Live talker is prompted into delegating, and the caller's turn is cut by the session clock

- **Status**: Accepted
- **Date**: 2026-09-16

Extends ADR 0181, which introduced the Live provider. Nothing in it is
withdrawn. ADR 0164's rule that the talker holds one tool and it delegates is
what this makes true on a protocol with no tools.

## Context

One call is the whole evidence, and the plan traces it:
`docs/plans/2026-09-16-a-live-call-delegates-what-it-promised.md`. A caller
asked what was going on in the workspace. The talker said "On it, I'll see what
I have.", asked for nothing, and the caller sat in silence for twelve seconds
and hung up. The thread recorded one caller row reading `now`, and no title.

Three things were wrong, and they are independent.

**The Live talker was never told how to ask for help.** Client delegation
declares no functions, so `DELEGATE_TOOL_DESCRIPTION` reaches `realtime.rs`
alone and rides its `delegate` declaration. A Live session got
`TALKER_INSTRUCTIONS` and nothing else. That text says "you have tools" (it has
none) and "tell them you are on it, then stop and wait".

**The caller's turn was cut by arrival order.** ADR 0181 ends that turn when
the talker starts speaking, which is the model's own judgment and stays right.
What was wrong is which words the boundary took. `live.rs` read `delta` and
dropped `start_ms`, so the cut handed over whatever had arrived.

**A talker-only call got no title.** Titles are generated inside a chat turn,
and `SpokenMessageReceived` starts none.

Three facts from the provider's own guides decide most of this.

- Client-mode delegation is steered by **prompting only**. There is no
  description field and no configuration under `session.delegation`.
- A Live session takes **no input-transcription settings**. Its startup fields
  are `model`, `instructions`, `input`, `audio`, `delegation` and `store`.
- Both transcript deltas carry **`start_ms` and `end_ms` on one session
  clock**, and the guide says not to treat a fragment as a complete user turn.
  Input transcription runs asynchronously, so late text is expected.

## Decision

**Each provider owns how delegation is expressed, below the seam.** Realtime
says it in the `delegate` tool's description. Live says it in the instructions
it opens with. `voice/mod.rs` owns the words as `DELEGATION_POLICY`, and
`live.rs::session_start` composes them onto `opening.instructions`. Nothing
above `provider.rs` learns which provider answered.

**The policy follows the provider's recommended shape**, with its three labels
kept verbatim: what the backend can do, when to hand over, when not to.

**The talker may not promise work it has not handed over.** Saying "on it" is
honest only in a turn that also asked for the doer.

**A held ask is bounded.** A delegation frame carries no text, so
`settle_the_pending_utterance` waits for a transcript. Past
`CALLER_WAITED_LONG_ENOUGH` with none, the ask is dropped and the caller is
told out loud.

**The caller's turn is cut by the session clock, not by arrival order.**
`live.rs` holds each caller fragment with its `start_ms`. The talker's first
word cuts at its own `start_ms`, taking every fragment that began before it and
leaving the rest. Words the caller spoke OVER the reply therefore stay out of
the turn that reply answered. The pieces are joined in the order they were
said, never the order they arrived.

**The cut does not WAIT for the transcript to catch up**, and a late fragment
is still lost to the next turn. A bounded wait was written and reverted: see
the alternatives below. Closing that hole means moving where `call.rs` reads a
move of the conversation, which is its own design.

**A Live session is configured with no transcriber and no language.** A test
says so, rather than leaving the omission to read as an oversight.

**A thread the caller only spoke on is titled the way a typed one is.**
`get_thread_first_message` reads `SpokenMessageReceived` as well, which is
already what the client's `threadTitle.ts` does. `api/voice.rs` titles at the
end of the call. A call that delegated is left to the chat titler, which owns
any thread carrying a `MessageReceived`.

## Rationale

**The prompt is the only lever, so it is where the fix goes.** No mechanical
test separates a promise from an answer. A talker that answers from the
resident block leaves the call in exactly the state a talker that promised and
stopped does: words held, no ask, nobody speaking. The safety net cannot tell
them apart. A net that fired on both would run a redundant turn on every call
the talker handled alone.

**A finished utterance is a MOVE, so it cannot be delayed.** `call.rs` reads one
as the thing that closes the talker's row and writes the caller's. The order it
depends on is that the question reaches it before the reply's first word. Any
wait breaks that, whichever side of the reply the words are released on. This
is the reasoning the reverted alternative below is measured against.

**The title belongs at the end of the call.** By then the thread is whole, and
no turn is going to claim it. A thread still on a call may yet delegate, and
that path titles from the same words.

## Consequences

- A Live talker's opening instructions are longer by one policy block. It is
  workspace-stable, so the cached prefix is still worth caching.
- `TurnState` holds fragments rather than one string. Turn synthesis stays
  entirely below the seam, and no event shape changes.
- A fragment transcribed after its own boundary read as part of the next turn.
  The 2026-11-11 amendment below closes that half.
- A thread can now be titled by two paths, so the voice one checks for a
  `MessageReceived` before claiming it.
- The workspace's transcriber and language preferences do nothing on a Live
  call. The protocol offers nowhere to send them, and the Settings copy does
  not say so yet.

## Alternatives considered

**Send the transcriber and language in the Live opening frame.** This is what
the code looked like it had forgotten to do, and `SessionOpening` carries both.
It cannot be done: the protocol has no key for either. A test now pins the
absence so the next reader does not spend the same hour on it.

**Re-arm the caller's bound whenever the talker's turn ends with words still
held.** Mechanical, and wrong. That state is also what a correctly answered
call looks like. It would wake the doer on every one of them, and speak a
second answer over the first.

**Read the talker's reply and classify it as a promise.** A second model call
on the hot path of a live conversation, to recover from a prompt we had simply
never written. Fix the prompt.

**Wait for the transcript to cross the boundary before cutting.** Written,
tested, and reverted in the same change, because it regresses ADR 0188.

A finished utterance is what closes the talker's row. So one released 1.5 s
into a reply cuts that reply in half, and files the question below its own
answer. Releasing it AFTER the reply instead only moves the break, leaving the
question below the answer for good.

A short reply is worse still. It ends before the wait does, so the late words
re-arm `CALLER_WAITED_LONG_ENOUGH` with nothing left to disarm it. The doer then
wakes on a question the talker already answered.

**Keep cutting on arrival order and re-join the pieces afterwards.** The engine
would have to revise a row it already wrote, and events are immutable.

**Cut the caller's turn when the talker's turn ENDS rather than when it
starts.** The transcript would always have caught up by then. It also delays
every delegation by a whole spoken reply, which is the latency ADR 0164
deliberately removed.

**Title on the first spoken row rather than at the end of the call.** The name
would appear sooner and describe less. A caller's first sentence on a call is
often "hello".

## Amendment, 2026-11-11: the rest of an asked sentence reaches the turn it started

**The open half above is closed by delivering the tail, not by waiting for it.**
A Home call showed it three times. The ask went out on "when do you estimate
that it's", and "finished" landed 116 ms later. The doer ran on half a question
and guessed the rest.

**Below the seam, the provider places where an asked sentence ends.** The
session clock bounds it to two seconds after the ask. The talker's first
worded delta can shorten that bound. An ask made mid-reply ends where that
reply began, within the same bound. A caller fragment that started before
that point goes out as `UserTurnContinued`. Later words stay a turn of their
own.

An ask with no offset, or a piece with no timing, cannot be placed and
continues nothing. This resolves residual (a), including a talker silent while
the doer works.

**Above the seam, the call writes the row as before and offers the words to the
turn that took its ask**, through `TurnStarter::continued`. The offer is a
`SpokenContinuation` injection. It adds no `PromptInjected`, because the row is
the record. The words skip the undelivered pile, so no later ask runs them again.
With no turn taken, they are ordinary words. With an ask still waiting for
words, they become its words.

**Nothing waits, and nothing is rewritten.** The ask still leaves on its own
frame, so delegation costs no added latency. Each row keeps its own `created`.
A turn not running yet reads the tail from history, since the row lands before
the offer.

**Only the turn that took the ask gets the tail, and only once it reads its
history.** That turn is either anchored on the ask's `WorkDelegated` or had
the ask coalesced into it. Another turn on the thread would bolt the words onto
the wrong request. A turn still to read its history finds the row there.

**A live turn that has not taken the ask yet holds the tail.** The ask's own
task may still be on its way to that busy thread, and its history was read
long before. When the ask is coalesced in, the tail follows it. When the ask
goes elsewhere, the held tail dies with the turn, and the ask's own turn reads
the row.

**The call stops owing a turn the tail only on that turn's own terminal.** It
matches the terminal's request id against the ask, or against the turn a
`PromptInjected` naming the ask joined. A turn the ask waited behind, or an
earlier turn's late terminal, no longer turns the tail into ordinary words.
The call offers the tail under that same turn, so the engine and the call
agree on it.

**An ask orphaned in a turn's teardown goes to the next turn as itself.** On
a Lucidos Agent thread with a live turn, the orphan chain hands each orphan to
that turn as the queued message it is. The turn's own pickup then names it,
under its own request id, and marks it taken, which releases a held tail.
The batch goes over under one lock, so the turn takes all of it or none, and
a later orphan never overtakes an earlier one. What no live turn takes is re-run as one turn on the batch's first message,
and its announce names that message. The call offers the tail under the turn
it tracks, so an ask batched behind an earlier message still gets it.

The chain no longer announces a live turn it has not joined yet. That
announce read the live turn apart from the re-run's admission. It missed a
turn still to record its request id. It left a tail held under the ask while
the re-run came in under another id. And it moved the turn's output under
the orphan before pickup.

**So an offered tail is one the turn's history may lack, and it reopens a
finished answer.** A spoken aside does not, since it asks nothing. The reopen
costs a round, never a second answer: the talker speaks only the turn's last
one. Reading the row in history costs nothing extra, which was the Home call's
case 3 times out of 3.

What stays open: a tail landing in the teardown window, after the answer went
out, is dropped and not re-submitted. A new turn would be a second spoken
answer. The row reaches the next round.

Resolved (c): hand-over and re-run admission now share one lock. An idle
thread gets the re-run's handle before the batch is announced. A live turn
receives each orphan under its own id and kind, preserving images and
releasing held tails. A closing turn queues the remaining batch. The re-run
keeps its pre-won registration through setup, and an early return releases
it and re-submits any injected follow-ups. The announce therefore names the
turn that runs, closing the race recorded in
`docs/plans/2026-11-11-live-voice-call-review.md` § F1.
