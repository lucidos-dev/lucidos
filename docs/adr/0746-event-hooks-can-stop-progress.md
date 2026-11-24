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
