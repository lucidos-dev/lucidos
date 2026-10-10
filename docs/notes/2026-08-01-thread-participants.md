# Thread participants: roster, roles, and cross-instance replication

- **Status:** Note. An architecture discussion written down, not a plan. Nothing
  is scheduled, nothing is implemented, and no decision has been made. If any of
  this is ever built it needs an ADR for the decisions and a real plan under
  `docs/plans/` for the work.
- **Date:** 2026-08-01
- **Where it came from:** the Buzz architecture review
  (`artifacts/research/buzz-fit-assessment.md` in the dev workspace) and the
  voice discussion.
- **Constrained by** ADR 0003 (see Non-goals).

## Problem

A thread today has an initiator and a per-event actor. It has no notion of a
*set* of entities present in it, no notion of what each is allowed to do, and
no notion of who a given event is *for*. Three consequences:

1. Two coding agents cannot share a thread. The second one is refused by a
   hardcoded string, `"A coding agent is already running for this thread"`
   (`engine/claude_code/mod.rs:413`), enforced by a TOCTOU-guarded map insert
   (`engine/agent_session/run_session/run.rs:283`). That is a mutex pretending
   to be an error message: the write token exists, but it is unnamed, owned by
   one subsystem, and invisible to the rest of the engine.
2. Voice has nowhere to sit. It is a consumer of a thread's event stream that
   renders to speech and mostly calls nothing. There is no role for that.
3. Nothing can cross an instance boundary, because an asserted actor is
   meaningless to a peer.

## What already exists (do not rebuild)

- **`MessageOrigin`** stamps every persisted event with its origin and already
  distinguishes exactly the entities in question: a named device, the Lucidos
  Agent, the engine, another thread (`ThreadLink`), another workspace
  (`Workspace`). Optional on `SystemEvent` as `actor: Option<MessageOrigin>`.
- **`ActorMode`** (`Human` / `Agent` / `Engine`) is the derived UI-label
  classification.
- **Total ordering per thread** via sequence numbers plus emit-before-ack
  (`LucidosEngine::pre_emit_chat_message_received`). Concurrency inside one
  instance is a solved problem.
- **Replay** is free: the event log is durable and queryable by id, so
  "everything after event X" is a query, not a subsystem.
- **`ThreadSummary.initiator`** is `LegacyInitiator { User, System }`, whose own
  doc comment defers promotion "until there is a UI need". This is that need.

So the **sender** direction is done. What is missing is the set, the
capabilities, and the receive direction.

## Model

### ParticipantId

Defined as a *projection of `MessageOrigin`*, not a new identity namespace.
This is the load-bearing decision: it makes the roster a fold over events the
engine has already been writing for months, so historical threads render
participant chips correctly on first boot, with no migration and no backfill.

### Roles

| Role | May mutate workspace | May call tools | Writes to thread | Typical holder |
|---|---|---|---|---|
| **Driver** | yes | yes | yes | the coding agent holding the token |
| **Contributor** | no | read-only | yes | second coding agent, reviewer agent |
| **Narrator** | no | no | rarely | voice, remote viewer, peer instance |
| **Principal** | via approval | n/a | yes | the human |

- **Driver** holds the write token. Exactly one per thread. Owns a worktree,
  proposes changes. Handoff is an event, so the log shows who had the keys when.
- **Contributor** unlocks something inexpressible today: a second coding agent
  reading the driver's diff and commenting, with no worktree of its own and no
  lock contention.
- **Narrator** subscribes and renders. When a narrator needs real work done it
  does not take the token, it spawns a thread whose agent is that thread's
  driver. This is exactly the voice design's quick-turn / heavy-turn split
  (`docs/notes/2026-06-01-voice-control.md`).
- **Principal** never needs the token. Human acts (write, answer a question,
  resolve a permission, Apply, veto) are already distinct event types; the role
  names the authority they carry, it does not route them.

### On the mob-programming analogy

It is inverted here and the difference matters. In mob programming the driver
types and explicitly does *not* decide; the navigator decides. Ours is the
reverse: the driver has the judgment and the hands, and the human navigates by
intent and verifies the result. The vocabulary transfers, the power
relationship does not. Recorded so nobody reasons from the wrong half of it.

## Sketch 1: roster as a projection

Fold `actor` over a thread's events into a participant set. Render as chips.
No new writes, no schema change beyond a projection table if the fold is too
slow to do live. Ships something visible without touching either turn loop.

## Sketch 2: the driver token, made explicit

Replace the map-insert mutex with a named lease held in thread state and
granted by an event. The refusal string becomes a typed outcome. Each loop asks
the roster exactly one question, "am I holding the token". Contributor role
becomes expressible at the same moment, because "may act but may not mutate" is
now a value rather than an absence.

## Sketch 3: keys and cross-instance replication

**Why keys.** Not crypto for its own sake: `actor` is *asserted* today and that
is fine within one trust boundary. Across instances it is worthless. Signing
makes `MessageOrigin` unforgeable. Tailscale already provides transport and
rendezvous (direct WireGuard, DERP fallback that cannot decrypt); what it
cannot provide is per-thread authorization, since tailnet membership is
all-or-nothing. The new object is a signed, scoped, expiring subscribe grant.

**Replication shape: partition by author.** Each instance appends only the
events it authored and ships them to grant holders. Every instance holds the
union. No two instances ever write the same event, so there are no conflicts.
Order within an author is their sequence; order across authors comes from each
event naming the latest events it had seen, giving a happens-before DAG (git's
model, and Nostr's), with a deterministic tiebreak for rendering. This is a
grow-only set of signed facts: the most boring CRDT available, and it is the
"immutable facts cross, each log owns its copy" rule generalized correctly.

**The driver token is not replicated.** Mutual exclusion is consensus, and two
instances both believing they hold the token means two worktrees proposing
changes to one thread. Instead the token is *granted*: the thread's home
instance (its creator) is the sole grantor, and a grant is a lease with an
expiry, emitted as an event. This keeps distributed consensus out of the design
entirely, and it matches physical reality, since worktrees and subprocesses are
local to a machine anyway.

**Offline is fine, and this is where we beat Buzz outright.** Their observer
stream is kind 24200, ephemeral, Redis pub/sub, never written to Postgres
(`crates/buzz-core/src/kind.rs:443`): miss the moment and the frame is gone.
Ours is durable on both ends, so catch-up is a range query.

**The consequence to decide deliberately.** Replicating a thread into a peer's
instance puts it in their memory layer, embeddings, and search index. It is
ingested, not merely rendered. Scope must be a property of the grant, not a
side effect of subscribing.

## Open question (the actually hard part)

**Whose text enters whose prompt.** Once a driver, a contributor, and a
narrator all write into one thread, each participant's context is a *filtered
view* of a shared log, and the filter is a per-role design decision with no
obvious default. Get it wrong and every participant pays tokens to read every
other participant's chatter. This, not the roster and not the token, is the
part that needs thinking before implementation.

The concurrency is not hard: sequencing and ordering are already solved.

## Non-goals

- **Not a loop framework.** ADR 0003 rejected a shared turn orchestrator across
  the agent-session loop and the chat agentic loop. Participants is attractive
  enough to become the excuse to revisit that; it must not. This is a registry
  and a token. Each loop asks one question and is otherwise untouched.
- **No relay.** No shared source of truth, no channels, no membership service.
  Instance to instance, each log sovereign.
- **No new identity namespace.** `ParticipantId` projects `MessageOrigin`.

## Relation to voice

Voice is one narrator. The fan-out it needs (subscribe to a thread's events,
render them) is the same fan-out a peer instance needs. Build the subscription
once, give it multiple renderers: speech for voice, cards for a viewer,
signed frames for a peer.

One correction to the voice note while it is open: it rejected WebRTC on
the grounds that "its latency edge only matters against a remote server, which
ours isn't". From a phone over the tailnet, the engine *is* remote. The
conclusion still holds (session state belongs server-side), but the stated
reason is wrong and should be restated before it is cited as precedent.
