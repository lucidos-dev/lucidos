---
name: Coding Agent Flows
description: Wiring handoff checkpoints into a repository's coding-agent flows, and the default plan and hardening flows.
---

# Coding Agent Flows

A **flow** is a procedure a coding-agent thread follows: a plan skill, a review
command, a section of `CLAUDE.md` or `AGENTS.md`. The repository owns its flows.
Lucidos owns two primitives they call: the *handoff checkpoint* and the
*handoff estimate*. ADR 0505 records why.

A checkpoint is a named point in a flow where the work may continue in a fresh
*successor session*, in the same thread. The engine knows no checkpoint by name. A flow names one by
running a command at that point, and nothing else turns it on.

## Wiring a checkpoint

Put one line at the point in the flow, as an instruction to the agent:

```bash
lucidos successor offer --at <name> [--progress <done>/<total>] [--ask]
```

The agent runs it there and follows what it prints.

- **`--at <name>`**: lowercase letters, digits and hyphens, up to 64. Give each
  step its own name, and keep it stable.
- **`--progress <done>/<total>`**: the flow's units of work, when it has some.
- **`--ask`**: the checkpoint offers a question card. Leave it out for an
  automatic checkpoint.

**The name is the history.** The estimate reads past runs of the same name, in
the same repository, on the same model and effort. A renamed checkpoint starts
again from the default, so rename only when the step really changed.

## The two kinds

| Kind | Use it when | What it prints |
|---|---|---|
| Automatic | The flow asks the user nothing at this point. | The handoff steps, or "carry on". |
| Offered (`--ask`) | The flow already asks the user a question here. | The shape of that question's card. |

An **automatic** checkpoint prints one of two things:

- **Hand off**: the estimate pays. The agent writes a *handoff narrative*, runs
  `lucidos successor start` with the same `--at` and `--progress`, and ends its
  turn. A *successor session* then carries on in the same thread.
- **Carry on**: the estimate does not pay. The agent asks nothing.

A handoff never asks the owner for approval.

An **offered** checkpoint folds a fresh-session option into the flow's own
question. The flow names its option to stay, such as `Approve here`. It also
says what a pick does before the handoff, such as approving the plan.

## What a unit is

A unit is a piece of work the flow repeats, such as a plan's phase. The estimate
multiplies the units left by the calls one unit took at past runs. Units left
are `total - done`, else one.

- Count units the flow already counts. Never invent them for the estimate.
- A flow with no repeated work leaves `--progress` out.
- Pass the units left at this point, not at the start. A plan of 5 phases at
  approval is `0/5`, and after its second phase it is `2/5`.

## Keeping a flow running after a handoff

The successor session reads only the narrative and the files. So the narrative quotes
the flow's remaining checkpoint instructions, and the successor runs them in
turn. Both outputs above say so. It starts at the step after the checkpoint
that started it, and never runs that checkpoint again. A flow file the successor will load anyway
needs no quote, only a pointer to the step.

## Taking a default flow

Lucidos ships two default flows, each as a reference file:

- `system-knowhow/coding-agent-flows/implementation-plan`: a plan flow, with an
  offered checkpoint at approval and an automatic one between phases.
- `system-knowhow/coding-agent-flows/hardening`: a review flow, with an
  automatic checkpoint at its first entry.

Copy a default only when the user asks for it. Read it with
`lucidos knowhow read <id>` and copy its marked block verbatim, markers
included. Write the repository's own steps outside the block. Then a later
default can replace the block whole, and a check script can compare the copy.
