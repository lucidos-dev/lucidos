# 0083: Sibling threads observe, never direct: an event states what happened, only the parent thread directs, and it judges from the shared record

- **Status**: Accepted
- **Date**: 2026-08-15

## Context

ADR 0043 gave a parent thread one privileged cross-thread write: a message to
its own direct children. It rejected sibling messaging in passing, as one
alternative among several, on a topology argument. What it did not record is the
judgement underneath: sibling messaging is the wrong shape, not merely a shape
we have not built yet.

The question came back as a claim that sibling addressing is a *missing
primitive*, discovered while running two sub-threads in parallel. It is not
missing. Three properties of the engine frame the answer, all verified against
the code rather than remembered:

- **`follow_up_child_thread` addresses direct children only.** The route loads
  the target's row and refuses unless `row.parent_thread_id == caller`. That
  caller comes from a thread-bound origin token a subprocess cannot forge (ADR
  0043's amendment). There is no sideways tool and no sideways route.
- **`await_event` matching is workspace-wide.** A thread can subscribe to any
  thread's events. Observation is already unrestricted, and always was.
- **Persisted events are append-only and timestamped.** Two children's competing
  claims land in one ordered record that a third party can read.

A live experiment on 2026-08-15 ran two children under one parent: one event
handoff at 15 ms, and one question escalated to the parent and answered back
into the still-running child. The two children never exchanged a word, and
nothing about the work wanted them to.

The objection this ADR has to answer is sharper than "we lack a tool". A child's
event carries a payload *and* can start a sibling's turn, because the sibling
may be parked on a matching `await_event`. So a child can already schedule a
sibling and hand it words. If the payload reads `{naming: "snake_case"}` that is
a fact, and if it reads "stop using camelCase" that is an order. Same wire, same
re-entry, same delivery path.

## Decision

**A sub-thread may observe any other thread and may never direct one.** Six
clauses, and the last is as load-bearing as the first.

1. **Observation is unrestricted, and stays that way.** Events, artifacts, files
   mid-edit, a sibling's transcript. Anything readable is fair to read. We draw
   no read boundary, because we could not enforce one, and a decorative boundary
   weakens the real rule beside it.
2. **An event states what happened.** This is the existing event model, not a
   new rule: events are immutable, append-only and named in the past tense. A
   reader treats a payload as a fact about the emitter's own domain, and decides
   for itself what to do. **Instructions reach an agent through its prompt,
   never through a payload.**
3. **Only the parent directs.** A child follow-up is the one instruction-bearing
   edge, it points down one level, and nothing points sideways.
4. **The immediate parent judges, at every depth.** A parent rules on its own
   children from the shared event record, never from either child's account of
   the other. A child that spawns children is their orchestrator and judges
   them, exactly as its own parent judges it. A dispute never skips a level.
5. **Everything past that is the parent's discretion, deliberately.** How to
   lead, when to follow up, when to kill a child, who edits an artifact after a
   ruling, whether to hear a second argument. We set no rules on any of it, and
   the engine models none of it.
6. **The role is called *orchestrator*; the thread is a *parent thread*.** The
   role name is new only as a formalization, since the tree already uses
   "orchestrator" in this sense in several places. There is no new entity, no
   new topology and no new event.

## Rationale

**The line is authority, not payload.** An imperative payload binds nobody. The
recipient reads it as a fact that a sibling said something, weighs it, and acts
on its own judgement or ignores it. Direction means the recipient had no say,
and no sideways mechanism has that property.

**The event model already carries the rule, so nothing new has to.** "Events are
immutable, append-only, named in past tense" is a core architectural principle
of this codebase. A payload phrased as a command is a malformed event under a
rule that predates this question. We get the content half of the rule for free,
and we invent no inspection regime to state it.

**A rogue emitter is a supervision problem, not a validation problem.** Nothing
inspects payloads for imperative mood, and nothing should. The check would be a
model call on every emit, and a determined agent routes around it in one
paraphrase. Instead the parent notices from the record and rules. That is the
same shape as every other judgement here, and it needs no new machinery.

**A judge exists so that a disagreement terminates.** Two children arguing
sideways have no stopping rule, and each round costs a turn. One judge with the
whole record ends it in a single exchange. This is the practical reason for the
hierarchy, sitting under the structural one below.

**Authority flows down from the user, and a mesh has nowhere to attach it.**
Principle 1 of `docs/philosophy.md` is that nothing consequential happens
without user intent. It says plainly that Lucidos is not a fleet of autonomous
agents acting on your behalf. A tree keeps that property, because every
instruction traces up an unbroken chain to a person. Peer instruction between
agents is exactly the fleet, and logging it does not turn it back into
delegation the user granted.

**Reading a sibling's transcript is allowed because the alternative is theatre.**
It is observation by the letter and something coarser in spirit, and we still
allow it. Every mechanism that would enforce a read boundary is absent: the
filesystem is shared, the event query has no aggregate predicate, and a
coding-agent subprocess can read the database. A rule we cannot enforce, stated
beside one we can, teaches a reader that neither is real.

**Nothing models a question versus a dissent, on purpose.** "I am stuck" halts a
child. "I think my sibling is wrong" does not. That is a real distinction about
what the child does next, and it is not one the engine needs to know. Both are
the child saying something its parent may act on, and the existing edges carry
both. A new event type would freeze one workflow into the platform, for a
judgement the parent makes better in context.

## Consequences

**What we keep.**

- The topology stays a star at every level, which is what ADR 0043 bought. This
  ADR gives that shape a reason rather than a scope.
- Every instruction has a traceable origin: a person, then a chain of parents.
- Two children's competing accounts are settled from one ordered record, not
  from whichever spoke last or loudest.
- Observation stays free, so the fast path costs nothing. The measured handoff
  between two children was 15 ms, through the event bus, with no parent in the
  loop.

**What we give up, knowingly.**

- **The parent serializes.** Ten children with something to say are ten
  re-entries through one context, and the parent pays full context each time.
  Accepted
  as the price of one judge and one record. The parent can spawn fewer children,
  or decline to subscribe to chatter and read the log on its own schedule. We
  prescribe neither.
- **A ruling costs a round trip.** The parent must be re-opened, must read, and
  must deliver, where two children could in principle have settled it between
  themselves in one message. That message is the thing we are refusing.
- **A determined child can still act on a sibling's work.** It cannot address
  the sibling, but it can read everything the sibling produces, and write
  wherever it has write access. The rule constrains addressing, not effect.

**What is enforced, and what is only convention.** This gap is where the rule
will erode, so it is stated flatly rather than smoothed over.

| Property | How it holds today |
|---|---|
| A child cannot deliver a message into a sibling's inbox | **Enforced.** One equality against an unforgeable caller, on the tool and the route alike. |
| A payload states a fact rather than an order | **Convention.** Nothing inspects payloads. The parent is the check. |
| A child does not act on an imperative payload | **Convention.** Prompt-level, and this ADR plus `system-knowhow/orchestrating-sub-threads.md` are that prompt. |
| A child cannot archive or cancel a sibling, or its own parent | **Enforced** since the amendment below. Was not enforced at all when this ADR was accepted. |

**The destructive verbs are ungated, and that is a defect rather than a
decision.** (Was true when this ADR was accepted. Fixed by the amendment below,
and kept here as the record of what the gap was.)
`POST /api/v1/threads/archive` and `POST /api/v1/chat/cancel` both
take any thread id and run no parent-child check. The actor resolved from
headers is display attribution only (ADR 0050). No LLM tool exposes either, so
no agent reaches them by accident. A coding-agent subprocess can still curl the
loopback and archive a sibling, or its own parent.

So "the parent can shut a rogue child down" is true for the wrong reason: it
can, because anyone can. The write we designed carefully is the locked one, and
the older writes are open. Both are owed the ladder the follow-up edge already
has. That means a caller bearing a thread-bound origin token reaches its own
descendants and nothing else, with a user device unaffected. Logged in
`docs/known-gaps.md` until it lands.

**A child that spawns children is not a loophole.** It becomes an orchestrator
of its own subtree, which is clause 4 recursing. It gains no address it did not
have: its own child can write only to *its* children, and a sibling is not among
them. What it does gain is the ability to duplicate a sibling's work in a
subtree it controls. That is a scoping mistake by the parent who handed out two
overlapping jobs, and it shows up in the record.

## Amendment, 2026-08-17: the destructive verbs are gated, and the table row moves

The defect this ADR logged is closed. `POST /api/v1/threads/archive` and
`POST /api/v1/chat/cancel` now run the ladder, in `api::thread_reach`. The
paragraph above and its original table row stay as the record of what was open,
and this amendment states what is true now.

**A caller bearing a thread-bound origin token reaches itself and its own
descendants, and nothing else.** Anything further is a 403 carrying a message
written for the agent that reads it. So the enforced row of the table is no
longer only about messaging: a child cannot deliver into a sibling's inbox, and
it can no longer end a sibling's work either.

Four details worth having in the record:

- **Descendants, not direct children.** The follow-up edge deliberately stops at
  one level (ADR 0043 refuses a grandchild edge, and a grandparent goes through
  the child). Archive cascades to the whole family, so its authorization has to
  reason over the same tree it tears down. The two scopes differ because the two
  blast radii differ, not because the topology changed.
- **A user device is unaffected.** A caller presenting no token keeps the reach
  it always had. That is the answer `refuse_event_waits_for_another_thread`
  already gives, and the one the un-tokened local API surface gets everywhere
  else.
- **The unscoped cancel is refused, not reinterpreted.**
  `POST /api/v1/chat/cancel`
  with no `thread_id` stops every thread in the workspace, which is the user's
  global Stop. A thread-bound caller asking for it is told to name a thread,
  rather than being quietly narrowed to itself.
- **One residual, and it belongs to the whole surface.** `/api/v1` carries no
  authentication (`docs/glossary.md` § unattributed caller, ADR 0050). So a
  subprocess that DROPS its origin token reads as an ordinary local caller and
  keeps full reach. Every route here has that property, and closing it is owed
  its own ADR. That ADR is 0169. The fix would require device attribution on the
  un-tokened path, refusing the e2e suite and every external client. What the
  gate buys is that a caller presenting its credential is bound by it.

Coverage is named rather than claimed, for the reason ADR 0043's own amendment
names it: only a spawned subprocess holds a token, and a test-only minter was
rejected there. So the agent paths are covered in the engine tests
(`api/thread_reach_tests.rs`) and the API e2e suite covers the user-device path.

## Superseded in scope by ADR 0168

This ADR governs what one agent may SAY to another, and that half stands
unchanged. It never classified the verbs that are not an agent speaking: Apply,
Discard, answering a question card, resolving a permission card, restarting a
turn. ADR 0168 classifies them, and two of its findings correct this document.

The refusal above tells an agent to "say so to your parent thread", which a
*top-thread* does not have. Under ADR 0168 the workspace is the root, so two
top-threads are siblings and their shared parent is the *workspace owner*. The
enforced-versus-convention table also has no row for those five verbs. Read this
document for the sibling-addressing rule, and ADR 0168 for everything wider than
a thread's own subtree.

## Alternatives considered

- **Sibling-to-sibling messaging.** Rejected here on authority, having been
  rejected in ADR 0043 on topology. An instruction between peers has no origin
  the user granted, and it gives a disagreement no stopping rule. Not a slippery
  slope by construction: the authorization check is a single equality against
  the caller, so a sibling edge needs a new predicate rather than a loosened
  one.
- **Sibling negotiation, with the parent as a fallback.** Rejected. It is the
  same edge with an escalation clause bolted on, and the clause is what never
  fires. Two agents that can keep talking will keep talking, and the parent
  learns of the disagreement after the tokens are spent. If the parent must read
  the record to rule, it may as well rule first.
- **A dissent event distinct from a blocking question.** Rejected: nothing
  should model it. The two differ in what the child does next, which the child
  already controls, and both travel on edges that exist. See the rationale.
- **Validating payloads for imperative mood at the emit boundary.** Rejected. It
  costs a model call per emit, and one paraphrase defeats it. It also moves a
  judgement into a filter with none of the parent's context.
- **An engine-level appeal channel.** Rejected. Whether to hear a second
  argument is the parent's call. Any mechanism a child could use for it is just
  another message to its parent, which it already has.
- **The root orchestrator as judge at every depth.** Rejected. It skips levels,
  contradicting ADR 0043's refusal of a grandchild edge, and it puts every
  dispute in the tree through one context. The immediate parent has the context
  for its own children and nobody else does.
- **An atomic claim or lease over the log, so two children cannot claim one
  file.** Rejected, and deliberately not designed. It does not exist today and
  nothing here needs it. A parent that hands two children the same file made a
  scoping mistake, and the fix is the scoping. Recorded so the next person gets
  the reasoning instead of building it on a hunch.
- **A shared pull surface children can read.** Already rejected in ADR 0043, and
  unchanged: a surface every child can read and write is a sideways channel
  whatever the wire shape. Restated only because a "message board" reads as
  observation and behaves as addressing.
- **Batching the fan-in so ten dissents cost one re-entry.** Rejected as
  premature.
  The cost is real and recorded above, and the parent already has every lever it
  needs to avoid paying it.
- **Calling the child threads "lanes".** Rejected on vocabulary. The word is
  already spent four times in this tree: `RiskLane`, the command permission
  lane, the two coding-agent lanes, the delivery lane. There is no new entity to
  name either. It is a parent thread, a child thread and a sibling, all three
  already canonical.
