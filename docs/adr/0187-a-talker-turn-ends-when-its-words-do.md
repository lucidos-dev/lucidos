# 0187: A talker turn ends when its words do, not when its output stream does

- **Status**: Accepted
- **Date**: 2026-09-15

Corrects the second half of [ADR 0181](0181-a-live-talker-has-no-turns-and-no-tools.md)'s
turn synthesis. [ADR 0185](0185-a-caller-nobody-answers-reaches-the-doer.md)
already corrected the first half, for the same reason.

## Context

ADR 0181 says the talker's turn ends when its own output stream goes quiet, and
calls that "a fact about a stream we are receiving". `voice/live.rs` implemented
it as a 700 ms hole in `last_output`, refreshed by every audio frame and by
every transcript delta.

A Live call streams output when nothing is being said. Audio arrives between
turns, and a transcript delta arrives blank. So the bound never expired: one
32-second call produced zero `TalkerTurnEnded`, and both of its replies landed
as one row at the hangup.

Seven things hang off that one event. The caller's row and the reply's row are
both written there. So are the floor coming back, the queued answer going out,
the per-turn delegation resetting, and the usage row. None of them happened.

The worst of them is silent. A doer answer reaching a held floor is queued, and
only a turn end releases it. So on a Live call the answer never reached the
caller's ear.

The full trace is in
`docs/plans/2026-09-15-a-talker-turn-ends-when-its-words-do.md`.

> **Corrected by [ADR 0188](0188-one-thing-the-talker-said-is-one-row.md).** The
> word stream is not a turn boundary either: the provider paces its transcript
> with its audio, so holes of up to 3.6 s fall inside one reply. What survives
> is everything below except the reply's ROW, which is now closed by the
> conversation moving. The bound stays, under the name it earned: a pause.

## Decision

**A talker turn ends when its WORDS go quiet.** `TALKER_IDLE` is armed by a
transcript delta carrying words, and by nothing else. Audio arms it, and a blank
delta arms it, no longer.

**A turn that said nothing has no end.** With no words there is no reply to write
down, so the provider yields no event for one.

**The floor is taken by words too**, in `voice/call.rs`. A talker's audio frame
forwards and claims nothing.

**An ignored duplicate ask leaves the caller owed an answer.** It started
nothing, so the caller-waited bound is still theirs, matching what `answer`
already did for a refused or held resolution.

## Rationale

**The two halves of ADR 0181 had the same flaw, and this is the second.** Reading
the caller's turn end off a resuming output stream cut one sentence into seven
rows. ADR 0185 fixed that by reading the talker's words instead. Reading the
talker's own turn end off that same stream failed the other way, and never fired
at all. Words are the signal in both directions.

**An output frame carrying no words is not somebody speaking.** The codebase says
this in four places now, and each was written after the same class of bug.

**The floor has to follow, or the fix is half of one.** A provider that streams
audio between turns claims the floor on its first frame and holds it until the
line closes. Deriving the turn end correctly does not help a floor nothing gives
back.

**No timer decides when a person stopped talking.** This measures our own
received words, which is the same subject ADR 0181 defended and a narrower
signal.

## Consequences

**A turn with audio and no words yields nothing at all**, where it used to yield
a `TalkerTurnEnded` with an empty transcript. It wrote no row, and the floor it
handed back is no longer taken. Three smaller things went with it:

- The `A talker turn ended with nothing said` log. It was a proxy for a mute
  talker, and the caller-waited bound reports the same fact by its effect.
- The zero-token usage row, which priced nothing.
- The `delegated_this_turn` reset. So a talker that never speaks now holds one
  turn for the call, and its second ask is dropped. That ask no longer disarms
  the caller-waited bound, so the doer still takes the question.

**A Realtime response's first audio frame now leads its first worded delta.** An
answer landing in that window is asked for early. The two interleave on that
provider, so the window is however long it takes to send one delta.

**A reply whose word stream pauses past 700 ms would split in two.** Unmeasurable
without a provider-frame recorder, and strictly better than a bound that never
expires. One answer in two bubbles is visible at once.

## Alternatives considered

**Keep reading the output stream and lengthen `TALKER_IDLE`.** Rejected: the
stream on the reported call had no hole at all in 31 seconds, so no length works.
A longer bound also delays every real turn end, and the caller's row waits behind
it.

**End the talker's turn when the caller speaks instead.** Rejected: the caller's
transcript and the talker's interleave under echo and under a barge-in. Every
alternation would then be a boundary, which is the seven-row defect mirrored onto
the other side.

**Detect silence in the audio itself.** Rejected: a PCM threshold is a heuristic
about content, and the transcript already states the same fact exactly. It would
also put an audio codec's business inside a turn-boundary rule.

**Leave the floor alone and fix only the clock.** Rejected: it fixes the two
defects a reader can see and leaves the one they cannot. A caller whose question
is answered in silence has no way to know an answer was ever produced.
