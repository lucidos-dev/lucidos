# 0746: Event hooks can stop progress; work items stay in the app

- **Status**: Accepted
- **Date**: 2026-11-19

## Context

A software factory tracks work in items. In the workspace that builds Lucidos,
items lived in an app's domain events with hand-typed `status`, `blocker` and
`next` fields. Those fields drifted from the truth within hours. Items said
"waiting on the owner" after their changes had been applied, archived, or
queued to land. The engine held every one of those facts.

The engine already records most of the lifecycle: plan start, proposal and
approval with kind and scope (ADR 0691), change proposal, hardening, landing
lane, Apply, question cards, permissions, pauses and queues. It lacked a few
facts: hardening start, the running build, and what a release contains.

The factory also wants every work thread tied to a work item, refused
otherwise. Only the engine can refuse a spawn, and the engine should not learn
one tracker's model to do it.

Three facts about the event bus shaped the mechanism:

- **Many emits record a fact after the act.** About a third of the emit sites
  use `emit_or_log`. A refusal there cannot undo what already happened.
- **A transient event is never recorded.** There is no moment before its
  recording to ask about.
- **"System event" was too narrow a word.** The design's own examples,
  `MessageReceived` and `ChangeProposed`, are thread events.

## Decision

1. **The work item is not an engine concept.** The engine has no work item
   type, id, stage or placement. An app owns the record through its own domain
   events and derives stage and condition from engine facts.
2. **Lucidos's own development process stays out of the engine.** A step only
   building Lucidos takes, such as hardening or a release, is posted by the
   repository's own scripts as a domain event. The engine adds only
   primitives and facts every install has, such as which build is running.
   This extends ADR 0505's rule: the repository owns its flows.
3. **An event hook is a new engine concept.** A *hook definition*,
   `data/hooks/<slug>/hook.toml`, names one event type by its wire name. A
   thread event and a system event qualify alike. Before the engine records
   that event, it asks the hook. The hook allows, asks for input, or refuses
   and so stops that progress.
4. **One rule for every asking site.** An emit site either asks or records.
   An *asking site* asks before its act happens, so it can honour a refusal.
   A recording site writes a fact after the act, and asks no hook. Arming a
   hook on a type with no asking site is refused, and so is arming one on a
   transient type. Engine-started work has no exemption: the hook sees the
   actor and origin and decides itself.
5. **A hook carries a condition**, in the trigger `condition` syntax. The
   engine evaluates it before it starts any script, so a hook on a busy type
   costs a script start only when the event matches.
6. **A hook cannot alter the event.** An allowing hook may return domain
   events, which the engine records atomically with the hooked event.
7. **A hook can ask for data.** Its definition declares arguments, and every
   surface offers them up front. A hook may also answer "needs input": a
   person gets a picker card, an agent gets a structured tool result. Nothing
   is recorded until the input arrives.
8. **Every refusal is an event** naming the hook, the hooked event, the actor
   and the reason. A crash or timeout refuses too. No hook is asked about an
   event a hook caused: its returned events, a refusal, or an emit from its
   own process. So hooks never loop.
9. **People and agents meet the same hooks** with the same arguments.
10. **Hooks and triggers are workspace content.** A workspace authors them, or
    installs a plugin that ships them. No repository ships one. A plugin ships
    hooks in a `hooks/` content directory. The install panel lists them under
    "Can stop progress", and an update that adds or widens a hook asks again.
11. **Only the workspace owner arms a hook.** An agent asks through a
    permission card, and a plugin install arms only on the owner's
    confirmation. No hook can stop arming, disarming or removing a hook.
12. **Arming binds to content.** The armed record names a hash of the
    definition and its script. A changed armed hook refuses its events until
    the owner arms the new content or disarms it.
13. **A hook cannot widen authority.** It returns domain events only, as
    itself. Allowing grants nothing, and it never receives credentials.

## Rationale

A field someone must remember to update goes stale. Facts the engine already
records cannot. The engine's job is to record those facts completely. Deciding
what a work item is belongs to the factory.

Factories differ in what must be true before work moves on. Event hooks let
each one enforce its own rule at the moment of progress. The engine stays a
neutral recorder.

A refusal only means something before the act. An asking site is where the
act has not happened yet, so that is the only place a hook is asked. Reacting
after a fact is what a trigger already does.

An agent can edit files in the workspace, including a hook's script. Binding
the armed record to the content means an edit never runs as armed. Refusing on
a changed hook means editing a gate never switches it off.

## Consequences

- A workspace that arms no hook behaves exactly as before, at the same speed.
- Asking sites grow by need. The first three are a thread's start, a change
  proposal and an Apply request. A thread starts at its first message, or at
  `TriggerStarted`.
- Lucidos's own gates leave the engine: the plan rule, the hardening check,
  the main check and the push. Each becomes a hook or a trigger in the
  Software Factory app, which is workspace content.
- Apply works for any repository and records `ChangeApplyRequested`. A
  default landing trigger, seeded into each workspace, calls the landing lane
  to merge locally. A push or pull request after `ChangeApplied` is a
  workspace trigger. This amends ADR 0001's carve-out for external
  repositories.
- The self-hosting rebuild stays, because Lucidos rewriting itself is the
  vision.
- Every hook has a timeout under one engine-wide ceiling, because a slow hook
  on a busy event would stall the workspace.
- A hook can stop the engine's own work, such as a landing lane thread. Every
  stop is a recorded event, and the owner can disarm the hook.
- Stages without an evidence source show as not instrumented or not tracked,
  and count as unknown in measurement.
- The engine's five `WorkItem…` events describe the root thread. They are
  renamed for it, so app event types can take the name.
- Hook code lives in a leaf crate (ADR 0738). The large engine crates hold
  only the interface.

## Alternatives considered

- **A first-class engine work item.** One API for every surface, but it puts
  one tracker's model into the platform. It also reclaims a name the
  root-thread API holds.
- **Engine-owned links with an app-owned record.** It splits one record across
  two layers.
- **Refusal by convention only.** An agent can still forget, which is how
  unlinked threads accumulated.
- **A meaning-free spawn key checked by the engine.** It enforces presence,
  not validity, and still teaches the engine a factory rule.
- **A registry of hookable events, exempting engine-started work.** Safer by
  default, but it adds rules to learn. The hook already sees the origin.
- **Asking at every emit site.** A recording site writes after the act, so a
  refusal there would stop nothing and hide the fact.
- **Hooks that observe after the fact.** Triggers already react to recorded
  events. They cannot stop progress, which is the point.
- **Arming by name only.** An edited script would change what the gate checks,
  or remove it, with nobody asked.
- **Typed engine events for hardening and release.** Simpler for one app, but
  it hardcodes Lucidos's own process into every install.
- **A hand-set stage with reminders.** It still goes stale, only more slowly.

## Amendment, 2026-11-25: the ask, as built

Plan W1.1 phase 4 built the ask. Decision 12 changes, and the build settled
the points below. The knowhow is `system-knowhow/event-hooks.md`.

### Decision 12 now reads

**Arming binds to content.** The armed record names a hash of every file in
the hook's folder, at any depth. A helper the script imports is content too.
A link in the folder makes the hook invalid, since it could reach content the
hash never sees. A changed armed hook refuses its events until the owner arms
the new content or disarms it. No hook was armed before this change, because
arming refused every type, so no stored hash needed a migration.

### Where Lucidos asks

- **Four types are asked**, all thread events: `MessageReceived`,
  `TriggerStarted`, `ChangeProposed` and `ChangeApplyRequested`
  (`ASKED_EVENT_TYPES`).
- **`MessageReceived` is asked only for a thread's first message**: no earlier
  `MessageReceived` or `TriggerStarted` row on the thread. The hook's view adds
  `first_message: true`. Lucidos reads this only while a hook is armed on the
  type.
- **The ask runs just before the hooked event's own append**, outside every
  transaction. Draft acceptance commits only its acceptance event, and the
  executor records `MessageReceived` later. So the first message is asked
  there, not in `accept_saved_draft`.
- **Any message that could become a thread's first recorded message is
  asked.** A Lucidos Agent turn holds the per-thread turn-start gate until its
  starter is recorded. A second message waits behind the hook, then is asked as
  a first message itself. A coding-agent run holds no such gate, so its
  follow-up fast path asks too. The ask returns at once once the thread has a
  recorded start.
- **The queue skips its eager sub-thread message while a `MessageReceived`
  hook is armed.** The sub-thread's own turn asks and records it. An admitted
  queue entry cannot be cancelled cleanly, and a slow hook must not block the
  parent's `run_thread` call.
- **Every `propose_change` path asks**, as does the startup sweep of held-back
  work. The ask comes before the sibling discard and before
  `ChangeBroughtBack`, so a refusal moves nothing.
- **Some records stay recording sites**, because stopping them would hide the
  user's work or a failure:
  - a correction to an emptied change;
  - leftover work set aside after a thread ends;
  - the debug-only seed route;
  - the notice thread for a refused script run;
  - a failed image input.

### The token and the gate

- **`Asked` is the proof.** Only `EventBus::ask_hooks` mints it, with the
  event type, the thread and the returned events. `EventBus::emit_asked`
  checks type and thread, then appends the event and the returned events in
  one transaction (`ThreadAppendEffect::HookReturned`). Validate drops both
  together.
- **The gate lives on the bus.** It caches the armed hooks, loaded at boot, so
  the unhooked path is one in-memory lookup. An arming event clears the cache.
  With a hook armed and no runner installed, the ask refuses.
- **No script runs before the engine serves.** Boot recovery reaches asking
  sites: standing applies, Apply All batches, allowed starts, the queue and
  the scheduler. A hook script may call the engine, so the runner waits for
  the engine to listen before it starts one. Reordering boot instead would
  break Apply All's order before the switch resumes. The inline held-back
  sweep cannot wait, so boot defers it while a `ChangeProposed` hook is armed.
- **Core owns the policy, the leaf crate the mechanics.** Core asks the armed
  hooks in slug order, and the first refusal wins. The leaf asks one hook: it
  checks the content hash, matches the condition, and only then runs the
  script.
- **The refusal causes** are `refused`, `crashed`, `timed-out`,
  `invalid-answer`, `changed`, `missing-input` and `unavailable`. Until
  declared arguments land, a `needs-input` answer refuses as `missing-input`.

### The refusal

- **`EventHookRefused` is a `SystemEvent`**, aggregate `event_hook_refusal`.
  Aggregate `event_hook` would read a refusal as the latest arming state, and
  aggregate `thread` would join every last-word query on a thread.
- **Lucidos records it with an ordinary emit after the ask.** A failed record
  is logged, and the event still stops.
- **A person's refused first message** gets an error toast.
- **A refused trigger start fails the fire.** The scheduler sends its failure
  notice and records the run as failed, and the queue retries nothing. A
  successful result would record a completed run on a thread with no events.
- **A refused proposal is withheld as `hook_refused`**, through the same path
  as the plan holds. An open change the branch moved past is withdrawn, or its
  Apply would merge the commits the hook refused. The next turn end asks again.
- **A refused Apply** records `ChangeApplyFailed` and fails with the reason.

### A hook's own identity

- **A hook's events carry `MessageOrigin::EventHook { slug }`**, mode Engine.
  The ask never asks about an event whose actor is a hook. That one rule covers
  returned events and anything a hook's process emits.
- **The origin token carries the hook's claim**, as `thread@depth@hook@trigger`.
  It names no thread, so the process never acts as the thread's agent. It is
  minted at the asked event's chain depth, so the trigger chain limit cuts a
  hook and trigger loop.
- **A slug that cannot ride a header gets no token at all.** A token without
  its claim would let the hook's emits escape the guard.
- **An owner door reads a hook as an agent.** The agent arms then refuse,
  because the token names no thread.
- **No subprocess inherits the engine's own origin token or thread id.** An
  engine started inside an agent session would otherwise hand them to a hook
  with an unfit slug.

### A private copy per ask

Each ask copies the hook's folder under `.lucidos/hook-runs/`, then checks,
hashes and runs only the copy. Hashing the folder and running from it left a
window in which an agent could swap the script. `.lucidos/` is read-only to a
sandboxed agent, and so is the copy. A hook keeps no state in its folder, and
Python runs without writing its cache.

### Known limits

- With the agent sandbox off, an agent can write `.lucidos/`, so the swap
  window reopens for that population.
- With a hook armed, the gap between reading the open change and recording
  the proposal lasts as long as the hook runs. A discard in that gap lets the
  late `ChangeProposed` upsert the dead row's files. It never revives the row.
- A refused first message leaves no notice in the thread. Phase 6 adds one.
- While a `MessageReceived` hook is armed, a parent may read its sub-thread as
  idle for a moment before the child's message appears.
- An agent whose spawned sub-thread is refused gets no signal. Its
  `run_thread` call already returned, so only `EventHookRefused` records it.
- The scheduler reads a hook that timed out as a transient failure, because its
  reason says "timed out".


## Amendment, 2026-11-26: choices for declared arguments

Each first-message argument names a label and a choices script in the hook
folder. Arming covers that script with the same hash as the hook. The
choices script reads the app projection through the read-only agent
database role and runs with confined writes. Its output is a bounded list
of values and labels. The first-message hook verifies a supplied value
against the projection.

Phase 5 adds selection in the composer and arguments on each spawn surface.
Phase 6 adds inline creation with the needs-input re-ask. That keeps the
created event and the link in the same append as the first message.


## Amendment, 2026-11-26: first-message needs input

An agent spawn asks the first-message hooks before queueing. When a hook
needs an argument, the spawn tool returns its name and top choices and
leaves the thread unstarted. The hook and its choices query share that
hook's timeout budget. This replaces the phase 4 rule that
`run_thread` returns before it asks. The executor checks the hook against the event it will record.
The policy or the event's final route can change while work waits in the queue.

The first ask has one deadline across all hooks. It stops at 60 seconds
for local sends and 15 seconds for routed spawns. The shorter deadline
leaves time to return a refusal through the gateway. A remote start
that needs owner approval refuses while first-message hooks are armed.
The receiving workspace can start the thread locally.

A person gets the same request as a picker beside the held message.
Lucidos holds the message in the client's unsent store and tracks the
pending request in the workspace database. A pick or inline creation
retries the original send. Another device can still start the thread by
satisfying the hook with its own message. If that send is accepted
first, Lucidos refuses the held answer and lets its sender return the
text to the composer.

Inline creation goes back to the hook, which
returns the creation and link events together with its allow answer.
The queue executor appends these events and the first message in one
transaction. Cancelling records a refusal and returns the message to
the composer. An engine-started event has nobody to answer and records
a refusal at once.

A refusal of a child's first message reports failure through the same
parent delivery path as other child reports. A needs-input response
to a caller that can retry does not report a child failure.
