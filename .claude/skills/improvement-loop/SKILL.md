---
name: improvement-loop
description: Read the scoreboard and write a dated report. Within the allowance, propose delivery findings or fall back to one kept-list batch. Use for the improvement loop or to find the next delivery improvement from the numbers.
---

# Improvement loop

One run of the *improvement loop*: read the scoreboard, compare it with the
last run, write a report, and propose within the allowance. The loop's trigger
starts the run on a schedule. A person may start it too.

## What a run never does

- It never edits source, commits, or applies a change.
- It never builds a proposal. A proposal is a planning thread that writes an
  implementation plan. The plan gate asks the owner for approval there.
- It never proposes more than its allowance.

## Inputs

The prompt gives the **allowance**: the number of proposals this run may
start. The trigger computed it from its limits and the earlier proposals.
With no allowance in the prompt, use 0. An allowance of 0 still writes the
report.

This thread runs in a worktree and cannot see the workspace's `data/` by
path. Use `lucidos data path artifacts/improvement-loop` to find the report
folder.

## Steps

1. **Read the scoreboard.** Run `./scripts/scoreboard.sh --json` and
   `./scripts/scoreboard.sh` (default 7 days). The number of changes behind
   each cell is part of the evidence.
2. **Run the lab, when it exists.** If a lab script is in `scripts/`, run it
   in the background class and in an idle window. With no lab script, skip
   this step and say so in the report.
3. **Compare with the last run.** Read the latest earlier report in the
   report folder. Name each measure that moved, by how much, and over how
   many changes. A first run has no earlier report: say so.
4. **Write the report.** Save the text and the JSON as
   `artifacts/improvement-loop/<YYYY-MM-DD-HHMM>.md` and `.json` with
   `lucidos data write`. Never overwrite an earlier file: the name holds the
   time.
5. **Decide what to propose.** Apply the delivery evidence rules below.
   Use the kept-list fallback only if no delivery finding qualifies and
   the allowance is positive.
6. **Propose.** Start one planning thread per selected proposal, up to the allowance.
   For a delivery finding, use `lucidos spawn-thread --to <this workspace> --coding-agent
   claude-code --relation top --title "Improvement proposal: <finding>"
   --message "<the plan request>"`. The trigger counts proposals by the title
   prefix, so keep it exact. The message names the measure, its value, its
   count, the earlier value, and the change you suggest. It asks for an
   implementation plan and nothing else.
   For a kept-list batch, use the same command with the title
   `Improvement proposal: Kept-list batch for <group>`.
   Ask only for an implementation plan. Name the group, its eligible row count,
   and the [kept-list method](../../../docs/plans/2026-11-23-kept-list-enforce.md).
7. **End the turn.** Say what you wrote and what you proposed.

## Delivery evidence

A delivery finding needs all of these:

- A measure with at least 5 changes behind it. A "too few" cell justifies
  nothing.
- A number that stayed bad or got worse since the latest earlier report, and
  the counts behind both numbers.
- A cause you can name from the report: a stage, a wait, a unit cost or a
  class.
- A finding no earlier proposal covers. List the earlier proposals with
  `lucidos threads list --limit 1000` and read the titles that start with
  `Improvement proposal:`. A repeated finding adds no proposal.

Numbers from this host describe this host. Never compare hosts.

## Kept-list fallback

- Read remaining `keep` rows in [the kept list](../../../scripts/comment-strip/kept-list.tsv)
  in list order. Group related files as in the kept-list method.
- Skip crates that still have moves pending in the
  [engine crate split](../../../docs/plans/2026-11-19-engine-crate-stack-split.md).
  Compare each open unit's branch against its merge base with `main`.
  Exclude any file those diffs touch.
- Take the first group with eligible rows and no open proposal for that group.
  Count only eligible rows. If no group qualifies, start no proposal.

## Checklist before step 6

- Every delivery proposal cites its numbers and counts and passes the duplicate check.
- A kept-list proposal is the only proposal in its run. Its title names the
  group and its request gives the eligible row count and the method.
- The batch excludes files touched by open unit branches and crates due to move.
- Every title starts with `Improvement proposal:`.
- The count of proposals is at most the allowance.
- The report file is new and the earlier files are untouched.
