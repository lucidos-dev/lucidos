---
name: implementation-plan
description: Create an actionable implementation plan before complex code changes, extracting load-bearing invariants and verification from the user prompt, grill/design threads, ADRs/plans, and code reconnaissance. Use before the first code edit for ADR-backed or design-thread-backed work, cross-layer changes, routing/topology/storage/security/migration/process changes, or any non-local implementation where decisions must stay top of mind. If decisions are unsettled, use the grill skill first; this skill consumes settled decisions and makes them executable.
---

# Implementation Plan

## Purpose

Use this skill to convert settled design intent into an executable implementation plan before editing code. ADRs are useful inputs, but they are not required: invariants can come from the newest user instruction, a grill/design thread, an issue, existing docs, tests, or current behavior.

The output is **always a checked-in plan file** at `docs/plans/YYYY-MM-DD-<slug>.md` (the repo convention; date = today, slug = a short kebab-case description), written and committed before the first code edit. Also surface a brief summary in the conversation, but the committed file is the source of truth.

**Enforcement (the Planned marker) + human approval.** Lucidos requires a gate-satisfying *Planned marker* on the branch before any source edit and before Apply. Running this skill records a **proposed** marker for you (after committing the plan file, run `lucidos planned mark --plan docs/plans/YYYY-MM-DD-<slug>.md`). A proposed plan does **not** unblock editing: it awaits the human's approval. **Present the plan, then ask for approval with the `AskUserQuestion` tool** (see "Asking for approval" below). Once the user approves, run `lucidos planned approve` to flip the marker to gate-satisfying; only then are source edits and Apply unblocked. If the user requests changes, revise the plan file, re-commit, and ask again the same way (the marker stays proposed until approved). For genuinely local fixes that don't warrant a plan, the agent acknowledges instead with `lucidos planned mark --simple "<one-line reason>"` (no approval needed); that path does not use this skill.

## Workflow

1. **Check whether decisions are settled.**
   If material design choices are still open, use the `grill` skill first. `grill` resolves the decision tree; `implementation-plan` consumes the settled decisions and makes them executable. If a grill/design thread already exists, read it and extract the resolved choices before planning.

2. **Gather only the context needed to plan.**
   Read the newest user instruction first, then linked threads/events, ADRs/plans/docs, and relevant code/tests. Do enough reconnaissance to understand boundaries, existing behavior, and verification hooks. Do not start implementation while gathering the plan.

3. **Extract implementation invariants.**
   An invariant is a property that must remain true for the implementation to be correct. Include compatibility promises, negative constraints, deliberately preserved behavior, security boundaries, routing/topology/storage/process contracts, and explicit non-goals.

   Use this shape for each invariant:

   ```md
   - **Invariant:** <exact property that must hold>
     Source: <user prompt | grill thread | ADR/plan/doc | existing behavior/test>
     Verification: <test, smoke check, inspection, or manual check>
     Failure signal: <what would prove this invariant was violated>
   ```

   Every load-bearing invariant needs a verification strategy. If verification is not practical in this session, mark it `manual` and explain why. If a load-bearing invariant is unclear, ask the user before editing.

4. **Plan the phases.**
   Split work into phases that preserve the invariants as early as possible. Name the files or modules likely to change, but avoid speculative implementation detail. For each phase, list which invariants it touches and what check will cover it.

5. **Carry the plan through implementation.**
   Before starting each major phase, re-read the invariant checklist. When changing topology, routing, storage, security, migrations, public APIs, or process behavior, explicitly keep the touched invariants in view. If a new invariant emerges, update the plan file before continuing.

6. **Hand off to verification and hardening.**
   Final verification must map back to the invariants. `/harden` is a backstop that checks the planning rule was followed; it is not where invariants should first appear.

## Required Output

Write the plan to a new file `docs/plans/YYYY-MM-DD-<slug>.md` (date = today, slug = a short kebab-case description of the work) using the structure below, then commit it (`docs(plans): <summary>`) before starting the first phase. Immediately after committing, record the **proposed** marker: `lucidos planned mark --plan docs/plans/YYYY-MM-DD-<slug>.md`. Then surface a short summary of the plan in the conversation (the committed file is the source of truth) and ask for approval as described below. Do **not** start editing yet: the edit gate stays closed while the marker is `proposed`.

### Asking for approval

**Ask with your question tool, never in prose** (`AskUserQuestion` on Claude Code, `ask_user_question` on the `lucidos` MCP server on Codex). Prose is what fails: told only to "present the plan and wait", the agent writes a summary, ends the turn, and the thread sits idle until the user types "approve" by hand. In the Lucidos UI the tool renders as clickable buttons, so approval is one tap.

Use exactly one question, with these two options:

| Option | Meaning |
|---|---|
| `Approve` | Start implementing the plan as written. |
| `Request changes` | The user wants something reworked before implementation. |

Claude Code's tool requires **2-4 options**, so a lone `Approve` button cannot be expressed, and it is never needed: it adds an **"Other"** free-text escape to every question, so the user can also type a nuanced answer. (Codex's tool allows omitting options for free text; the same two labels are still the right shape.)

This is a **DECISION** question, not a post-work confirmation, so the "never ask about work you've already done" rule in the system prompt does not apply to it. The distinction is concrete: source edits are *blocked* until it is answered, and a plan is a proposal about work that has NOT been done, so there is nothing to hand off yet.

**Prose is not a lighter-touch version of the tool, it is silence.** The tool call is the only thing that parks the thread in `WaitingForUserAnswer`, and that state is the sole input to `thread_lifecycle::is_attention_needing`: it lights the needs-attention badge, keeps the thread in the Current section, bubbles up the ancestor chain, and is a fire condition for the "When agent needs me" trigger that notifies the user. A question typed into your final message ends the turn instead, so the thread is indistinguishable from a finished one and nobody is told you are waiting.

When the user approves, run `lucidos planned approve`. This flips the marker to gate-satisfying and unblocks source edits and Apply. If the user requests changes, revise the plan file, re-commit, and ask again the same way (the marker stays `proposed` until approved). When an invariant changes mid-implementation, update the file (per Workflow step 5).

```md
## Implementation Plan

### Context
<brief summary of what is being implemented and the inputs used>

### Settled Decisions
<decisions from the prompt, grill/design thread, ADRs/plans, or docs>

### Non-goals / Deferred Work
<explicit boundaries so implementation does not drift>

### Implementation Invariants
<checklist using the invariant/source/verification/failure-signal shape>

### Phases
<ordered implementation phases; include touched invariants per phase>

### Verification
<tests, smoke checks, inspections, or manual checks mapped to invariants>

### Open Questions
<None, or load-bearing questions that block edits>
```

If `Open Questions` contains a load-bearing unresolved decision, stop and ask the user instead of editing.
