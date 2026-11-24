# 0797: The call loop in `voice/call.rs` talks to the talker, caller, doer and decisions through separate seams

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0149](0149-a-rented-talker-beside-the-agent-we-own.md), [0170](0170-the-talker-holds-a-tool-set-and-none-of-it-acts.md), [0185](0185-a-caller-nobody-answers-reaches-the-doer.md), [0205](0205-a-live-delegation-settles-what-is-waiting.md), [0213](0213-one-opener-buys-one-answer.md)

## Context

A voice call runs from the opening payload to the paired end event. In that
time the loop has three live inputs: the caller, the talker and the thread. The
thread is the third party in the room.

A finished utterance wakes the thread's own agent through a `TurnStarter`. What
that agent produces comes back over the EventBus.

This record restates the design comments of
`crates/lucidos-engine/src/voice/call.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- The loop is provider-agnostic above its seam and socket-agnostic below it. It
  talks to a `VoiceProvider` on one side and a `CallTransport` on the other.
- `api::voice` supplies a WebSocket transport. The tests supply a scripted one.
- `run_call` takes the opening payload and does not build one. `opening_for`
  builds it: the stable persona plus the resident block, built fresh from the
  workspace as it is now.
- The loop selects over three things: the caller, the talker and the thread.
- What the thread is waiting on goes through a third seam, a
  `DecisionResolver`. A card the caller can settle is read and resolved there,
  never by this file reaching into the engine.
- A delegation meets the same seam while the doer is parked inside a card of
  its own. `Call::settle_or_refuse` handles it, so a talker holding no
  answering tool settles one anyway.
- The caller, the talker and the thread share one struct of floor state, not a
  column of arguments.
- Held talker audio has a size bound. Above about twenty seconds of 24 kHz
  16-bit audio, the hold is decided as unheard.
- `run_call` returns the reason the call ended, or `None` when the talker never
  answered at all. In the `None` case it writes no events.

## Rationale

- The loop is exercised without a socket or a credential, because both sides
  are seams.
- Assembling the resident block reads half the workspace. Driving a call needs
  none of it, so the two are kept apart.
- The caller, the talker and the thread all read and write the same floor
  state. Passing it around by hand is how the two halves of one rule drift
  apart.
- The audio bound is a memory bound, far past any first sentence. It is never a
  judgment of what the talker is saying, since no length separates an answer
  from a recitation (ADR 0213).
- A start with no call behind it would make the paired start and end count
  sessions that never happened.
- Four arguments to `run_call` are the seams: the talker, the caller, the doer
  and what is waiting. Each is a separate thing a call can be given.

## Consequences

- A test names which seams it stands in for, because each seam is its own
  argument.
- A call the talker never answers leaves no trace in the event pairs.
- Changes to what a call opens with happen in `opening_for` and do not touch
  the loop.
- The loop never reaches into the engine to read or resolve a waiting card. It
  goes through the `DecisionResolver`.
- The audio bound is not a place to decide what the talker may say.

## Alternatives considered

- Bundling the four seams into one argument was rejected. It would hide which
  ones a test is standing in for.
- Building the opening payload inside the call loop was rejected. It would pull
  a half-workspace read into every driven call.
- Passing floor state around as separate arguments was rejected. It lets the
  two halves of one rule drift apart.
- Bounding held audio by judging its content was rejected. No length separates
  an answer from a recitation.
