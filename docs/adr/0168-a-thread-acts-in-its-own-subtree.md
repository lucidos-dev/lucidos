# 0168: A thread acts in its own subtree on its own authority; anything wider is the workspace owner's button, and a thread may press it only while carrying a standing instruction from the owner

- **Status**: Accepted
- **Date**: 2026-08-30

## Context

ADR 0083 settled what one agent may say to another. It never classified the
verbs that are not an agent speaking at all. Applying a change, discarding one,
answering a question card, restarting another thread's turn: those four are the
user's own answer, and no ladder was ever fitted to them.

The question came back out of the voice plan. Its decision 9 deferred
cross-thread answering, because what text may do outside its own thread was
itself unsettled.

Everything below was measured against a live workspace's event log. Four
findings changed the shape of the answer.

**The remedy ADR 0083 offers names nobody.** Its refusal ends with "say so to
your parent thread and let it decide". `api::thread_reach` prints the same
sentence at every 403. That workspace holds 5410 active threads with no parent
and 974 with one. For 85% of them the sentence points at nothing.

**The case ADR 0083 argues about does not occur.** Of 342 `EventWaitDelivered`
rows, 142 carried an event another thread emitted. None of those 142 were two
children of one parent. 91 were two top-threads. 26 were a thread waiting on its
own child. Applies say the same: of 365 done by an agent, 264 stayed inside its
own subtree, and none of the 101 that went outside was a same-parent sibling.

**Two gates exist, and each covers a different incomplete set of routes.** Reach
is enforced on five verbs and absent on four. Attribution is enforced on the
chat path and nowhere else. `api::chat::HUMAN_MODE_UNATTRIBUTED` states the rule
out loud, telling an agent not to post as the user. Four routes let it do
exactly that.

**`continue_thread` falsifies the obvious sort.** Sorting by who is acting puts
archive, cancel and child follow-up on the ladder. It leaves the user's own
answer off. That sort predicts a gate on `continue_thread`, which restarts
another thread's turn and is nobody's answer to anything. It has none. So the
code's real rule was not a sort: it was whichever route someone remembered to
gate.

## Decision

Six clauses. The first two are vocabulary the rest depends on.

1. **The workspace is the root.** Every *top-thread* sits directly under it, so
   two top-threads are **siblings**. The root is a container and never a place
   work runs. Nothing holds a turn there, and nothing can be delegated to it. It
   gets no row.
2. **The workspace owner is the authority.** *Thread reach* answers which
   threads a caller may aim at. It never answers on whose behalf. Those are two
   questions, and the second belongs to a person.
3. **A thread acts inside its own subtree on its own authority.** Unchanged from
   ADR 0083 and its amendment. This is the whole of what a thread holds by
   itself.
4. **Anything wider is the owner's button.** Apply, Discard, answering a
   question card, resolving a permission card, restarting a turn, archiving,
   cancelling, and creating a top-thread. None is thread-scoped, because the
   root is not a thread. Each needs evidence of the person, not a place in the
   tree.
5. **A thread may press one only while carrying a standing instruction from the
   owner.** Two shapes qualify, and no third. A turn the owner opened, where
   their words in that turn are the press. A *trigger* firing the owner
   authorized, which is the same decision made in advance.
6. **A standing instruction spends the owner's authority while they are away.**
   A turn opened at 22:00 can act at 03:00, and a nightly trigger acts with
   nobody watching. Recorded as a consequence rather than discovered as a
   surprise. It is the release case working, not a leak.

## Rationale

**The sentence costs the measured workflow nothing.** Run it over the same log.
264 in-lane applies pass with no owner instruction required. 72 applies had one
top-thread applying another's change. 63 of those ran inside a turn whose last
message came from a registered device. The other 9 predate origin recording.

All 27 agent-created top-threads ran inside an owner-opened turn, and so did
each of the 8 question cards an agent answered. Trigger threads applied 107
times, and 92 of those stayed inside their own subtree under clause 3. Clause 5
covers the remaining 15. The only clear failures left are 32 applies from a bare
curl.

**Out-of-lane action was never orchestration. It was a missing button.** 26
threads did those 72 applies, and they are named for the job: applying other
threads' work and releasing. They spawned almost none of the threads whose work
they applied. The workers were started first, and the orchestrator was created
afterwards.

Their authority was never in doubt, and the missing piece was the relationship.
The relationship is not what makes the act legal: the owner's instruction is.

**A root with a row would be a fleet controller.** Whatever ran there would
reach every thread in the workspace by clause 3 alone. `docs/philosophy.md`
refuses exactly that, and ADR 0083 already quotes it. Keeping the root row-less
makes the refusal structural rather than a rule someone must remember. It also
keeps a *voice session* out of the same seat, which ADR 0148 decided on its own
grounds.

**The workspace is the root rather than the person.** A thread can live here and
answer elsewhere. 132 active top-threads arrived from another workspace, which
vouches for its own human through `caller_workspace`. Those threads sit in this
tree and trace their authority out of it. Naming the person as root would claim
they belong to someone who never opened this workspace. Naming the container
says what is true, and leaves authority to the owner clause.

**A trigger is a standing instruction, exactly like the Apply All checkbox.**
The owner wrote it and switched it on, so a firing is their decision made in
advance. Both are checkable: a trigger has a creator and an enabled state.
Treating the two alike is why clause 5 needs no special case for a schedule.

**The loop worry closes structurally.** Two threads cannot escalate at each
other, because neither is the root. Neither can supply the other's evidence.
That removes the need for a counter, a depth cap or an admission bound here.
`max_event_trigger_depth` caps concurrency rather than exchange length, so it
would never have caught a two-thread ping-pong.

**Attribution is what makes clause 4 meaningful.** Sorting by who is acting only
works when the engine can tell who is acting. On those four routes it cannot,
and dropping the origin token currently buys more than presenting it. Closing
that is ADR 0169. Clause 4 is not enforceable until it lands.

## Consequences

**What we keep.**

- Every instruction traces up an unbroken chain to a person. For the first time
  that is checkable rather than conventional.
- The release workflow is legal for the right reason. The orchestrator relays
  the owner's instruction, rather than claiming standing over its siblings.
- Observation stays unrestricted, exactly as ADR 0083 left it.
- The voice plan's decision 9 is unblocked. A caller answering another thread's
  card is the owner answering, through a thread carrying a turn they opened by
  speaking. No thread-to-thread edge opens.

**What we give up, knowingly.**

- **A standing instruction holds authority while the owner is away.** Clause 6.
  The alternative was a confirmation card, weighed and rejected below.
- **The engine checks the instruction, never its scope.** It confirms that the
  owner opened the turn or authorized the firing. It never confirms they asked
  for this particular act. Naming the gap is the honest way to accept it.

**What changes elsewhere.**

- **ADR 0083's refusal text and its enforced-versus-convention table.** The text
  sends an agent to a parent a top-thread does not have. The table has no row
  for the four verbs above. Both become wrong rather than incomplete, and both
  are amended in the change that lands this.
- **Apply All gains a "Keep going as the rest settle" checkbox.** It reads
  "Apply as they settle" when nothing is pending. That is the sweep: everything
  pending, plus everything still working, as each one lands.
- **A single change gains a standing apply.** That is the selection, and it is
  not redundant beside the sweep. The owner may have two threads running, one
  wanted in this release and one not, and a sweep cannot tell them apart.
- **The change actions fold into one control**, reachable from the thread's own
  prompt row. That row lifts one button today, and `getStandaloneCcDiffButton`
  is the only thing it can hold. So a working coding-agent thread offers no way
  to arm an apply at all.
- **No change action renders disabled, on either surface.** The Changes panel
  draws Apply and Discard at 40% opacity for an unsettled thread, and the prompt
  row draws neither. Both are wrong in the same way: a control that cannot act
  is replaced by the one that can, which is the standing apply.
  `.action-btn:disabled` also sets `pointer-events: none`, so the tooltip
  explaining the block can never be read.
- **Both are the standing owner instruction** those 26 orchestrator threads
  stood in for. A thread that parks or fails drops its standing apply and
  reports it, so nothing waits forever. Both are invokable from the prompt, per
  philosophy rule 2.
- **The glossary gains a *workspace owner* entry**, and the *Top-thread* entry
  gains the sibling clause.

## Alternatives considered

- **Agents stay in their lane, with no owner exception.** Rejected on the
  measurement. It blocks 101 applies, about 2% of the total, and that 2% is the
  whole release workflow.
- **Adoption, where the owner re-parents existing top-threads under an
  orchestrator.** Rejected as unnecessary once the orchestrator is understood as
  a relay. It also makes `parent_thread_id` mutable. `api::thread_reach` leans
  on that column being stamped once at spawn, which is why its upward walk needs
  no depth cap and cannot cycle.
- **Acquired standing, where whoever speaks first becomes parent-ish.**
  Rejected. A parent's standing is recorded in the child's row, checkable and
  unclaimable twice. Standing that exists only because one thread spoke first is
  the same authority with no record.
- **Spawner standing, where a thread may always act on what it spawned.**
  Rejected as a second kind of standing beside parenthood, for a case clause 5
  already covers. The spawning link is recorded, so it was not the acquired kind
  and deserved weighing. It fails on scope: it says nothing about a thread
  acting on work it did not create, which is most of the measured cases.
- **Gate by blast radius rather than by actor.** Rejected. It explains why
  archive was gated first, and it makes every new verb a fresh judgment about
  how bad it is. A rule re-argued per verb is how four routes ended up ungated.
- **A confirmation card for every wider act.** Rejected, though the cost is
  small at roughly 23 events a month. Cost is not the reason. The owner said it
  in that thread seconds earlier, and the log already records the turn. The card
  would ask a question it already holds the answer to.
- **The person as the root instead of the workspace.** Rejected on the
  cross-workspace case above.
- **A root with a row.** Rejected: it is a fleet controller, and it hands voice
  the seat ADR 0148 denied it.
- **A loop counter or an admission bound on thread-to-thread exchange.**
  Rejected as unnecessary under clause 5. It was also the wrong layer: a bound
  is state carried across an exchange, while `authorize_thread_reach` is a
  stateless answer derived from graph shape.
- **Sibling-to-sibling messaging.** Rejected twice already, in ADR 0043 on
  topology and ADR 0083 on authority. Nothing here reopens it. A sibling still
  cannot deliver into a sibling's inbox, and clause 4 permits pressing the
  owner's button, never addressing a peer.
