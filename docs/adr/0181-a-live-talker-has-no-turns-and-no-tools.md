# 0181: A Live talker has no turns and no tools, and the seam absorbs both

- **Status**: Accepted
- **Date**: 2026-09-10

Narrows ADR 0170 to the talker it was written for. The rest of it stands, and so
does ADR 0149, whose guarantee this keeps.

## Context

OpenAI shipped GPT-Live-1 in the API. It is not a Realtime model: its model page
supports `v1/live/sessions` and marks `v1/realtime` unsupported. Adding the id to
the talker picker would have shipped an option that dials nothing.

Three of its differences reach our design.

**It has no turns.** There is no user-turn-end frame and no output-done frame,
which its own guide states. The caller's words arrive as
`session.input_transcript.delta` and simply stop. Our seam speaks in finished
utterances, and `voice/call.rs` needs both ends to write a row and to hold the
floor.

**It has no tools.** Under client delegation the API declares none. The talker's
one way to reach us is `session.delegation.created`, and that frame carries an
id and no words.

**It bills by the second.** `ApiUsage` counts tokens, and Live's own duration
figures are cumulative snapshots rather than per-turn ones.

## Decision

**GPT-Live-1 is a second provider behind the existing seam**, `voice/live.rs`
beside `voice/realtime.rs`. `voice/build.rs` picks between them from the model
id, and nothing above `voice/provider.rs` learns which answered.

**Client delegation, never Responses delegation.** The doer stays ours.

**A Live talker holds no tools, and ADR 0170's three scope to a Realtime one.**
`delegate` is the delegation frame. On a Live call the caller settles a card by
tapping it and rings off on the button.

**The provider synthesizes the two turn ends the seam needs**, and both live
below it:

- The **caller's** turn ends when the talker starts speaking, or when a
  delegation lands. Both are the model's own judgment, so the parent plan's
  decision 11 holds: no timer reads a person's pause.
  **"Starts speaking" means its first WORDS, never its output stream resuming**
  (ADR 0185). The two look alike and are not: the talker's idle bound clears
  that stream between turns, so a resumption edge fires on every hole in the
  audio. Reading the caller's turn end off one cut a spoken sentence into seven
  rows.
- The **talker's** turn ends when its own WORDS go quiet, and never when its
  output STREAM does (ADR 0187). This ADR said the stream, and that is what
  shipped and failed: the provider streams audio between turns and blank
  transcript deltas with it, so the bound never expired on a 32-second call.
  Both replies then landed as one row at the hangup. Its WORDS are a fact about
  a stream we are receiving, and still not a judgment about anybody.

**A Live turn reports zero tokens.** What prices the call is
`VoiceSessionEnded.duration_secs`, which every call already writes.

**The caller's partials are forwarded as well as held** (ADR 0185). The seam
speaks in finished utterances, and `VoiceEvent::UserTranscript` is the one thing
in it that is not one. Holding them alone left a Live caller's bubble pulsing
for the whole of a sentence.

## Rationale

**The seam was built for exactly this.** ADR 0149 said swapping the
implementation changes no socket payload and no event shape. Live is the first
real test of that claim, and it passed: `call.rs`, `api/voice.rs` and the client
protocol are untouched by this change.

**Turn synthesis belongs below the seam, because it is one provider's problem.**
Realtime states its turn ends and Live does not. Lifting a turn-less mode into
the seam would make every consumer handle a case that one implementation never
produces.

**The silence-timer ban is about the caller, and it survives.** Its reason is
that a timer cannot tell a pause from a full stop, so it must not decide when a
person finished. Both flush points here are the model deciding. A gap in the
talker's own WORDS is a different measurement with a different subject.

**ADR 0170's guarantee was never the count.** Its own words: the three tools are
each on the harmless side of ADR 0149's line, and none reaches a capability the
doer does not gate. Zero tools is further inside that line, not outside it. What
a Live caller loses is convenience, and both losses have a working surface on
screen.

**Responses delegation would have broken the line that matters.** It rents a
second model that holds tools and acts. ADR 0149 puts every action behind the
doer we own, so the managed mode is the one shape here that was never available.

## Consequences

- Two providers now answer one seam, and the model id is what picks. The rule is
  one function, `speaks_live` in `voice/build.rs`, and an unknown id keeps the
  Realtime behaviour it had before Live existed.
- **A Live call cannot be settled or ended by voice.** The card and the hangup
  button do both, as they did before ADR 0170. Settings says so on the talker
  row, since no model name can.
- **A Live call shows no spend in the usage rollup.** The duration that prices it
  is on the session's end event, and turning that into a figure is follow-up
  work.
- The default stays `gpt-realtime-2.1`. Live is offered and never chosen for
  anybody.
- `VoiceEvent::Interrupted` never fires on a Live call, so a spoken reply is
  never recorded as cut off. The model handles barge-in itself, and there is no
  frame that reports it.
- A doer answer longer than one append is split across several, because the
  provider caps `content` at 500 tokens. Repeated appends extend the same
  delegation, so the caller still hears the whole thing.

## Alternatives considered

**Add `gpt-live-1` to the picker and change nothing else.** The cheapest, and it
ships a dead option: the Realtime socket does not answer to that id. Rejected on
the model page's own endpoint table.

**Widen `VoiceProvider` with a turn-less mode.** It would let `call.rs` skip the
floor for Live, which is closer to how a full-duplex model actually behaves.
Rejected because it puts a provider's shape into the seam, and every consumer
would then handle a case one implementation never produces. Revisit if a third
provider turns up with the same shape.

**Responses delegation, declaring our three tools as
`delegation.responses.tools`.** It would have kept all three, since Live forwards
the nested tool calls and takes a `function_call_output` back. Rejected: the
tools would be declared to a rented backend model, which is the second acting
brain ADR 0149 exists to prevent. It also adds a model to pay for and a nested
event vocabulary to track.

**Read the caller's turn end off a silence timer.** The obvious fix for a
protocol with no turn ends, and the one thing the parent plan's decision 11
forbids. Rejected outright.

**Report the Live duration as tokens.** It would put a number in the rollup.
Rejected because the number would be invented: no frame carries tokens, and a
fabricated count is worse than an honest zero beside a real duration.
