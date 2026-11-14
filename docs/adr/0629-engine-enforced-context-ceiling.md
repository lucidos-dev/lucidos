# 0629: A coding-agent session hands off at an engine-enforced context ceiling: 300k by default, set per workspace, trigger or thread and inherited by children; the turn-end gate asks for the narrative while the cache is warm, a brief replaces a narrative the session did not write, and the successor starts on the next input; amends 0505, 0539, 0560 and 0609

- **Status**: Accepted
- **Date**: 2026-11-08
- **Amends**: [0505](0505-named-handoff-checkpoints.md), [0539](0539-successor-context-by-cache-warmth.md), [0560](0560-successor-session-in-the-same-thread.md), [0609](0609-no-keep-alive-or-forced-successor-on-finished-threads.md)

Plan: `docs/plans/2026-11-08-engine-enforced-context-ceiling.md`.

## Context

A coding-agent session hands off only where its own flow says so. A flow
calls `lucidos successor offer --at <name>`, or a prose rule says "hand off
past about 300k". Both depend on the agent. The agent cannot reliably see its
own token count, and a session that never reaches its checkpoint never hands
off.

The dev workspace showed it. Session 13 of the thread "Tour v2: fix staging,
film per scene" ran Claude Code on Opus 5.5 [1m] at high effort. It opened at
about 53k and passed 300k at 10:40. It reached 522k by 12:07 with no handoff,
because hours of rig fixes and a long take kept it from its scene checkpoint.

- **The cost past 300k.** The session made 308 main-session calls for about
  $27.90 at list price. The 141 calls past 300k cost $17.35.
- **The keep-alive rode the same context.** While the session waited on a
  take, the resume route beat it about every 4 minutes. Those beats are the
  "resumed" `SessionStarted` rows. Eight of them reread about 520k each.
- **Every turn end was a chance.** The session ended a clean turn at 10:46, at
  about 335k, and at five more turn ends after it.

The pattern is general. Over 30 days in that workspace, 39% of Claude Code
main-session cache reads came from calls past 300k (20,287 calls). For Codex
the share was 15%. Every `ContextCaptured` already records the call's input
tokens, the session ordinal and the model's context window, for both backends.

## Decision

**The engine enforces a *context ceiling* on every coding-agent session.** At
the first turn end past it, the session hands off to a *successor session* in
the same thread. No flow, prose rule or agent judgement is needed.

### The value

- **Grammar:** `off`, an absolute size such as `300k`, or a window share such
  as `30%`.
- **Effective ceiling:** the value, capped at 75% of the context window the
  session's newest capture records. With no recorded window there is no cap.
- **Floor:** no ceiling sits below the handoff floor of 100k
  (`HANDOFF_FLOOR_TOKENS`). A fresh session starts near 60k, so a lower one
  would hand off in a loop.
- **Default:** `300k`.
- **The measure:** the thread's context size as `thread_context` reads it.
  That is the newest of the current session's main-session captures and
  compaction readings. A compaction lowers it, so a compacted session does not
  hand off. An unknown reading never fires.

### Where it is set

| Scope | Where | Recorded as |
|---|---|---|
| Workspace | the `coding_agent_context_ceiling` preference | the preference |
| Trigger | an optional `context_ceiling` in the trigger definition | `ContextCeilingSet` on each fired thread |
| Thread | `lucidos threads context-ceiling`, its HTTP route, the composer card | `ContextCeilingSet` |
| Child | its parent's ceiling, as it stood at spawn | read from the parent's events before the child's first |

A thread value wins over the workspace preference. A trigger value is copied
onto the new thread at creation. A child reads its parent's ceiling as it stood
at spawn, so no spawn path has to copy it. A later change at the source never
reaches a running thread. A thread with neither follows the workspace
preference live.

**Only a person raises a ceiling or turns it off.** An agent may lower its own
thread's ceiling, and set a child's at or below its own. The route refuses an
agent's raise. A higher ceiling is a choice to spend more, which is the user's
to make.

### Where it fires

**At the turn-end gate (ADR 0417), as a session re-entry.** Both backends end
their turns through it. Like every re-entry, it runs after the turn's forced
decisions and its idle. It outranks the proposal hold's re-entry. So the real
turn keeps its read decision, and a parent gets one report for it.

**The re-entry turn is an engine turn.** Like a cache keep-alive turn, its end
forces no read or wake decision. Its handoff is recorded before its idle, so
the parent fan-in reads the handoff as pending and stays quiet.

It never fires:

- mid-turn or mid-tool, and never while a question card or permission prompt is open;
- on a cache keep-alive turn, a re-entry turn or in a merge-resolution session;
- in a session's first turn, so a successor never hands off on its own opening;
- more than once per session.

Only an ordinary turn end reads the measure. The keep-alive and the input rule
below key on the recorded `ContextCeilingReached`, never on a fresh reading.

A turn that grows far past the ceiling before it ends hands off only at that
end.

### Warm narrative, cold brief

- **The narrative is written at once.** The re-entry asks the session for its
  *handoff narrative* and `lucidos successor start`. The cache is warm at a
  turn end, since the last request was seconds ago.
- **A brief replaces a narrative the session did not write.** The re-entry is
  one turn. It may end in prose, fail, or meet a user Stop. In each case the
  engine records the handoff itself, from a *handoff brief*. So the bound stays
  once per session, and the person's way back is *Return to the old session*.
- **A card in the re-entry belongs to it.** A question card or a permission
  prompt, such as one for `lucidos successor start`, resolves in the re-entry
  turn. The turn has not ended, so no brief is recorded while it is open.
- **A restart cannot skip it.** A session that reached its ceiling and has no
  handoff owes one. The next spawn records the brief handoff before it starts.
  A keep-alive beat never settles it.

### The successor starts on the next input

**A ceiling handoff starts no session at once.** The successor session starts
on the thread's next input: a user message, an event-wait wake, a background
task's completion or a child's report. Its first message is the opening, then
that input, as `PendingHandoff::first_message` already builds it.

**A session past its ceiling takes no further input.** That covers a reply, a
wake and a keep-alive beat. It holds from `ContextCeilingReached` on, through
the re-entry turn: input that arrives meanwhile waits for the successor. The
one exception is its own handoff re-entry. Card answers travel outside the
input channel, so they still reach the re-entry turn. A person can lift the
rule only by raising the ceiling.

**Running work belongs to the thread, not the session.** Event waits,
background tasks, unread results and children survive the boundary. The engine
appends a "Still running" list to the opening, so the successor knows what will
wake it, whether or not the narrative names it.

### Return to the old session

A pending ceiling handoff shows a card above the composer once the user starts
writing. Sending is the default and goes to the successor. **Return to the old
session** records `SessionHandoffWithdrawn`. It raises the thread's ceiling to
the session's size plus 100k, the Successor card's existing hold. It also
records a *Stay here* at that size, so the Successor card holds back as well.
The next input then resumes the old session.

The window cap still binds the raised value. A session already above three
quarters of its window stays past any ceiling, so the card offers no return and
the route refuses one. Turning the ceiling `off` would lift the cap, but it
would also change the thread for good, which a one-tap return should not do.

### What it does not change

- **The handoff estimate and its `pays` verdict.** The ceiling does not consult
  them, and records no `HandoffCheckpointReached`. Below the ceiling, the
  Successor card and the checkpoints work as before. A checkpoint run past the
  ceiling tells the agent to hand off there, since that boundary is better than
  the turn end.
- **The cache keep-alive's own rules.** It simply never beats a session past
  its ceiling. A copy beat for a parked card still runs, because that turn must
  finish in its own session first. A started successor is beaten like any
  session.
- **Auto-compact.** Whichever comes first shrinks the context. A ceiling above
  the compact window never trips.

### Events

| Event | When |
|---|---|
| `ContextCeilingSet` | a thread's ceiling is set, copied from a trigger at creation, raised or lowered; it names the value, the scope it came from and the actor |
| `ContextCeilingReached` | once per session, at the turn end that starts the re-entry; it names the size, the effective ceiling and the window |
| `SessionHandedOff` | gains `cause` (`context-ceiling` beside the existing kinds) and `starts` (`now` or `on-next-input`) |
| `SessionHandoffWithdrawn` | a person returns to the old session |

## Rationale

**The engine already holds the number the agent lacks.** Every capture records
it, per session, for both backends. Enforcement over prose is the same move as
ADR 0071's wake check: the engine computes the fact, so it acts on it.

**A ceiling cannot ask the estimate.** The estimate weighs units of work left,
and a turn end has no units. It also barely matters at this size. On Opus 5.5
at 300k, a handoff costs about 8 calls of saved rereads. Every other Claude
model breaks even sooner, since its write costs 12.5 times a read, not 25. A
session that carries on past 300k makes far more than 8 calls.

**Writing the narrative at once is cheap insurance.** At 335k on Opus 5.5 the
narrative turn costs about $0.15: one warm reread and about 4k output tokens.
Resuming the same session on a cold cache costs about $1.68 to rewrite it. At
10:46 the next input came 11 minutes later, after the cache had gone cold. A
handoff left to that input would have started from a weaker brief.

**Starting the successor on the next input is free.** A successor with nothing
to do would write a fresh context, then draw keep-alive beats. Deferred, it
writes once, and only if an input comes. A waiting session then costs nothing
while it waits.

**The turn end is the only safe boundary.** A tool call may hold a build, a
take or a lock. Cutting it loses work, and the agent's own narrative is the
best carry-over. The re-entry already sends agents back for plan markers and
`/harden`, so the ceiling reuses that path.

**A standing choice is not an override.** ADR 0609 rejected a forced successor
on a cold reply because it overrode the user. The ceiling is the user's setting,
`off` restores the old behaviour, and the card offers a return.

**The philosophy lens is silent here.** This is engine policy, not a surface or
an integration. Principle 1 still binds the raise rule: spending more is the
person's decision.

## Consequences

- **ADR 0505:** a ceiling handoff is not a checkpoint and adds no samples. A
  sample that spans it still follows into the successor.
- **ADR 0539:** the brief also covers a ceiling narrative the session did not
  write. The warm-or-cold choice at a Successor card tap is unchanged.
- **ADR 0560:** a handoff may start its session on the next input. Only the
  ceiling uses that today.
- **ADR 0609:** a cold reply to a session past its ceiling starts the successor
  from the narrative written at the turn end. Under the ceiling, 0609 stands.
- **Every workspace changes behaviour at once** with the `300k` default. A
  heavy session now ends in a successor where it used to grow.
- **A session that crosses the ceiling mid-turn overshoots** until that turn
  ends. The plan measures the overshoot and names when to add a mid-turn signal.
- **Prose rules like "hand off past 300k" become redundant.** Workspace
  knowhow may drop them.
- **The auto-compact trial's window arm (400k) never trips** under the default.
  Its enrolment has already closed.

## Alternatives considered

- **Keep prose rules and flow checkpoints.** Session 13 is the counterexample.
  Prose depends on an agent that cannot see the number.
- **Show the agent its token count every turn.** The decision would still rest
  on compliance. A volatile number also breaks the cached prefix (ADR 0084).
- **Gate the ceiling on `pays`.** No units of work exist at a turn end, and at
  ceiling sizes the estimate pays after a few calls anyway.
- **Set Claude Code's auto-compact window to the ceiling.** It covers one
  backend. The trial could not show a saving, and a compaction summary is
  opaque, with no narrative or brief contract.
- **Stop the session mid-turn.** It can cut a running tool and lose work.
- **Hand off lazily, on the next input.** The common case is a cold cache, and
  so a brief where a warm narrative was nearly free.
- **Start the successor right after the narrative.** It writes a fresh context
  with nothing to do, then draws keep-alive beats until a wake.
- **Stop the keep-alive past a size, with no handoff.** The wake then rewrites
  the whole context cold, about $2.60 at 520k on Opus 5.5.
- **A window share as the only default.** Windows run from 200k to over 1M. A
  share that suits 1M would sit below the floor on 200k.
