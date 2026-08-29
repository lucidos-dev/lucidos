# 0154: An unattended security run may commit a bounded fix; wider work reports blocked on a decision

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

Two rules collided head-on, and the collision cost real security fixes.

The nightly orchestrator's first cross-cutting rule says a sub-session must
never pause for input and must decide on its own. A child that stops to ask
anything is a failed step, and a failed step stops the pipeline. The plan gate
says complex work needs a human plan decision before the first source edit, and
a security change is complex by name.

The nightly's Step 3(c) security scan is scoped to the gateway, the engine chat
and proxy path, `signers/`, the auth pipeline and SDK escaping. That is
cross-layer by definition, so it trips the gate every night. The child cannot
ask, because it is unattended. It stops.

On 2026-08-07 the step produced a plan document and nothing else. It emitted
`SecurityScanFailed`, and the pipeline never reached harden or e2e. On
2026-08-25 the gate blocked a confirmed critical. Both findings filed on
2026-08-29 end their notes with the same line: not committed, because the run is
unattended and the gate blocks a source edit for a security change.

Two bypasses existed and both sessions correctly refused them. `lucidos planned
approve` records an approval the maintainer never gave. `lucidos planned mark
--simple` declares a cross-module credential fix a local one. That refusal is
right behaviour, and loosening agent honesty is not a fix.

One premise needs correcting, because it is what a reader assumes. "Nothing
merges to main without an Apply" is true of an ordinary coding-agent thread. It
is false of the nightly, which applies its own children's changes unattended by
the maintainer's standing decision of 2026-08-12.

## Decision

A fourth plan-marker state, `bounded_security_fix`, recorded by
`lucidos planned mark --security-fix "<reason>" --files <csv>`. It satisfies the
gate with no approval step. It names at most ten files, and the Apply floor
refuses the branch if it went outside that list.

Anything wider stays gated. The session commits its plan, leaves the marker
`proposed`, and ends its reply with a literal `BLOCKED ON PLAN DECISION:` line.
The orchestrator reads that as a step outcome, reports it as a decision the
maintainer owes, and carries on with the pipeline.

## Rationale

**The lane is a fourth state because the other three would be lies.** That is
the whole design. `acknowledged_simple` claims the work is local. `planned`
claims a human approved. The new value claims exactly what happened: an
unattended run bounded itself and is asking the human to decide at review. The
honest option is now also the working one.

**The bound is enforced, not promised.** The nightly applies its own children's
changes, so no human stands between this lane and main. The Apply-time check is
the only automated thing that does.

It asks what will LAND, not what is committed. The branch's committed diff is
re-derived from git rather than read off the recorded projection, which a later
commit can outrun. The worktree's uncommitted files are added to it, because
every apply path stages the tree before merging, so a dirty file lands too.

It runs where an apply BEGINS, in both `apply_change` and the live-session
`apply_now`, and it fails closed: a git or database call that cannot answer
refuses, and refusing costs a click.

**It deliberately does not run at the tier fast paths**, which was tried and
reverted. In this codebase an `Err` out of the merge helpers means "main
diverged, escalate to a slower route", not "refuse". A bound enforced there was
therefore read as a conflict. It sent the branch into the conflict-resolution
tiers, one of which finalises through a path the wrapper never covered. It also
promised the user "the change will apply automatically". A refusal has to be a
refusal, and the entry points are where that is expressible.

**The lane's residual risk is bounded by what the night already does.** Every
apply is a git merge that `git revert` undoes. Harden and the full e2e suite run
on top of the merge the same night, and the merged change is named in the
morning notification. A bad bounded fix goes red at 03:00, not in a release
candidate.

**The blocked lane needs no new marker state.** `proposed` already means
"awaiting the human's decision". What was missing is not a marker but a *step
outcome*: the orchestrator had no way to tell a scan that worked and asked for a
decision from a scan that fell over. The literal reply line supplies it. It
rides the mechanism the orchestrator already uses to judge a step, which is
reading the child's final response.

## Consequences

- A security finding that fits the bound gets fixed the night it is found.
- A run that ends with a plan and the blocked line is a complete run. It no
  longer turns the pipeline red and no longer stops harden and e2e.
- The gate is unchanged for every other kind of work. `proposed` still blocks
  the hook and Apply, the question tool is still how approval is asked, and the
  other three states behave exactly as before.
- A reviewer can tell the three satisfying states apart, because the lane
  reports its own kind on the `planned-state` wire.
- The write path parses the state strictly. A misspelling such as the
  kebab-case `bounded-security-fix` is refused rather than recorded as
  `planned`, which would have claimed a human approved and carried no bound.
  Reads stay lenient, so a drifted row still satisfies the gate.
- **The conflict-resolution merge is checked separately**, because it is the
  one route the entry-point check cannot cover: that session edits after the
  check and has its work auto-committed. It gets its own recheck in
  `completion.rs`, which is safe there for the reason the tiers were not. A
  failure on that path is terminal, emitting `ChangeApplyFailed`, rather than a
  cue to escalate. The marker and the merge source are read as two branch
  names there, since a Tier-3 session publishes a temp branch.
- Three things are declared rather than proved, and are named as such in the
  plan: that the run is genuinely unattended, that the fix is security work, and
  that a regression test exists. The deny text and the CLI output condition all
  three loudly. The file bound is what keeps the blast radius small when a
  declaration is wrong.

## Alternatives considered

**A pre-approved standing plan for the recurring sweep.** Tried in spirit and
known not to work: writing a plan document is not what satisfies the gate,
approval is. A fix has to move the marker state, not add a file.

**Let the child run `lucidos planned approve` itself.** It records an approval
the maintainer never gave, which corrodes the one signal the marker carries. Two
sessions refused it unprompted, and they were right.

**Let the child use `--simple`.** Same objection in a different place: it
declares a cross-module credential fix local. It would also be invisible, since
nothing would distinguish it from an ordinary small change at review.

**Make the security pass read-only, with no source-edit authority.** This is
what happened in practice from 2026-08-19, arrived at by the child rather than
by design. It removes the failed step but it never fixes anything, and it leaves
one unappliable plan-only change pending per night. Rejected because a scan that
can never remediate is worth less than one that can fix what fits.

**Enforce the bound in the `cc-plan-gate` hook as well.** The hook would have to
re-derive repo-relative paths from tool input, and a false denial there is the
same deadlock in a new place. The Apply floor gets the authoritative list from
git for free, and it is the only enforcement Codex has anyway.

**Check mechanically that a regression test exists.** A Rust fix and its inline
`#[cfg(test)]` test are one file, so every path heuristic false-refuses the
commonest shape. A false refusal in an unattended run recreates the deadlock.
The test stays a stated precondition, verified by the night's own harden and e2e
steps.

**Widen the gate for all unattended work, not just security.** It would fix the
same symptom and give up the gate's actual purpose. The nightly's other steps
already have their own commit postures, and none of them was deadlocked.
