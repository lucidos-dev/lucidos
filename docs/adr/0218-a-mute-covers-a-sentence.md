# 0218: A mute covers a sentence, and the caller's device opens the floor

- **Status**: Accepted
- **Date**: 2026-09-18

Amends [ADR 0211](0211-a-call-opens-silent.md) in two places: the unit the
audience latch covers, and which providers get an opener no transcriber can
withhold.

## Context

A reader made two calls on one thread inside three minutes, and reported three
separate failures. All three come from one cause, and the log names it twice:
`[Voice] The floor was shut, so nobody heard the talker`.

A call opens with the floor shut (ADR 0211), so the talker is muted until the
caller speaks. Both calls opened with the Live talker speaking anyway, which is
the case that gate exists for. What went wrong is what happened next.

The caller spoke over the muted opener. The latch is per provider turn, and a
Live turn ends at every 700 ms hole in the words (ADR 0187). So the next
fragment of that same sentence was played and written down, and the row began
"doc. It says it's intentional". The reader read it as an answer to a question
nobody had asked.

That heard fragment then counted as the talker answering them. Their own
sentence had been split by the transcriber one second apart, into "What about"
and "now", and the fragment landed between the halves. The first half was
dropped as already answered, so the doer was woken on the single word "now" and
answered something else entirely.

ADR 0211 named this outcome exactly, when it rejected re-asking the floor per
delta: "a caller who speaks over a babble would then hear the back half of a
sentence whose front half was dropped." Latching per turn was the guard against
it. On the Live provider a turn is smaller than a sentence, so the guard expired
inside the thing it was guarding.

The first failure is the other half. The caller said "Please", and the talker's
whole answer was muted, because its first word landed before the caller's first
transcript partial. ADR 0211 solved that class with
`input_audio_buffer.speech_started`, and judged that a Live call "needs none"
since the provider transcribes the caller itself. That judgement is wrong in one
direction: the Live talker hears the caller's audio and starts composing before
its own transcriber reports a word.

## Decision

**A mute covers a sentence.** An unheard turn whose words stop mid-sentence
holds the latch across its own turn end, so the rest of that sentence is muted
too.

Three bounds on the hold, each closing a way it could outlive its purpose.

- **The words decide, and nothing else may.** A turn ending in `.`, `!`, `?` or
  `…` ends the run, closing quotes and brackets stripped first. No clock, no
  duration, no frame count.
- **Only over an OPEN floor.** Over a shut one the mute is already total, so a
  hold would only delay the answer the caller's next word buys (ADR 0213).
- **At most `TURNS_ONE_OPENER_BUYS` turns**, and an answer the engine hands over
  through `Call::say` clears it outright.

**The caller's own device is the third opener on every provider.** The client
already decides the caller is speaking from measured energy, 120 ms in, with no
transcriber in the path (`voice/speechGate.ts`). It now sends that opening edge
as the `caller_started_speaking` control, which opens the floor and does nothing
else.

## Rationale

**A turn was never the unit; it was a convenient stand-in for one.** ADR 0211
made the FLOOR sticky across turn ends for exactly this reason, because one
answer spans several Live turns. The latch keeping the opposite convention is
the asymmetry that produced the fragment. Both now span the answer.

**The sentence is the unit the complaint is written in.** The defect is a half
sentence reaching an ear and a thread, so the terminator is the thing to read.
It is also the cheapest possible test: the engine already holds the turn's text
for its log line.

**A silence window is not available here, and would not be better.** ADR 0213
measured a recitation pausing 2.3 seconds against a real answer pausing
eighteen, so the two orders are the wrong way round. That finding rules out a
clock for this decision as much as for the floor.

**The open-floor condition is what keeps ADR 0213's promise.** After the turn
bound bites, one word from the caller buys the next answer. The talker is
usually mid-sentence when it bites, so an unconditional hold would spend that
word on silence. Over a shut floor there is nothing to hold back, because every
turn is muted anyway.

**A client signal is not a weaker signal.** The client draws the caller's own
bubble off this same edge (ADR 0184), so the engine is already trusting it for
what the reader sees. ADR 0211 wanted a signal a transcriber cannot withhold,
and the microphone is upstream of every transcriber there is.

**It is not an interruption, for ADR 0211's reason unchanged.** It fires on the
first word of a call with nothing playing, so reading it as a cut would report
one on every turn. The client sends its barge-in separately, under its own
quiet-time condition, and only while the talker holds the floor.

## Consequences

**A caller who speaks over a babble now hears nothing until it finishes a
sentence.** More silence than before, and the silence is honest: what they used
to hear was a fragment. The transcript loses the fragment's row with it.

**A held run is greppable**, on the shut-floor log line that names it. It sits
beside ADR 0211's line for a floor that never opened and ADR 0213's for a spent
budget.

**A talker that never punctuates mutes at most one answer's worth of turns.**
The bound is `TURNS_ONE_OPENER_BUYS`, reused because a mute has no business
outlasting the answer it covers.

**The Live provider's first answer now reaches the caller.** The Realtime one
gains a second, redundant opener, which costs nothing: the floor opening twice
is the floor opening.

**The wire gains one control, and the client gains one send per utterance.** It
carries no payload. The mirror tests on both sides fail if either half is
missing.

**The engine still cannot make the talker wait.** A Live talker cannot be
cancelled, so muting remains the only lever, exactly as ADR 0213 records.

## Alternatives considered

**Hold the mute until the caller completes an utterance.** The obvious rule, and
it does not work on Live. There the caller's turn end is derived from the talker
starting to speak. So it arrives in the same frame batch as the word it is meant
to gate, immediately before it. The hold would clear on the very sentence it
exists to stop.

**Clear the hold on any floor opener landing between turns.** The same failure
one step further on, for the same reason.

**Give the muted turn's text a full stop and leave the latch per turn.** It
makes the bound-resume test pass and fixes nothing. The defect is a real talker
stopping mid-sentence, which no test fixture controls.

**Re-say the muted words once the floor opens.** It answers the first failure
directly: the caller never heard them, and the engine still has them. Rejected
because a talker asked to repeat itself is the recitation ADR 0213 bounded. The
words are muted precisely when the engine trusts the talker least.

**Shorten `CALLER_WAITED_LONG_ENOUGH` after a muted run.** The caller met six
seconds of dead air, and the engine knew at second one that nothing was coming.
Deferred rather than refused: that bound is ADR 0185's, and moving it wants its
own evidence.

**Discover a speech-start frame in the Live protocol.** Better placed than the
client, being upstream of the client's own guess. Rejected on evidence: it would
need a live call and a log of unknown frames to find, and it may not exist. The
client's gate is measured, shipped and already trusted for the bubble.
