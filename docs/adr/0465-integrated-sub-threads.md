# 0465: An integrated sub-thread runs the fast tier, proposes nothing and hands its branch up to its parent

- **Status**: Accepted
- **Date**: 2026-10-13
- **Amends**: [0416](0416-unhardened-work-is-never-proposed.md),
  [0315](0315-incremental-hardening.md)

## Context

A sweep spawned 22 sub-threads in one day. Each ran the whole `/harden`, merge
of `main` and engine suite included, and then proposed its own change. The day
spent 47 `/harden` runs and about 52 agent-hours on them, with 14 running at
once at the peak. The sub-threads' own merges of `main` hit 23 conflicts, and
8 of the day's 11 Apply conflicts came from them. The measurements and the
grill are in `docs/plans/2026-10-12-landing-lane.md` (decisions 2, 3 and 13,
Phase 5).

Most of those sub-threads were follow-ups of their parent's own work. They
would have landed better as part of the parent's one change.

## Decision

A coding agent can spawn a child as an **integrated sub-thread**, with
`lucidos spawn-thread --relation child --integrated`. The default stays
independent.

- **Who may.** The child is a new Lucidos-source coding-agent thread, and its
  parent a Lucidos-source coding-agent thread. The CLI and the chat endpoint
  refuse anything else and name the flag to fix.
- **Where the flag lives.** The spawn request carries `integrated` into the
  queued `AgentChat` request, which `ThreadQueued` persists. The projection of
  the child's first `MessageReceived` copies it from that request into
  `thread_summaries.coding_agent_integrated`, which keeps it, monotone. So a
  first run that fails before its `SessionStarted` still reads integrated.
  Every `SessionStarted` also carries the flag.
- **Only through the queue.** A `mode=human` spawn starts at once, outside the
  queue. So the CLI and the endpoint refuse the flag with it, rather than
  start an independent child.
- **The fast tier.** The child's `/harden` reviews as usual, skips Phase 0.3,
  and runs the fast suites: `make lint`, tsc, Vitest, `vite build` and the
  script tests, with no crate test suite. `harden-suites.sh` asks the engine
  for the tier, and no variable overrides its answer. Phase 5 marks
  `--tier fast`, and `hardened_branches.tier` records it.
- **It proposes nothing.** With a fresh marker of either tier, its work waits
  as `integrated_into_parent`. Without one, it waits as `hardening_missing`, as
  any branch does. Its turn end sends no re-entry, and Apply Now refuses it.
  A branch of plan files alone is the exception: it needs no hardening and
  leaves no marker to hand up, so it proposes and applies like any branch.
- **It hands its branch up.** The child's success report carries
  `integrated_branch { branch_name, head_sha }`, and the parent's card tells
  the agent to `git merge` it. The engine never writes the parent's branch.
- **A landing without the merge sets the work aside.** The branch-work net
  hears each `ChangeApplied`. For the landed thread's integrated children, it
  skips any that is live or still owes the parent its report. It sets aside
  the rest's work beyond `main`, with origin reason `unmerged_integrated_work`.
  A merged child's commits are on `main`, so it has nothing to set aside.
- **The parent reviews only what no tier reviewed.** `lucidos hardened
  integrated-heads` lists the heads the children's runs certified.
  `harden-scope.sh` counts every commit under one as reviewed, as it counts
  `main`. The parent still runs every suite its diff selects.

### Amendments

- **ADR 0416.** "Unhardened Lucidos-source work is never proposed" now reads
  "no integrated sub-thread's work is ever proposed, and other work needs a
  *full* marker". A fast marker alone leaves an independent branch held as
  `hardening_missing`, and `apply_must_harden` still hardens it at Apply.
- **ADR 0315.** "Reviewed" means on the base or under a head another `/harden`
  certified, not only on `main`. A merge of a child under its head adds only
  its resolution. The overlap is now per merge, a file both sides changed since
  they parted. That definition fits a child merge as well as a merge of `main`.

## Rationale

- **Review is where the tiers agree.** Both tiers review every line the
  branch wrote. Only the suites differ, and the parent runs all of them once
  over the merged work. So every line is reviewed once and tested once.
- **The marker names what ran.** A tier on the marker lets every reader ask
  the question it needs. Apply asks for full, the child's hand-up for either.
  A fast marker can never stand in for the engine suite.
- **The parent's agent merges.** It knows its own work and picks the moment.
  The engine merging into a working agent's branch is the hazard ADR 0060 and
  ADR 0241 refuse.
- **One flag, one record.** The persisted spawn request is the record, and the
  child's row holds it from its first event. A first run that fails, a restart,
  a resume and a recovery all read it from the same column. An in-memory
  hand-off to the first `SessionStarted` lost it whenever that first run died
  early.

## Consequences

- A sweep's subtree hardens once with the engine suite, at its integrating
  parent, instead of once per child.
- `lucidos hardened query` answers for the tier the branch needs, so a fast
  marker on an independent branch reads `MISSING`. Its three words stay the
  same. `tier` and `expected-tier` read the rest.
- The engine refuses `mark --tier fast` with a 409 on a branch that needs the
  full run, while the agent can still run it.
- A parent with no marker of its own counts its own work from where it forked.
  `harden-scope.sh` says so with a `fork-point` line, and `/harden` reviews the
  listed commits at full strength.
- Commits the child made after its marker are not under its head. Merging them
  answers `FULL`, which costs time and never skips review.
- An archived parent with an unmerged child sets the child's work aside through
  the archive net, as for any archived thread. Once the parent's change lands,
  a merged child's commits are on `main` and its worktree is finished.
- A parent's change can land before its agent merges a child. Without the set
  aside, the child's work would wait as `integrated_into_parent` for good, and
  Continue would only hold it again. The set-aside change clears that reason,
  so the strip stops naming the parent, and Bring back offers the work.
- Set-aside work is applied as the child's own change, so Apply needs a full
  marker. A branch with a pending or set-aside change of its own therefore
  expects the full tier, and its `/harden` runs every suite.
- The set aside can be early. A parent woken by the report, but not yet merged,
  may still merge the branch after the landing. The set-aside row then names
  work its next change carries too.
- The pass runs on a landing only. A restart between the landing and the net,
  or a report that reaches the parent after its last landing, leaves the child
  held. Archiving the child still sets its work aside.
- `CodingAgentThreadSpawned` stays unchanged. It is a transient frame emitted
  only by the LLM `run_coding_agent` path, so nothing durable could rest on it.

## Alternatives considered

- **Put the flag on `CodingAgentThreadSpawned`, as the plan first said.** That
  event is transient, and the CLI spawn path never emits it. A restart would
  forget the flag.
- **A separate `integrated` event on the child.** It would add a second write
  for a fact the persisted spawn request already records.
- **A new `integrated` field on `MessageReceived`.** It would have to thread
  through every message constructor and caller, for a fact only one spawn path
  sets and the queued request already carries.
- **Let the child propose and the parent apply both.** Two changes and two
  landings for one piece of work, and the child's change would need the full
  suite to be safe to apply alone.
- **Count a stale fast marker for the hand-up.** The child's branch would then
  carry commits no review saw. A fresh marker is the only proof the head the
  parent merges was reviewed.
- **Let the engine merge the child's branch into the parent.** Rejected in the
  grill (decision 13): it writes a working agent's branch behind its back.
- **Keep the overlap as "the branch's files that a merge brought".** For a
  child merge that lists every file the child touched, which buries the few
  files both sides really changed.
