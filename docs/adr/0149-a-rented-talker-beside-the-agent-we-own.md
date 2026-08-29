# 0149: The talker is rented and tool-less; the reasoner is the standard Lucidos Agent, unmodified

- **Status**: Accepted
- **Date**: 2026-08-28

## Context

A voice conversation needs a model that hears and speaks with low latency. The
Lucidos Agent is not that model. It runs a full agentic loop with a large system
prompt, a tool registry and knowhow routing, and a spoken turn cannot wait for
it.

The obvious shortcut is to give the fast speech model the tools, so it can act
on what it hears. The 2026-06-01 voice note went further and proposed a slim
voice-specific orchestrator with its own curated toolset.

Of the shipped voice products, only OpenAI's GPT-Live splits the two. Its voice
model holds the conversation and delegates deeper work to a frontier model
behind the scenes. Claude's voice mode and Hermes Agent both run the real agent
loop and accept the latency instead.

A split raises a question a single model never has to answer. If two models
share a thread, does the user meet one participant or two?

## Decision

Two models share one thread, and the split is by capability, not by speed.

The **talker** is rented. It hears, it speaks, and it does nothing else. It
holds no tool schemas, so it can mutate nothing. It is opened with an empty tool
list, for every `VoiceProvider` implementation.

The **reasoner** is the standard Lucidos Agent, unmodified. It owns every tool
and does all the work. It is never told a voice session is live. It is shown
one, by reading the talker's turns in the thread.

**The user meets one entity.** The talker speaks as Lucidos, in the first
person, and the persona never changes hands. It may stall truthfully while work
runs, because work really is running on its behalf. It may not state a fact it
did not receive from the reasoner. The split attribution in ADR 0150 is
internal, and the user never meets it.

## Rationale

This is the canonical Talker-Reasoner split, and the reason it is canonical is
that the two roles have different failure modes. A talker that gets something
wrong says a wrong sentence. A reasoner that gets something wrong sends an
email.

A tool-less talker cannot make the second kind of mistake. That is a structural
guarantee, not a prompt we hope holds.

Keeping the reasoner unmodified is the other half. The philosophy rule is *own
the surface, rent the model*: the model that speaks is rented and replaceable,
and the agent that acts is ours. Telling the reasoner that voice is live would
break that, because the same question would then get two answers depending on
how it was asked. The user would have no way to predict which one they get.

A second toolset is also a second thing to keep correct. Every tool added to the
engine would need a decision about whether the voice model gets it too. There is
no such decision here, because the answer is always no.

One entity is the right presentation because the user did not ask to talk to a
proxy. They asked Lucidos something. Two names for one product is a second brand
for the same thing, and it makes ordinary sentences strange: a narrator reading
a pending question aloud has to attribute it to somebody else.

The honesty constraint is what the narrator framing was really protecting, so it
survives as a rule instead of a persona. A tool-less talker knows nothing
first-hand. "I checked your calendar" is therefore false, and the first person
makes a wrong summary sound confident. Forbidding the claim costs nothing that
third person would have bought.

A tool-less talker also cannot look anything up mid-sentence. So whatever it
answers instantly has to be resident in its session at open. That resident block
is the whole of what voice answers with no wait, and phase 3 of the plan defines
it.

## Consequences

- Nothing spoken can act. Every action goes through the reasoner and its
  ordinary turn, with its ordinary admission and its ordinary events.
- A spoken turn is slower to *act* than a purpose-built voice agent would be. It
  is not slower to *answer*, because the talker replies immediately.
- The talker needs the reasoner's progress narrated to it, or it has nothing to
  say while work runs. That is a real cost and it is what the appended summaries
  in phase 5 pay.
- Swapping the rented model changes no engine behaviour, because no engine
  behaviour depends on it.
- The reasoner's assembled system prompt is byte-identical whether a session is
  live or not. A test asserts it.
- The transcript renders a spoken turn as Lucidos, because that is who the user
  heard. The agent actor stays on the event, so the route popover can still say
  the turn was spoken.
- Every spoken turn starts a reasoner turn, because the engine decides and the
  talker cannot. GPT-Live's talker delegates, so it can skip the frontier model
  on a cheap turn. Ours cannot, and the plan carries that as an open question.
- What voice answers instantly is bounded by the resident block, so that block
  is a product decision rather than a tuning detail.

## Alternatives considered

**One model doing both.** A single fast speech model with the full toolset.
Rejected: it puts every tool behind a model chosen for latency, and it puts a
rented model in charge of mutating the workspace.

**A slim voice-specific orchestrator with a curated toolset.** The 2026-06-01
note's proposal. Rejected: the curated list is a second registry to maintain, and
every new engine tool becomes a judgment call about whether voice gets it. It
also splits the answer to one question across two agents with different
capabilities.

**No split at all: run the real agent and eat the latency.** What Claude's voice
mode and Hermes Agent both do. Respectable, and it buys full tool access on
every spoken turn. Rejected because a spoken turn that waits for the whole
agentic loop is dead air, which is the failure mode voice has to avoid first.
The `Cascaded` implementation is close to this shape, and the seam keeps the
option open.

**The talker as a narrator, reporting on the reasoner in the third person.**
Structurally honest: it never claims first-hand knowledge, because it has none.
Rejected: it costs the single-entity presentation for a problem a prompt
constraint solves, and it makes reading a question aloud awkward. GPT-Live keeps
the persona with the voice model for the same reason.

**Telling the reasoner that voice is live.** Tempting, because it could then
write shorter replies. Rejected: it makes the agent's answer depend on the input
channel, which is the one thing a user cannot see and cannot predict. Shortening
for speech is the talker's job, and the talker is the one with the microphone.
