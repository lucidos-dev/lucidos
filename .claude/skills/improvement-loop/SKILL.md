---
name: improvement-loop
description: One run of the improvement loop. It reads the scoreboard, writes a dated report, and starts up to the allowed number of planning threads. Use when a prompt names the improvement loop, or when asked to find the next delivery improvement from the numbers.
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
5. **Decide what to propose.** Apply the evidence rules below. Propose
   nothing when no finding passes them.
6. **Propose.** For each finding, up to the allowance, start one planning
   thread with `lucidos spawn-thread --to <this workspace> --coding-agent
   claude-code --relation top --title "Improvement proposal: <finding>"
   --message "<the plan request>"`. The trigger counts proposals by the title
   prefix, so keep it exact. The message names the measure, its value, its
   count, the earlier value, and the change you suggest. It asks for an
   implementation plan and nothing else.
7. **End the turn.** Say what you wrote and what you proposed.

## What counts as evidence

A proposal needs all of these:

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

## Checklist before step 6

- Every proposal cites its numbers and the count behind each.
- No proposal repeats an earlier one.
- The count of proposals is at most the allowance.
- The report file is new and the earlier files are untouched.
