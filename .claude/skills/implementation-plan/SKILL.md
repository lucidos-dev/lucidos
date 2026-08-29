---
name: implementation-plan
description: Create an actionable implementation plan before complex code changes, extracting load-bearing invariants and verification from the user prompt, grill/design threads, ADRs/plans, and code reconnaissance. Use before the first code edit for ADR-backed or design-thread-backed work, cross-layer changes, routing/topology/storage/security/migration/process changes, or any non-local implementation where decisions must stay top of mind. If decisions are unsettled, use the grill skill first; this skill consumes settled decisions and makes them executable.
---

# Implementation Plan

## Purpose

Use this skill to convert settled design intent into an executable implementation plan before editing code. ADRs are useful inputs, but they are not required: invariants can come from the newest user instruction, a grill/design thread, an issue, existing docs, tests, or current behavior.

The output is **always a checked-in plan file** at `docs/plans/YYYY-MM-DD-<slug>.md` (the repo convention; date = today, slug = a short kebab-case description), written and committed before the first code edit. Also surface a brief summary in the conversation, but the committed file is the source of truth.

**Enforcement (the Planned marker) + human approval.** Lucidos requires a gate-satisfying *Planned marker* on the branch before any source edit and before Apply. Running this skill records a **proposed** marker for you (after committing the plan file, run `lucidos planned mark --plan docs/plans/YYYY-MM-DD-<slug>.md`). A proposed plan does **not** unblock editing: it awaits the human's approval. **Present the plan, then ask for approval with the `AskUserQuestion` tool** (see "Asking for approval" below). Once the user approves, run `lucidos planned approve` to flip the marker to gate-satisfying; only then are source edits and Apply unblocked. If the user requests changes, revise the plan file, re-commit, and ask again the same way (the marker stays proposed until approved). For genuinely local fixes that don't warrant a plan, the agent acknowledges instead with `lucidos planned mark --simple "<one-line reason>"` (no approval needed); that path does not use this skill.

## When nobody can be asked: the unattended run

This rule and the nightly orchestrator's own rule used to deadlock, and the
deadlock cost real security fixes. The nightly's cross-cutting rule is that a
sub-session never pauses for input and decides on its own. This gate wants a
human decision before a security change is edited. A security scan is
cross-layer by definition, so it tripped the gate every night. The child
correctly refused both available bypasses: `lucidos planned approve` records an
approval nobody gave, and `--simple` calls a cross-module credential fix local.
So it wrote a plan and stopped.

Two lanes resolve it. Both are for an **unattended** run only. If you can ask
the user, ask: everything above still applies to you unchanged.

**The bounded lane, for a fix that fits.** Take it for a security fix confined
to a small, named set of files, shipping a regression test that fails without
it. Such a fix may be committed with no prior plan decision:

```bash
lucidos planned mark --security-fix "<the finding, and the test that proves the fix>" \
  --files crates/lucidos-engine/src/api/proxy.rs,crates/lucidos-engine/src/api/proxy_tests.rs
```

That records `bounded_security_fix`, which satisfies the gate at once. It is a
distinct marker state on purpose. It claims neither that the work is local nor
that a human approved. It claims only that an unattended run bounded itself, and
asks the human to decide at review.

**Apply refuses the branch if it touched anything outside that list**, so the
bound is a check rather than a promise. If the fix has to grow, re-run the
command with the full list. The engine caps the list; a list that long is not a
bounded fix.

The lane is for **security work only**. It is not a general way past the gate,
and using it for anything else is the failure the whole rule exists to prevent.

**The blocked lane, for anything wider.** Do not stop silently and do not file
the finding as unfixable. Write the plan, commit it, record it with
`lucidos planned mark --plan <path>`, leave the marker `proposed`, and end your
final reply with this literal line:

```text
BLOCKED ON PLAN DECISION: <plan path> | <one line naming the decision the human owes>
```

That line is a *step outcome*, not a failure. The orchestrator reads it and
reports the run as blocked on a decision rather than as a failed scan. It then
carries on with the rest of the pipeline. Ending with a plan and that line is a
**complete** unattended run.

The orchestrator's half of this lives in the workspace knowhow
`lucidos-ops/nightly-pipeline`, § "Two cross-cutting rules every step carries"
and § "Step 3 detail". The reasoning and the rejected alternatives are in
[ADR 0154](../../../docs/adr/0154-unattended-security-fixes-get-a-bounded-lane.md).

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

Use exactly one question. `Approve` is always the first option. What goes in the second slot is a **floor, not a fixed shape**:

| Option | Meaning | When |
|---|---|---|
| `Approve` | Start implementing the plan as written. | Always, first. |
| A fork of the plan (`Frontend only`, `Skip the migration`, …) | Approve a named variant. | Whenever the plan has a genuine fork: a narrower scope, one layer instead of two, a different approach. |
| `Request changes` | Something needs reworking before implementation. | **Only** when the plan offers no real fork. Never alongside one. |

Claude Code's tool requires **2-4 options**, so a lone `Approve` button cannot be expressed. That is the *only* reason `Request changes` exists as a default second option: it fills the mandatory slot while carrying a real "don't start yet" decision. A genuine fork fills the same slot better, because it satisfies the minimum **and** tells you what to do.

Carrying `Request changes` as a *third* option beside a fork is the failure this rule prevents. It then means only "I will type what I want changed", which is the escape every card already has, and tapping it sends back the literal label so you have to re-ask what to change. That is precisely the dead-end shape the system prompt's NEVER AUTHOR AN "OTHER" OPTION rule bans, and a live card carried it on 2026-08-04 (`Approve` / `Frontend only` / `Request changes`).

The user can type a nuanced answer straight into the prompt and it arrives as their answer to the question, so no "let me type it" option is ever needed in any form. Lucidos renders exactly the options you pass (CC's tool description promises an automatic "Other" option; Lucidos adds none). (Codex's tool allows omitting options for free text; the same shapes are still right.)

This is a **DECISION** question, not a post-work confirmation, so the "never ask about work you've already done" rule in the system prompt does not apply to it. The distinction is concrete: source edits are *blocked* until it is answered, and a plan is a proposal about work that has NOT been done, so there is nothing to hand off yet.

**Prose is not a lighter-touch version of the tool, it is silence.** The tool call is the only thing that parks the thread in `WaitingForUserAnswer`, and that state is the sole input to `thread_lifecycle::is_attention_needing`: it lights the needs-attention badge, keeps the thread in the Current section, bubbles up the ancestor chain, and is a fire condition for the "When agent needs me" trigger that notifies the user. A question typed into your final message ends the turn instead, so the thread is indistinguishable from a finished one and nobody is told you are waiting.

When the user approves, run `lucidos planned approve`. This flips the marker to gate-satisfying and unblocks source edits and Apply. **A fork answer is an approval too**: revise the plan file to the chosen variant, re-commit, then run `lucidos planned approve`. If the user requests changes instead, revise the plan file, re-commit, and ask again the same way (the marker stays `proposed` until approved). When an invariant changes mid-implementation, update the file (per Workflow step 5).

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
