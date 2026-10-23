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
`<phases>` counts the plan's phases. Ask with your question tool, in the card
shape it prints. The option to stay is `Approve here`. With no new-thread
option, the floor is `Approve` / `Request changes`, and a real fork takes the
second slot. A re-plan runs this checkpoint again before it asks again.

**A new-thread pick.** It approves the plan: record the approval as this
repository does. Then run `lucidos successor spawn --build-chain --narrative
<file>` with the flags the checkpoint printed. The narrative names the plan's
path, says the user approved it, and quotes the phase checkpoint below.

**Phase commit.** After you commit a phase that another phase follows, run
`lucidos successor offer --at phase-commit --progress <done>/<phases>`, where
`<done>` counts the phases committed. Follow what it prints. Skip it after the
last phase.
<!-- /default-flow: implementation-plan -->

Use exactly one question, with the options the checkpoint prints. `Approve` is the first option unless that output leads with a new-thread option (§ Implement in a new thread). What goes in the second slot is a **floor, not a fixed shape**:

| Option | Meaning | When |
|---|---|---|
| `Approve` | Start implementing the plan as written. | First, unless the new-thread option leads; then it reads `Approve here`, second. |
| A fork of the plan (`Frontend only`, `Skip the migration`, …) | Approve a named variant. | Whenever the plan has a genuine fork: a narrower scope, one layer instead of two, a different approach. |
| `Implement in a new thread: <implementer> (Recommended)` | Approve as written, and build in a *successor thread* on that implementer. | **Only as the approval checkpoint prints**: when the *handoff estimate* pays, or when your recommended implementer is a lower rung than this thread's. It then goes FIRST, then `Approve here`. A paying card may also name this thread's implementer second. May sit beside a fork, within 4 options. |
| `Request changes` | Something needs reworking before implementation. | **Only** when the plan offers no real fork and no new-thread option. Never alongside either. |

**The plan as written is the thorough, long-term fix.** The maintainer always prefers it to a narrower patch. So write the thorough variant as the plan, and offer the narrower one as the fork, never the reverse. An unattended run still stops at the decision, as a plan or under `Decisions:`: the preference picks which variant to recommend, not whether to ask.

Claude Code's tool requires **2-4 options**, so a lone `Approve` button cannot be expressed. That is the *only* reason `Request changes` exists as a default second option: it fills the mandatory slot while carrying a real "don't start yet" decision. A genuine fork fills the same slot better, because it satisfies the minimum **and** tells you what to do.

Carrying `Request changes` as a *third* option beside a fork is the failure this rule prevents. It then means only "I will type what I want changed", which is the escape every card already has, and tapping it sends back the literal label so you have to re-ask what to change. That is precisely the dead-end shape the system prompt's NEVER AUTHOR AN "OTHER" OPTION rule bans, and a live card carried it on 2026-08-04 (`Approve` / `Frontend only` / `Request changes`).

The user can type a nuanced answer straight into the prompt and it arrives as their answer to the question, so no "let me type it" option is ever needed in any form. Lucidos renders exactly the options you pass (CC's tool description promises an automatic "Other" option; Lucidos adds none). (Codex's tool allows omitting options for free text; the same shapes are still right.)

This is a **DECISION** question, not a post-work confirmation, so the "never ask about work you've already done" rule in the system prompt does not apply to it. The distinction is concrete: source edits are *blocked* until it is answered, and a plan is a proposal about work that has NOT been done, so there is nothing to hand off yet.

**Prose is not a lighter-touch version of the tool, it is silence.** The tool call is the only thing that parks the thread in `WaitingForUserAnswer`, the sole input to `thread_lifecycle::is_attention_needing`. That state does four things:

- it lights the Blocked badge;
- it keeps the thread in the Current section;
- it bubbles up the ancestor chain;
- it fires the "When agent needs me" trigger that notifies the user.

A question typed into your final message ends the turn instead. The thread then looks finished, and nobody is told you are waiting.

When the user approves, run `lucidos planned approve`. This flips the marker to gate-satisfying and unblocks source edits and Apply. It also rewrites the plan's `- **Status**: Proposed` line to `Approved` and commits that, so keep the line exactly as the template writes it. Lucidos checks that the user approved: `lucidos planned approve` is refused unless the user tapped an approving option, or typed a plain approval such as `approve` or `lgtm`, after you proposed the plan. **A tapped fork is an approval too**: revise the plan file to the chosen variant, re-commit, then run `lucidos planned approve`. A fork pick does not re-mark.

**A typed reply that asks, suggests or questions is a request for changes, never an approval.** Revise the plan file, re-commit, run `lucidos planned mark --plan <path>` again, and ask again (the marker stays `proposed` until approved). When an invariant changes mid-implementation, update the file (per Workflow step 5).

A re-plan after approval re-runs `lucidos planned mark --plan <path>`, which proposes it again, then asks again.

**The card carries the plan.** A message that also calls a tool never reaches the user, so the summary goes on the card. Write the card's question as a short markdown summary of the plan: what changes, the phases, the invariants that matter. Put the ask on its last line, such as "Approve the plan?". When the second option is a fork, name that trade-off in a few words.

**A re-ask carries what changed.** The user has already read the plan, so the card says what moved and why, in a sentence or two. The committed file holds the rest.

```md
## Implementation Plan

- **Status**: Proposed

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

### Implement in a new thread

A planning thread that ran a long grill carries every question, picture and
draft into each later turn. The plan file already holds what a builder needs, so
a fresh thread can start from it alone.

The plan-approval checkpoint prints the card's options. Offer exactly what it
prints:

- **When the *handoff estimate* pays**, the paying card below. The estimate
  weighs this thread's context against the calls the phases left will take.
- **When it does not pay**, a new thread only for a lower rung on the
  intelligence ladder than this thread's (ADR 0497). Otherwise the plain floor.
- **When the engine could not be read, or the thread cannot hand off**, the plain
  floor.

**Your judgement picks the implementer** (ADR 0486): one rung on the
*intelligence ladder*, a model at an effort. The output prints the ladder with
this thread's rung marked, and `lucidos intelligence-ladder` prints it on
demand. This skill names no model and no effort, because the ladder carries
them (ADR 0500). The principle:

- Turn intelligence down for settled work. A fully specified plan with settled
  decisions suits a lower rung.
- Turn it up for hard or open work. A plan that leaves design judgement to the
  builder keeps this thread's rung or takes a higher one.
- Say why in one line of your message.

Every call reads the whole context again, so a rung above what the work needs
costs on every turn.

The paying card reads, never more than 4 options and never `Request changes`:

1. `Implement in a new thread: <implementer> (Recommended)`, the rung named by
   its picker label and effort as the ladder prints them.
2. `Implement in a new thread: <this thread's implementer>`, only when it
   differs.
3. `Approve here`.
4. A real fork of the plan, if it has one.

A card that does not pay drops option 2: a new thread on this thread's
implementer then saves nothing.

Name the backend only when it is not this thread's.

Picking a new-thread option is an approval, asked once. The owner's pick on a
device is also their consent to the handoff (ADR 0493), so it needs no owner
approval card. A typed reply naming another implementer counts as an approval
too, but a refused handoff then needs `lucidos ask-owner-approval`:

1. Run `lucidos planned approve` in this thread.
2. Write a handoff narrative and run `lucidos successor spawn --build-chain
   --narrative <file>` with the flags the checkpoint printed. Add only the flags
   that differ from this thread's implementer: `--coding-agent`,
   `--coding-agent-model`, `--reasoning-effort`. An omitted flag keeps this
   thread's, and ids go in, never picker labels.
   `lucidos successor --help` has the six sections. The narrative names the plan
   path, says the user approved it here, and quotes the phase checkpoint. The
   builder then builds it phase by phase without asking again.
3. End your turn with the link it prints.

The successor's branch starts at this branch's last commit, so it already holds
the plan, and the approved marker carries over. From a top-level planning
thread the builder starts a *build chain* (ADR 0503). It is a `no-wake` child
of this thread, which stays open and proposes nothing. Its successors join the
chain under this thread, and need no owner approval card when the handoff
estimate pays. From a child planning thread the builder takes this thread's
place, so the parent still gets the report. The same text reaches the agent
through the checkpoint, from `card_shape` and `new_thread_handoff` in
`crates/lucidos-cli/src/planned.rs`.

When the estimate does not pay and the thread has a rung above its own, the
checkpoint also prints a raise clause. When no lower rung fits but the plan is hard or long, offer
`Implement in a new thread: <a higher rung> (Recommended)` beside `Approve
here`. Go one or two rungs up, never to the top. If neither applies, keep
`Approve` / `Request changes`.
