# 0211: A call opens silent: the talker is not heard before the caller's first word

- **Status**: Accepted, and amended by [ADR 0213](0213-one-opener-buys-one-answer.md)
  and [ADR 0218](0218-a-mute-covers-a-sentence.md)
- **Date**: 2026-09-17

> **What 0213 changes.** "Three things open the floor, and none closes it again"
> is superseded. Each opener now buys a bounded run of turns, because a floor
> held for the whole call let one hello license a forty-nine-second recitation.
> Everything else here stands, including why the floor survives a turn end.

> **What 0218 changes.** Two things. "The decision is per turn" is superseded,
> and a mute now covers a SENTENCE. A Live turn ends at every 700 ms hole, so
> the latch expired inside the sentence it was guarding. And "A Live one has no
> such frame, and needs none" was wrong. Its talker answers the caller's audio
> before its own transcriber reports a word, so the caller's device sends the
> opener instead.

## Context

A reader turned caller mode on in a thread that already had history. The talker
started talking to itself. The call ran fourteen seconds, the caller never said
a word, and the thread took five `SpokenReplyGenerated` rows.

None of it answered anybody. It was the thread's own recent history read back
out, ending in a paraphrase of a question card the reader had cancelled six
minutes earlier. Its first row was "Yeah, on it", the promise the talker makes
when it asks for work. It said that with nothing asked, and no `WorkDelegated`
behind it.

Nothing said a call may not speak first. The Realtime provider happens to stay
quiet, because it sends no `response.create` at open. That is an accident of
one protocol.

The Live provider supplied the prompt. It sent the *resident block* after
`session.started`, as `session.instructions.append` frames. That is the steering
channel, and the block is a snapshot of the conversation written as
`They said: …` and `You said: …` lines. So the last line of new steering read as
an unanswered turn.

## Decision

**A call opens with the floor shut. Nothing the talker produces reaches the
caller until the caller has said a word**: not audio, not a transcript frame,
not a row on the thread, and not an aside to a running doer round.

**Three things open the floor, and none closes it again.** The caller saying
anything. The provider reporting that they started speaking. And the engine
asking the talker to speak, through `Call::say`, which is how a card parked on a
silent caller is still put to them.

Two more rules make the rest precise.

- **The decision is per turn, taken by its first WORD.** A turn that began
  unheard stays unheard even if the caller speaks halfway through.
- **Audio decides nothing.** A Live talker streams audio between turns, so a
  turn read off the stream would be decided at call open and never again.

The gate is `Audience` in `voice/call.rs`, above the provider seam. Beside it,
the Live resident block moves to `session.thinking.append`, the quiet channel.

## Rationale

**The floor is a mechanism, and the prompt is a tendency.** The block already
carries a heading saying it is what the talker KNOWS, and this model answered it
anyway. A rule that holds only when the model cooperates does not hold on the
call where it matters.

**It belongs above the seam for the same reason.** Realtime's silence at open is
a property of its protocol, not a decision anybody took. So a provider swap
would reopen this with no code change to blame.

**The floor is sticky because an answer is not one turn.** A Live turn ends at
every 700 ms hole in the talker's words, so one spoken answer spans several
(ADR 0187). Spent per turn, the engine's own answer went audible for one
sentence and then cut out on a caller who had said nothing.

**A transcriber must not be able to withhold the floor.** `whisper-1` is
selectable in Settings and streams no partials. Its completed frame is
asynchronous, so it can land after the reply it prompted. Waiting on words
alone therefore drops the answer to the caller's very first sentence.
`input_audio_buffer.speech_started` is the signal that cannot be withheld: the
provider fires it on every utterance, and it needs no transcriber. It reaches
the seam as `VoiceEvent::CallerStartedSpeaking`, which opens the floor and does
nothing else.

**That signal is still not an interruption.** It fires on the first word of a
call with nothing playing, so reading it as a cut would report one on every
turn. The client detects its own barge-in and says so with a `barge_in`
control, and that control is not a floor opener either: it is sent only while
the client's phase is `speaking`, and a muted talker never puts it there.

**The quiet channel is what the block always was.** `session.thinking.append` is
documented as state that reaches the talker and is not spoken on arrival. The
glossary already said the block enters the session as a history item and never
as instructions. The Realtime provider did that and the Live one did not.

## Consequences

**A call now opens in silence, and there is no greeting.** Somebody who presses
the button and waits hears nothing until they speak. That was chosen over a
fixed opener, which stays open as a later change and would be built on this
gate.

**A Realtime call survives a transcriber that says nothing**, because
`speech_started` opens the floor without it. A Live one has no such frame, and
needs none: it transcribes the caller itself and its partials are what the
whole turn synthesis already reads. A dropped turn is logged with its text
either way, so a floor that never opens is greppable.

**An unheard turn still records its usage**, because we were billed for it, and
it still leaves the caller owed an answer. That second half matters: a babble
the caller never heard must not disarm the bound that sends an unanswered caller
to the doer (ADR 0185).

**The talker still remembers what it babbled.** The gate drops those words on
our side. The provider holds them in its own session history and no seam member
can retract them.

## Alternatives considered

**Prompt only: reword the block and tell the talker to wait.** Cheapest, and it
attacks the real cause. Rejected as the whole fix because it is a tendency
rather than a guarantee, and this model had already ignored the block's existing
heading. The block's move to the quiet channel keeps the useful half of it.

**A fixed opening line, then silence.** The engine speaks a short greeting and
drops anything else the talker volunteers. It answers the awkwardness of a
silent line, and it needs this gate underneath it anyway. Deferred rather than
refused.

**Fold the block into the Live opening frame.** The first idea, and worse. The
startup `instructions` field has an undocumented cap. A refused opening frame
kills the call outright, which is worse than the defect being fixed. The quiet
channel reaches the same talker with no such risk.

**Re-ask the question per delta instead of latching per turn.** Simpler state,
and wrong: a caller who speaks over a babble would then hear the back half of a
sentence whose front half was dropped.
