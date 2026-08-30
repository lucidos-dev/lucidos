# 0150: An agent-authored thread event names its author, so two agents can share one thread

- **Status**: Accepted
- **Date**: 2026-08-28

## Context

`EventMeta.actor` is a `MessageOrigin`. Every variant names a *human* path in or
a non-human trigger: a device, an API caller, another workspace, a linked
thread, a webhook, the engine, the system. None of them names an agent.

So an agent-authored event carries `actor: None`. That was unambiguous while a
thread had exactly one agent. Voice ends that: a *talker* and the Lucidos Agent
share one thread, and both write to it.

`engine/chat/process/history.rs` renders each turn as `User:` or `Assistant:`
from a two-valued role. With two agents writing, `Assistant:` stops being a
name and becomes a collision.

## Decision

Add `MessageOrigin::Agent { agent }`, carrying an `AgentParticipant`:

- `AgentParticipant::LucidosAgent` is the workspace's own agent, and the
  `Default`. The `agent` field is `serde(default)`, so a payload that names the
  variant without naming the participant decodes to it.
- `AgentParticipant::Guest { label }` is any other agent sharing the thread.
  `label` is the speaker name the rendered history prints.

`MessageOrigin::Agent{..}.mode()` is `ActorMode::Agent`, which is what the
variant means: an LLM decided. `history.rs` renders `LucidosAgent` as
`Assistant`, unchanged, and a guest under its own label.

## Rationale

The attribution has to be in the event, not derived at read time. Events are the
source of truth. A rule such as "a voice session's second assistant turn is the
talker" would be a guess, reconstructed by every reader. Any interleaving breaks
it.

Mis-attribution is the specific failure this prevents, and it has two shapes.
Render a talker turn as `User:` and the doer obeys an instruction the user
never gave. Render it as `Assistant:` and the doer reads it as its own prior
turn, so two models echo each other into agreement.

The variant belongs on `MessageOrigin` rather than beside it. That type is
already the actor for every thread event, and `EventMeta.actor` already holds
it. A parallel field would mean two places to look for the same answer, and a
state where both are set.

`AgentParticipant` is an enum, not a name string. The Lucidos Agent is not a
label we could get wrong or spell two ways. It is a variant, and comparing
against it is a `match`.

The change stands on its own without voice. Agent-authored events go from "no
actor" to "the Lucidos Agent", so the route popover names the actor instead of
falling back to the engine.

## Consequences

- `MessageOrigin` outgrew its doc comment, which said it captures where an
  *inbound* message entered. It now also names who authored an outbound one, and
  the comment says so.
- The frontend `MessageOrigin` union and `originMode` grow the `agent` case.
- `AgentParticipant::Guest` has no producer until the phase 5 talker exists.
  That is deliberate: a one-variant enum distinguishes nothing, and the rendering
  branch it feeds is the whole point of this ADR. Its consumer is scheduled in
  `docs/plans/2026-08-28-voice-joins-a-thread-as-a-participant.md`.
- `.claude/rules/rust.md`'s actor rule said internal state machines leave the
  actor unset. That is no longer true of the agent's own writes.

## Alternatives considered

**Leave `actor: None` and infer.** Read the talker's turns back out of the voice
session events that bracket them. Rejected: it makes every reader re-derive the
same fact, and the derivation breaks the moment a typed turn interleaves.

**A `speaker: Option<String>` field on the event payload.** Rejected: it is a
second actor field beside `EventMeta.actor`, so a reader has two places to look
and a state where they disagree. A free string also lets the Lucidos Agent be
spelled two ways.

**A fourth `EventChannel::Voice` carrying the attribution.** Rejected in ADR
0148 for a wider reason: `EventChannel` drives message grouping, so the talker's
turns would group apart from the conversation they belong to.

**A roster projection of a thread's participants.** The 2026-08-01 participants
note's shape. Rejected as out of proportion. Voice needs one variant on an
existing enum. A roster is a table, a projection and a read API, answering a
question the event payload already answers.
