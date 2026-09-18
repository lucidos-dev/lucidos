# 0208: A call is named by its exchange, at the first answered utterance

- **Status**: Accepted
- **Date**: 2026-09-17

## Context

A thread is named from its conversation by the chat titler, which reads ONE
message. That is the right unit for something typed, where a person writes a
whole thought before pressing send.

It is the wrong unit for speech. A spoken sentence leans on the one before it,
so "Yeah, please check" is a whole request and none of its subject. Three real
calls, each named from one utterance:

| The call | What the model saw | The name it got |
|---|---|---|
| 17 utterances, about a watch and a tab-icon fix | "Yeah, please check" | Request Verification and Check |
| 6 utterances, about naming voice threads | one delegated sentence | Improving Thread Title Generation Model |
| 1 fragment, caller hung up | " So, yeah, I think" | Incomplete Conversation Opener |

The last one is worse than no name, and it is permanent: a title blocks
re-titling.

Timing was the second half of it. A call that delegates is named on the
delegated turn, and one that does not is named when the call ends. Both leave
the caller looking at their own raw first sentence while they talk.

## Decision

A call is named from its EXCHANGE: the caller's utterances and the talker's
replies, oldest first, capped.

It is named at the first caller utterance that FOLLOWS a talker reply, asked
from the call loop through a `ThreadNamer` seam. `api::voice` asks again once
the call is over, as the backstop for a call that never reached that point.

A call with only one voice in it is not named at all. The first name wins, as
it does in typed chat.

## Rationale

**The first utterance that follows a reply is the cheapest honest signal that a
call has a subject.** A person opens a call with "hey" or "what's going on", and
that names nothing. What they say after an answer is about something, and it
arrives with the answer behind it. The same signal doubles as the floor: a call
nobody answered never reaches it, so the one-fragment case declines itself with
no extra rule.

**The talker's replies are half the conversation**, and often the half naming
the thing. In the 17-utterance call above, the caller never says what is being
checked. The reply before them does.

**Naming mid-call rather than at the end** puts a name on screen in tens of
seconds instead of minutes. Waiting for the end buys a longer transcript, which
a title does not need, and costs the whole call.

## Consequences

- One entry point, `LucidosEngine::spawn_call_title_generation`, decides what a
  call is named from. Three sites ask it and differ only in WHEN.
- The call loop gains a fourth seam, beside `TurnStarter`, `DecisionResolver`
  and `CallTransport`. It stays drivable with no engine and no credential.
- A call that delegates its very first utterance is named at call end now,
  rather than on the delegated turn. That is a few seconds later than before.
- A call with one voice keeps the display fallback, which is the caller's own
  words. It is handed back to the chat titler rather than claimed, so words
  typed on that thread later still name it.
- Naming is guarded twice. The loop asks once per call, and the engine holds a
  naming slot per thread. `thread_has_title` cannot hold this alone: a name
  takes a model call, so two askers a second apart both read "no name yet".
  The chat titler takes the same slot, which also closes that window between
  two rapid typed follow-ups.
- `TITLE_SYSTEM_PROMPT` now describes transcribed speech. One prompt still
  serves both kinds of conversation.

## Alternatives considered

**Name it at the first delegation, as the chat titler already did.** Free, and
it is what produced "Request Verification and Check". A call that never
delegates never reaches it, and one that delegates late shows raw speech until
it does.

**Name it at call end.** The best possible input, and the worst timing: the
header carries a raw fragment for the whole call. An engine that dies mid-call
then names nothing at all. Kept as the backstop, not the rule.

**Name it after N utterances or N seconds.** Lands on the same material as the
chosen trigger, and adds two constants with no principle behind them.

**Feed the caller's words alone.** Cheaper by roughly half, and it drops the
speaker who most often names the subject.

**Re-title at call end when a call wanders.** A second model call and a second
event per call, for a label that moves after the reader has learned it. Typed
chat settles a name once, and a call is not different enough to justify two
rules.

**Let the chat titler branch on whether a call is live.** Refused by ADR 0149's
guard: nothing on the prompt path may read the live-session registry, in a word
or in a symbol. The chat titler asks the naming entry point instead, which
answers whether there is a call here to name.
