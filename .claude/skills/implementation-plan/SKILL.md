---
name: implementation-plan
description: Create an actionable implementation plan before complex code changes, from the prompt, grill threads, ADRs and code reconnaissance. Use before the first edit for ADR- or design-thread-backed work, cross-layer changes, routing/topology/storage/security/migration/process changes, or any non-local implementation. If decisions are unsettled, use the grill skill first.
---

# Implementation Plan

## Purpose

Use this skill to convert settled design intent into an executable implementation plan before editing code. ADRs are useful inputs, but they are not required: invariants can come from the newest user instruction, a grill/design thread, an issue, existing docs, tests, or current behavior.

The output is **always a checked-in plan file** at `docs/plans/YYYY-MM-DD-<slug>.md` (the repo convention; date = today, slug = a short kebab-case description). Write and commit it before the first code edit. Put a brief summary on the approval card. The committed file is the source of truth.

**Enforcement (the Planned marker) + human approval.** Lucidos requires a gate-satisfying *Planned marker* on the branch before any source edit and before Apply. Running this skill records a **proposed** marker for you (after committing the plan file, run `lucidos planned mark --plan docs/plans/YYYY-MM-DD-<slug>.md`). A proposed plan does **not** unblock editing: it awaits the human's approval. **Ask for approval with the `AskUserQuestion` tool, and put the plan summary on its card** (see "Asking for approval" below).

Once the user approves, run `lucidos planned approve` to flip the marker to gate-satisfying; only then are source edits and Apply unblocked. If the user requests changes, revise the plan file and re-commit. Then ask again, with a card naming only what changed (the marker stays proposed until approved). For genuinely local fixes that don't warrant a plan, the agent acknowledges instead with `lucidos planned mark --simple "<one-line reason>"` (no approval needed); that path does not use this skill.

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

**The blocked lane is only for a branch with no fix commits.** A proposed
marker makes Apply refuse the whole branch, so it holds every fix on the branch
hostage. If your session has committed a fix, or will commit one, do not write
a plan document and do not run `planned mark --plan`. Instead, list the finding
in your reply under `Decisions:`. Give the thorough fix and the narrow fix for
it. The orchestrator then has the plan written in a separate session, on its
own branch.

The orchestrator's half of this lives in the workspace knowhow
`lucidos-ops/nightly-pipeline`, § "Two cross-cutting rules every step carries",
§ "Step 3 detail" and § "A plan never rides on a branch that carries fixes".
The reasoning and the rejected alternatives are in
[ADR 0154](../../../docs/adr/0154-unattended-security-fixes-get-a-bounded-lane.md).

## Workflow

First run `lucidos planned start --kind implementation` to record Planning. This
records no approval and grants no source edit permission.

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
   Before starting each major phase, re-read the invariant checklist. When changing topology, routing, storage, security, migrations, public APIs, or process behavior, explicitly keep the touched invariants in view. If a new invariant emerges, update the plan file before continuing. After each phase commit that another phase follows, run the phase checkpoint (§ Handoff checkpoints).

6. **Hand off to verification and hardening.**
   Final verification must map back to the invariants. `/harden` is a backstop that checks the planning rule was followed; it is not where invariants should first appear.

## Required Output

Write the plan to a new file `docs/plans/YYYY-MM-DD-<slug>.md`, using the structure below. The date is today, and the slug is a short kebab-case description of the work. Commit it (`docs(plans): <summary>`) before starting the first phase. Immediately after committing, record the **proposed** marker: `lucidos planned mark --plan docs/plans/YYYY-MM-DD-<slug>.md`. Then ask for approval as described below, with the plan summary on the card (the committed file is the source of truth). Do **not** start editing yet: the edit gate stays closed while the marker is `proposed`.

### Asking for approval

**Ask with your question tool, never in prose** (`AskUserQuestion` on Claude Code, `ask_user_question` on the `lucidos` MCP server on Codex). Prose is what fails: told only to "present the plan and wait", the agent writes a summary, ends the turn, and the thread sits idle until the user types "approve" by hand. In the Lucidos UI the tool renders as clickable buttons, so approval is one tap.

Run the plan-approval checkpoint below first. It is Lucidos's copy of the default plan flow (`system-knowhow/coding-agent-flows/implementation-plan`), and `./scripts/check-default-flows.sh` keeps the two identical. Lucidos records an approval with `lucidos planned approve`.

<!-- default-flow: implementation-plan -->
#### Handoff checkpoints

**Plan approval.** Before you ask for approval, run
`lucidos successor offer --at plan-approval --ask --progress 0/<phases>`, where
`<phases>` counts the plan's phases. Use your normal user question tool with
relevant choices from the shape it prints. Follow-up questions may choose the
backend and then the rung.

Always include the option to approve here and `Commit plan for later`.
Include the other installed backend when its ladder was read, unless the
recommended option already names it. A typed reply requests changes; it needs
no option. A real fork that has no slot is named in the question as a typed
reply. A re-plan runs this checkpoint again before it asks again.

**A fresh-session pick.** It approves implementation: record the approval as
this repository does. Then run `lucidos successor start --narrative <file>`
with the flags the checkpoint printed, and end your turn. The narrative names
the plan's path, says the user approved implementation, and quotes the phase
checkpoint below.

**Commit plan for later.** Record plan-only approval as this repository does
(`lucidos planned approve --plan-only` in Lucidos), commit only the plan, and
end the turn. Never edit source or begin implementation. A later implementer
proposes the applied plan and asks for implementation approval anew.

**Phase commit.** After you commit a phase that another phase follows, run
`lucidos successor offer --at phase-commit --progress <done>/<phases>`, where
`<done>` counts the phases committed. Follow what it prints. Skip it after the
last phase. Other-backend automatic rungs are taken only when the user, plan
or approval pick named that backend; otherwise follow the own-backend verdict.
<!-- /default-flow: implementation-plan -->

Use normal user questions with at most four options each. Offer relevant
choices rather than every rung. Backend and rung may be separate questions.
The checkpoint prints example shapes. For choices sharing one question, fill
slots in priority A, P, X, R, F, T; display R, X, T, A, P, F:

| Code | Option | Eligibility |
|---|---|---|
| A | `Approve here` (or `Approve` with no readable handoff) | Always |
| P | `Commit plan for later` | Plan approval only |
| X | `Implement in a fresh session: <rung> on <backend>` | Other installed backend's ladder read; merge into R when R names it |
| R | `Implement in a fresh session: <rung> (Recommended)` | Estimate pays, past ceiling, or a lower/raise pick |
| F | A real fork of the plan | When one exists; name it as a typed reply if slots fill |
| T | `Implement in a fresh session: <this thread's rung>` | Paying card, only if it differs from R |

Never include `Request changes`: the user can type a nuanced answer directly
into the card. With no handoff available, offer `Approve`, P, and a real fork
if any. Plan-only approval never opens the source edit gate. The plan as
written is the thorough, long-term fix; offer a narrower variant as a fork.

This is a **DECISION** question, not a post-work confirmation, so the "never ask about work you've already done" rule in the system prompt does not apply to it. The distinction is concrete: source edits are *blocked* until it is answered, and a plan is a proposal about work that has NOT been done, so there is nothing to hand off yet.

**Prose is not a lighter-touch version of the tool, it is silence.** The tool call is the only thing that parks the thread in `WaitingForUserAnswer`, the sole input to `thread_lifecycle::is_attention_needing`. That state does four things:

- it lights the Blocked badge;
- it keeps the thread in the Current section;
- it bubbles up the ancestor chain;
- it fires the "When agent needs me" trigger that notifies the user.

A question typed into your final message ends the turn instead. The thread then looks finished, and nobody is told you are waiting.

When the user approves implementation, run `lucidos planned approve`. This flips the marker to gate-satisfying and unblocks source edits and Apply. It also rewrites the plan's `- **Status**: Proposed` line to `Approved` and commits that, so keep the line exactly as the template writes it. Lucidos checks that the user approved: `lucidos planned approve` is refused unless the user tapped an approving option, or typed a plain approval such as `approve` or `lgtm`, after you proposed the plan. **A tapped fork is an approval too**: revise the plan file to the chosen variant, re-commit, then run `lucidos planned approve`. A fork pick does not re-mark.

**A typed reply that asks, suggests or questions is a request for changes, never an approval.** Revise the plan file, re-commit, run `lucidos planned mark --plan <path>` again, and ask again (the marker stays `proposed` until approved). When an invariant changes mid-implementation, update the file (per Workflow step 5).

A re-plan after approval re-runs `lucidos planned mark --plan <path>`, which proposes it again, then asks again.

**The card carries the plan.** A message that also calls a tool never reaches the user, so the summary goes on the card. Write the card's question as a short markdown summary of the plan: what changes, the phases, the invariants that matter. Put the ask on its last line, such as "Approve the plan?". When the second option is a fork, name that trade-off in a few words.

**A re-ask carries what changed.** The user has already read the plan, so the card says what moved and why, in a sentence or two. The committed file holds the rest.

Fill the Reachable Population section using the [population check](../../rules/build-for-users.md).

```md
## Implementation Plan

- **Status**: Proposed

### Context
<brief summary of what is being implemented and the inputs used>

### Settled Decisions
<decisions from the prompt, grill/design thread, ADRs/plans, or docs>

### Non-goals / Deferred Work
<explicit boundaries so implementation does not drift>

### Reachable Population
<who the fix covers, who it excludes, and why>

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

### Implement in a fresh session

The checkpoint reads both installed backends' ladders against one reference.
Compare intelligence, cost and time only when both references are present and
equal. Recommend a rung by the remaining work, never by a hardcoded model name.
When the estimate pays or the context ceiling is exceeded, recommend a fresh
session. Otherwise recommend one for a lower rung that fits, or one or two
rungs above this thread for hard work when available, never the top.

A handoff needs no permission card. A fresh-session pick approves implementation. Run `lucidos planned approve`, then write the six-section narrative.
Run `lucidos successor start --narrative <file>` with the checkpoint flags
and the chosen backend, model id and effort.

Use the full other-backend flags printed by the ladder; do not
retain this backend's model id when changing backends. The narrative names the
approved plan and quotes the phase checkpoint. End the turn so the successor
continues the approved plan without another approval question.
