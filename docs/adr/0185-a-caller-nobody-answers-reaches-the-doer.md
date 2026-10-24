# 0185: A caller nobody answers reaches the doer, and is told so

- **Status**: Accepted
- **Date**: 2026-09-14

Narrows ADR 0181 and sharpens one sentence of it. ADR 0149's guarantee is
unchanged: every action still goes through the doer we own.

## Context

A 55-second call drew seven caller bubbles for one spoken sentence and never
answered. The event log and the two root causes are in
`docs/plans/2026-09-14-one-thing-the-caller-said-is-one-row.md`.

Two things in the shipped design let that happen.

**The caller's turn end was read off the wrong stream.** ADR 0181 says it ends
"when the talker starts speaking", meaning the model's own judgment. The code
read it off `last_output` going from absent to present, and `TALKER_IDLE` clears
that after 700 ms. So every hole in the talker's audio stream cut the caller's
sentence. Twenty-one holes produced seven rows out of one breath.

**The talker was the only route to an answer.** On a Live call the doer is woken
by `session.delegation.created` and by nothing else. A talker that neither
speaks nor delegates leaves the caller in silence, with no bound, no log line,
no event and nothing on screen. That call spent 55 seconds in exactly that
state, and the exchange header read "Requesting" until hangup.

## Decision

**A persisted caller row is one thing the caller said.** It covers everything
they say between two moves of the conversation. A move is the talker actually
saying words, the doer being asked, or the call ending. No provider item
boundary closes a row, so `voice/call.rs` accumulates rather than flushing.

**The caller's turn ends on the talker's WORDS, never on any timing.** This is
ADR 0181's rule stated so it cannot be read the other way. Every transcript
delta asks `voice/live.rs::caller_finished`, which answers with nothing when the
caller has said nothing since the last one.

**That emptiness is the ONLY gate, and a richer one is a bug.** Two were tried.
Reading the output stream resuming cut one sentence into seven rows. Reading
`talker_words` being empty is the same cut at a higher threshold: `TALKER_IDLE`
empties it, so a hole over 700 ms in the middle of one answer re-opens a turn.
Any gate the idle bound owns puts the caller's boundary back on the talker's
clock.

**A caller with nothing answering them reaches the doer anyway.** Past
`CALLER_WAITED_LONG_ENOUGH` with no talker words, no delegation and no answer,
`voice/call.rs` wakes the doer itself with what the caller said. The talker is
the fast path to an answer. It is not the only one.

**The bound is armed and disarmed on WORDS, never on anything blank.** Both
providers forward an empty delta exactly as it arrives, so a mute talker
streams them. Reading one as an answer is how the bound would never fire on the
failure it exists for. A talker turn that ended having said something disarms
it too, so a transcription landing late cannot re-arm it behind a real reply.

**What the bound spends, it spends everywhere.** `VoiceSession::caller_words_were_taken`
tells the provider its own copy is gone. A turn-less one accumulates until
something asks. Left holding them, it hands the same sentence over at its next
boundary, and one breath draws two rows.

**It refuses a parked doer, exactly as `delegated` does.** There is no turn to
start while the doer is blocked inside the card that is waiting. The caller is
still told and their words still land.

**That recovery is loud in three places**, because a silent one hides a broken
talker: a log line, the `WorkDelegated` row's own reason, and an error frame the
caller sees. A talker turn that ends having said nothing is logged on its own.

**Live forwards the caller's partials.** `session.input_transcript.delta` leaves
the provider as `VoiceEvent::UserTranscript` as well as accumulating, so a Live
call draws the caller's words as they speak. The engine holds them too, as the
only account of a caller that a turn-less protocol gives.

**Closing a session hands back what it was holding.** `VoiceSession::close`
returns its remaining events. A turn-less protocol releases the caller's last
sentence only as the socket goes. That is after the loop stopped reading, so
those words used to die with the call.

## Rationale

**The row's unit is the thought, not the slice.** A provider closes a
transcription item for its own reasons, and a semantic VAD, a Live stream and a
Whisper commit all close different ones. Anything downstream that counts rows is
then counting the provider's internals. Making the boundary a move of the
CONVERSATION is the only definition all three share.

**Silence is the one failure a call cannot recover from.** Every other failure
mode reaches a person: a refused microphone toasts, a dead socket ends the call,
a failed turn is said out loud. A mute talker looks like a working call to
everything in the system. That is why this one needed a bound rather than better
reporting.

**Waking the doer beats telling the caller to try again.** They asked a
question, and the doer is the thing that answers questions. A message saying the
voice is broken is worth less than an answer plus a message saying the voice is
broken, and both are cheap.

**The bound is not the forbidden silence timer.** That ban is about deciding
when a PERSON stopped talking, and nothing here decides that: the words handed
over are the words the provider already transcribed. What the bound measures is
our own failure to respond.

## Consequences

**The caller's row and the talker's turn are now independent.** Nothing the
talker's stream does, at any timing, can split a caller's sentence. That holds
for a provider we have not written yet.

**A working call can now start a turn the talker did not ask for.** The bound is
six seconds of a talker saying nothing at all, so a talker that acknowledges
every utterance never reaches it. One that does reach it was already failing.

**The seam grew one return value.** `VoiceSession::close` returns events, so a
new provider has to decide what it is holding rather than dropping it silently.

**One narrow Realtime case still loses partials**, because this provider's
`item_id` is not carried through the seam. The residual is named in the plan and
pinned at the mapping site.

## Alternatives rejected

**Coalescing in the client reducer instead.** The engine's row count was the
wrong number, and the transcript was drawing it faithfully. Fixing the drawing
would leave the event log wrong for every other reader.

**Nudging the talker instead of waking the doer.** A talker producing nothing is
not one a note will reach, and the nudge costs the caller another round trip
before the same silence.

**Disarming the bound on talker AUDIO as well as words.** It is safer against a
double answer and reopens the exact failure: the reported call streamed audio
deltas throughout, and the caller heard none of it. Audio nobody hears is
silence.

**Ending the caller's turn on their own quiet.** It would be a timer over a
person, which the parent plan's decision 11 bans, and the conversation already
carries a boundary worth using.
