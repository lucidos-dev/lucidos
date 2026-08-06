# 0043: The parent-to-child edge is the only privileged cross-thread write, and it asserts nothing

- **Status**: Accepted
- **Date**: 2026-08-05

## Context

Child to parent already worked end to end (ADR 0011): every child terminal fires
a typed `ChildThreadCompleted` on the parent plus a wake that resumes the
parent's turn. The other direction did not exist. A parent could spawn N
children and read their results, but it could not address a child it had already
spawned, so it could not redirect one going the wrong way, could not feed one
child something a sibling had learned, and could not tell a stalled one to
continue. Its only lever was to spawn yet another child, against a cap of ten.

Adding the reverse edge is where an orchestration design usually acquires an
any-to-any address space, and that is the thing worth refusing up front. The
questions this ADR settles are: who may write to whom, how the caller is
identified, and what the write is allowed to assert about itself.

## Decision

Exactly one new privileged edge: **a parent to its own direct children.**

- **Topology.** A thread can address the threads whose `parent_thread_id` is the
  thread itself, and nothing else. No sibling edge. No grandchild edge (a
  grandparent goes through the child). No cross-workspace edge.
- **Authorization is read, never asserted.** The caller passes the child's id
  and a message. The engine loads the child's `thread_summaries` row and refuses
  unless `row.parent_thread_id == caller`. The relationship is never a parameter.
- **The caller identity is ambient.** On the shipping surface (the in-process
  `follow_up_child_thread` LLM tool) the caller is `execute_tool`'s `thread_id`,
  which the model cannot set. The tool schema exposes no caller argument.
- **Everything derivable is derived.** Coding-agent-ness comes from the child's
  own `source`; the mode is fixed; there is no repo, model, or continuity field
  to get wrong.
- **The write asserts no spawn linkage.** The emitted `MessageReceived` carries
  `parent_thread_id: None` and `spawning_event_id: None`, so the projection
  routes it down the revive branch. A follow-up therefore never creates a
  thread and never changes `total_children_count`.
- **It returns an ack, not the child's turn.** The child's `MessageReceived` is
  persisted before the ack returns; the child's turn is spawned and not awaited.

Named `child follow-up` at every layer: the glossary entry, the
`follow_up_child_thread` tool, `engine/chat/child_follow_up.rs`,
`ChildFollowUpError`, `FollowUpAck` / `FollowUpDelivery`.

## Rationale

**Why a dedicated verb rather than a field on `POST /api/v1/chat/stream`.** The
tempting shape is a flag on the existing chat POST. The argument that decides it
is `validate_mode_and_spawn`: that function enforces "Agent or Engine mode
requires `parent_thread_id` OR `caller_workspace`" and treats the two as
mutually exclusive descriptions of provenance. A follow-up has neither. Putting
it there means adding a third provenance kind to that matrix **and** a
derive-it branch for `use_coding_agent`, which is today a caller-asserted,
continuity-locked field. That is a larger widening of a heavily loaded contract
than a new verb is. Recorded so it is not rediscovered: `chat_submit` is also
where roughly 730 lines of accumulated correctness for "deliver a message to an
existing thread" live, so the dedicated path adopts its asynchrony shape
deliberately rather than reinventing it.

**Why the ladder is a real boundary in-process and only an accounting boundary
over HTTP.** `api/actor.rs` documents `X-Lucidos-Source-Thread-Id` as display
attribution: any subprocess can claim any source thread id. So over HTTP,
"authorization read from the projection" would constrain only *which* thread a
caller must claim to be in order to reach a given target. In-process the caller
is ambient and unforgeable, and the same ladder is a genuine authorization
boundary. Those are different guarantees from the same code, so the HTTP route
and the CLI are **deferred** behind a stated precondition: the source thread id
must be bound to the subprocess origin token at spawn time rather than accepted
from a header. Deferring them removes the entire spoofable surface from this
change.

**Why an ack rather than the child's result.** Three of the six delivery modes
do not return promptly (a chat child's whole agentic loop, a coding-agent
`--resume` session, a Codex live turn that first blocks up to 30 s on the
interrupt). The follow-up runs inside the *parent's own agentic loop*, so
awaiting would park the parent for the child's entire run, and while parked the
child can complete and inject its wake into the parent's still-running turn,
ahead of the tool result the parent is waiting for. The mirror,
`notify_parent_of_child_completion`, gets away with awaiting because it runs on
the ParentCallback listener task. That asymmetry is deliberate and is documented
at both sites.

**Why the emit is nonetheless awaited.** The Thread Queue's `prepare` hook
already states the invariant this transposes: a child's `active_children_count`
must increment before the parent can finish its turn, or `ResponseGenerated`
wins the race and the parent flips to review while the child is still working.
So the follow-up persists the child's `MessageReceived` inline and awaits it
exactly when a revive re-increment is owed, which is exactly when the child is
outside the in-flight set. When the child IS in flight there is a live lane that
already owns the emit and sequences it against a Codex interrupt boundary, so
pre-empting it would reorder the child's timeline.

**Why the LLM tool is standalone rather than a `threads` action.**
`llm/tools/tests.rs` already asserts a hot-single-purpose guardrail for
`run_thread` and `run_coding_agent`, and the follow-up is the third member of
that family. The `threads` domain is introspection: its summary opens
"Introspect threads" and its other actions are reads, so a model that just
called `run_thread` would not look there for a write verb. ADR 0018 parity is
per-operation rather than per-tool, so the split costs nothing.

**Why a reclaimed child worktree is accepted.** No third clause is added to
`has_pending_fan_in`, which protects a thread whose own children are
outstanding, not a child on the grounds its parent may redirect it. Three
existing mitigations carry it: the retention gate keeps a non-archived child's
worktree warm while free disk is above the soft threshold, both full-removal
tiers skip a dirty tree or a pending change, and the spawn path recreates a
missing worktree from the branch before resuming. The residual cost is a cold
rebuild of `target/` and `node_modules/`. One case fails outright and is
accepted: an **archived** coding-agent child whose branch had no commits ahead
of main is Tier-0'd with the branch deleted, and a later follow-up returns the
existing actionable error. Nothing was lost, because the branch carried nothing.
ADR 0035's "exactly one owner" holds unchanged: this adds no removal path and no
second reclaimer.

## Consequences

- The parent gains a write it can only aim at its own children, and the failure
  mode this design exists to prevent (a silent mis-delivery to the wrong thread)
  is unreachable by construction rather than by validation: the tool takes a
  uuid and never a title, and the caller cannot state who it is.
- A child can now report more than once. `ChildThreadCompleted` is a log of
  completed turns, not one card per child, and `child_thread_id` is not a key.
  Documented in `system-knowhow/thread-events.md`.
- A follow-up consumes no child slot, because the recursion guard counts rows
  rather than messages. That is intended: reviving a child is cheaper than
  spawning an eleventh, and the system prompt says so.
- A redirect resolves the child's pending permission cards as superseded. That
  is a user-visible side effect of an agent action, so it is stated in the tool
  description and pinned by a test.
- **A Codex redirect dips the parent's `active_children_count` to 0 for the
  length of the interrupt, and that is accepted.** `SupersededByFollowup` sends
  no card, but its projection arm is cause-agnostic: it settles the child to
  idle and reconciles the parent from ground truth, so between the interrupt
  landing and the redirected turn's `MessageReceived` the parent reads as having
  no children in flight. The lane waits for a turn boundary first, so on the
  Codex path that window can reach `REDIRECT_INTERRUPT_MAX_WAIT`. A parent that
  ends its own turn inside it shows idle rather than "waiting for children"
  until the redirect starts. Nothing is lost: the message is not dropped, the
  card is not skipped, the counter is not permanently wrong (the redirected
  turn's start re-increments and every terminal reconciles from ground truth
  rather than by delta), and the parent is still woken by the redirected turn's
  own completion. Closing the window would mean not settling the child to idle
  on this cause, which is a change to a contract-tested lifecycle transition and
  a wider blast radius than the transient earns. Pinned by
  `a_codex_redirect_dips_the_parent_count_then_restores_it`. Note this window is
  not new: it is the shape `arm_followup_redirect` (`arm_codex_redirect` when
  this was written) has always had for a human
  follow-up into a live Codex child. What is new is how often a parent reaches
  it.
- A follow-up to an archived coding-agent child re-surfaces it to the user's
  Inbox at its next idle. Consistent with the existing rules, but it means an
  agent can pull an archived thread back into view.
- The HTTP route, the CLI, and the manifest operation do not ship. Until the
  precondition above holds, the only surface is the in-process tool, and the
  coverage that would have been API e2e is engine integration coverage instead.

## Amendment, 2026-08-05: the precondition is met, and the ladder is now an authorization boundary everywhere

The deferral above was gated on one thing: the caller thread being
authenticated rather than header-asserted. It is. The subprocess origin token is
now **thread-bound** (`docs/glossary.md` § thread-bound origin token): the engine
mints one per spawn, shaped `"<thread-id>.<mac>"`, and the token's own prefix is
the source thread. The separate `x-lucidos-source-thread-id` header, which any
subprocess could set to any value, is gone.

Three consequences for this ADR:

- **The distinction it drew between in-process and over-HTTP collapses, in the
  good direction.** The rationale said the refusal ladder was a real
  authorization boundary in-process and only an accounting boundary over HTTP,
  because `caller_thread_id` would come from a spoofable header. It no longer
  can. `POST /api/v1/threads/:thread_id/follow-up` and
  `lucidos threads follow-up` therefore ship with the same guarantee the
  in-process tool always had: a thread can reach its own direct children and
  nothing else.
- **The last bypass around the ladder is closed.**
  `subprocess_chat_legitimate`'s `parent_matches_source` arm allowed a
  subprocess to POST into any *existing* thread by naming it as the target and
  naming itself as the parent. Hazard 10(b) of the plan scoped that out as a
  separate change; with the ladder now shipping over HTTP it stopped being
  separate, because a caller refused by `NotYourChild` could have posted the
  same message through `/chat/stream`. The arm now also requires that the target
  does not exist. `lucidos spawn-thread` is unaffected: it pre-generates a fresh
  client-side uuid.
- **No manifest operation ships with the route.** The reason is a property of
  the CLI generator rather than a judgment about parity; recorded as an
  amendment on ADR 0018.

One piece of coverage is named rather than claimed: authorizing a follow-up
needs a thread-bound token, which only a spawned subprocess holds, so the API
e2e suite covers the refusals (403 with no token, 400 malformed, 400 empty, and
that a refusal creates nothing) while the authorized delivery path stays covered
in the engine tests. A test-only endpoint minting a token for an arbitrary
thread was rejected: its gate would be `debug_assertions`, which is the build
`web-dev.sh` runs, so it would put a token minter on every developer's live
workspace.

## Alternatives considered

- **A `follow_up: true` field on the chat POST.** Rejected on
  `validate_mode_and_spawn`, above. Two weaker arguments were offered first and
  are recorded as *not* load-bearing, so they are not re-run: "a path segment
  naming a thread gets existence checking structurally" (it does not, axum does
  not check existence and the 404 comes from a handler query the chat POST would
  run identically), and "you would have to infer follow-up from `thread_exists`"
  (a straw man once the field is explicit).
- **Sibling-to-sibling messaging.** Rejected, and not a slippery slope by
  construction: the check is a single equality against the caller, so a sibling
  edge would need a new predicate rather than a relaxed one.
- **Grandchild addressing.** Rejected. A parent addresses its direct children
  (`parent_thread_id`), never the recursive ancestor CTE. A grandparent that
  wants to reach a grandchild goes through the child, which is what a star
  topology at each level means.
- **A shared pull surface children can read.** Rejected. A surface readable by
  all children is a shared mutable channel between siblings whatever the wire
  shape, which is the address space this design refuses. With the
  parent-to-child edge closed the parent holds both halves it needs: it already
  receives every child's result and now has the write path to hand a sibling's
  finding to a child. If a child-side pull ever proves necessary, the honest
  version is a parent-scoped read-only surface, and it should be its own design
  with its own ADR.
- **Arguments on `run_thread` instead of a new tool.** Rejected. `run_thread`'s
  name and entire description are "Start a new Lucidos thread"; a `thread_id`
  argument that flips it to "do not start one, message an existing one" makes
  the name actively misleading, which the glossary rule treats as a rename
  obligation. It would also have to be duplicated onto `run_coding_agent`, and
  the model would have to remember which tool matches which child when the
  engine already knows from the child's row.
- **Cross-workspace follow-up.** Rejected, and refused explicitly rather than
  silently reinterpreted. Cross-workspace spawns require `relation = "top"`, so
  they land with `parent_thread_id = NULL` in the receiving workspace and no
  cross-workspace caller has a child to follow up on.
- **Awaiting the child's turn and returning its result.** Rejected: see the
  rationale. It parks the parent's agentic loop for the child's whole run and
  lets the child's own wake overtake the tool result.
- **A `"self"` sentinel on the listing's `parent` filter.** Rejected. One wire
  value would have had three behaviours (resolved at the LLM edge, resolved at
  the CLI edge, a 400 at HTTP), which is a magic string with a per-surface rule.
  `parent` stays a literal uuid everywhere and the LLM tool takes a separate
  boolean-shaped `my_children` that the handler resolves from its ambient thread
  id.
- **Widening `has_pending_fan_in` to protect a child whose parent might redirect
  it.** Rejected: see the worktree rationale. It would add a second retention
  clause for a case three existing mitigations already cover.
