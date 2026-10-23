# 0184: The caller's speaking indicator follows the microphone, not the socket, and the audio behind it is held

- **Status**: Accepted
- **Date**: 2026-09-14

## Context

A call opens its devices in one order: `voice/call.ts` awaits `openAudio`, then
dials the socket. The engine answers with `session_started` only after
`provider.open(opening).await`, which is a WebSocket dial to the provider plus
two frames.

`captured()` began with `if (!isLive(state.phase)) return;`, and `isLive` is
`listening` or `speaking`. The phase leaves `connecting` on that frame. So for
the whole window the speech gate was never stepped, no *live utterance* row was
drawn, and every captured frame was discarded.

On the call in
`docs/plans/2026-09-14-the-transcript-shows-a-call-as-it-happens.md` the window
from `ThreadStarted` to `VoiceSessionStarted` was **3.79 seconds**. Somebody who
speaks the moment they press the button watches an empty transcript for that
long. Their opening words are never transcribed at all. It was reported as the
indicator lagging.

Nothing else in the path is slow. The gate opens after 3 frames of 40 ms, and
the reducer runs synchronously on that edge. `chatExchangePropsEqual` compares
the bubble's text, so the swap is never memoized away.

## Decision

The indicator belongs to the microphone. `hearsTheCaller` widens `isLive` by
exactly `connecting`, and `onSpeech` gates on it, so an utterance can begin
before the socket is up.

Audio captured in that window is HELD rather than dropped: a bounded ring of at
most `PREROLL_FRAMES_MAX` frames, flushed in order by a `flush-audio` effect on
`session_started` and emptied by `teardown`.

## Rationale

The two halves are one decision. Drawing the bubble earlier without the audio
behind it is a nicer-looking lie: the row promises a transcription that cannot
arrive, and the client withdraws it when the next utterance starts. The reader
would see a bubble appear and vanish.

Holding the audio makes the promise keepable. The provider's input buffer takes
pre-roll, and its own turn detection segments it exactly as it would have if the
frames had streamed in live.

The ring is bounded because the alternative leaks. A dial that never lands would
otherwise grow it for the life of the page. Three seconds is what the measured
window needs, and a connect slower than that has lost its opening words whatever
we do.

## Consequences

- A caller who speaks during the connect window sees their bubble inside the
  gate's own 120 ms, and their words reach the provider.
- `CallEffect` gains `flush-audio`. The reducer stays pure: it says when, and
  `voice/call.ts` owns the ring.
- `connecting` is now a phase in which an utterance can exist. Anything reading
  `isLive` to mean "the caller's voice counts" must read `hearsTheCaller`.
- A call that never connects drops its held audio at teardown, so nothing
  crosses into the next one.
- The engine is untouched. Making it answer `session_started` sooner was not
  needed, and would have been a lie of a different kind.

## Alternatives considered

**Draw early and go on dropping the audio.** One line, and it answers the
report as written. It also puts a bubble on screen for words that are never
transcribed, so the row is withdrawn when the next utterance starts. A bubble
that appears and vanishes is worse than one that appears late.

**Send `session_started` as soon as the socket is up, before the provider
session.** The client would go `listening` at once and stream audio into an
engine with nowhere to put it. The frames would be dropped one layer down
instead, and the frame would no longer mean what it says.

**Buffer on the ENGINE instead, and accept the client's frames from the
handshake.** The engine cannot: the client has not dialled it yet. The window
this covers starts before the socket exists.

**Lower `framesToOpen` so the gate opens sooner.** It is 120 ms of the 3.79
seconds, and cutting it trades a marginally faster bubble for one that flaps on
a cough. The gate was never the defect.
