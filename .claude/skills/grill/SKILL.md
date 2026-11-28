---
name: grill
description: Interview the user relentlessly about a plan or design until you share an understanding, asking each decision-tree question via AskUserQuestion with multiple-choice options. Default skill when the user wants to stress-test a plan, get grilled on their design, or says "grill me". Uses and sharpens the project glossaries so vocabulary stays aligned across code, UI and prose.
---

# Grill (Lucidos)

Project-scoped grill skill for the Lucidos repo. Interview relentlessly via `AskUserQuestion`, AND: actively **use** the project glossaries while grilling, and **sharpen** them in the same conversation when concepts crystallize.

## Intent

> Interview me relentlessly about every aspect of this plan until we reach a shared understanding. Walk down each branch of the design tree, resolving dependencies between decisions one-by-one. For each question, provide your recommended answer.
>
> Ask the questions one at a time.
>
> If a question can be answered by exploring the codebase, explore the codebase instead.

## Door test: ask only about one-way doors

Before each question, ask yourself one thing: **can this decision change cheaply later?**

| Door | Examples | What you do |
|---|---|---|
| **Two-way** | A schedule, a name inside one file, an internal module split | No card. Decide it yourself, and record it under *Decided without asking*. |
| **One-way** | What gets recorded, a default that reaches users, a schema, a security boundary | Ask it on a card. |
| **Seen by the user** | What is drawn, where it lives, how the user reaches it, what it says, a step they must take | Always ask it on a card, with pictures from [visual-options](../visual-options/SKILL.md). |

A one-way door loses data or work if the answer is wrong. When unsure, ask: a wrong guess there costs more than one card.

**A choice the user sees is never two-way**, however cheap the code is to change. The user lives with a bad experience until someone notices it, so it never goes under *Decided without asking*. A settled convention is not a choice: apply it without asking (`.claude/rules/frontend-css.md`). These cards count in the progress bar like one-way doors.

### Walk the user journey

When the design has a surface the user meets, write its journey as numbered steps before the last question. Start where the user is, not where the feature lives. Cover how they find it, first use, errors and empty states, and the way out, on desktop and phone. Read each step as the user. A step that makes them hunt, type a command, sign in again or guess is a design bug: ask how to remove it. The journey goes into the design plan, and the implementation plan carries it into its *User Journey* section.

*Decided without asking* lists every two-way decision with its one-line reason, so the user can strike any of them. Put it in the design plan, or in the grill's closing message when it writes no plan. A struck item becomes a question.

## Question format

Ask each question via the `AskUserQuestion` tool, not as plain text.

- Phrase the question clearly and end with a question mark.
- Provide 2–4 plausible options. Make the recommended option first and append " (Recommended)" to its label.
- Recommend the thorough, long-term option over a quick patch. The maintainer always prefers it.
- Each option needs a short `description` explaining the trade-off or implication.
- Keep `header` to a short chip-style label (max 12 chars).
- Use `multiSelect: true` only when choices genuinely aren't mutually exclusive.

The user can always type a free-text answer in the prompt instead of picking, so don't pad with junk options just to hit four: three sharp ones beats four with filler. That escape is the prompt textarea, NOT an option, so never add an "Other" / "Let me type it" choice. Lucidos renders exactly the options you pass, and picking one sends its label back as the answer, so that button would report "Other" as the user's decision.

**When the user asks back.** A typed reply that asks something is not an answer. Answer it in a visible message before the next question.

**Plain-text fallback:** if a question is genuinely open-ended and no 2+ plausible options exist (e.g. "paste the error message", "what's the API key"), ask in plain text. Use this fallback sparingly — most design questions have natural option sets.

## Progress bar

Open every grill question with a progress line, as the first line of the question text:

```
`██████░░░░░░░` 6 of 13 decided
```

- **One cell per one-way door.** `█` for each decided one, `░` for each one still open. Above 20, draw 20 cells and fill them in proportion. Two-way doors never appear in the bar.
- **Wrap the cells in a code span.** The card renders markdown, and a code span keeps the cells one width.
- **The line describes itself.** Never write "the bar above" or point at earlier prose: the card must make sense alone, and the question hook refuses a card pointing "above".
- **A decided question advances it.** That is a picked option, or a typed answer that settles the question.
- **A re-ask does not.** When the user typed a question back, answer it, then ask again with the same count.
- **Glossary cards do not count.** They still open with the bar, at the current count.

### Keep the total honest

The total is an estimate of the open decision tree, not a promise. Re-count it before every question:

- An answer that opens a branch adds its questions.
- A question that turns out not to be a decision, that the codebase answered, or that turns out to be a two-way door drops out.

When the total moves, say so on the same card, on the line after the bar, with the reason: *"Total 13 → 15: the sync answer opened conflict handling."* Never move the total silently, and never lowball it to make the end feel closer.

When the grill ends on a design-approval card, that card shows the bar full and carries *Decided without asking*.

## Use and sharpen the project glossary

The grill exists to produce **shared understanding**, and shared understanding rides on shared vocabulary. Lucidos has two glossaries that the grill both consults and improves:

- **`system-knowhow/glossary.md`** — user-facing terms (app, trigger, knowhow, intent, script, artifact, manifest, thread variants, event, domain event, workspace, plugin). Also loaded by the workspace LLM at runtime.
- **`docs/glossary.md`** — dev-only extension (aggregate, actor, EventBus, BusEvent, SystemEvent, ThreadEvent, EventMeta, projection, change, request_id, channel, signer, hardening, worktree, Loadable<T>, scheduler blocklist, …).

### At the start of every grill session

Read both files before asking the first question:

- `system-knowhow/glossary.md`
- `docs/glossary.md`

Skim the "Names you might be tempted to use but shouldn't" lists in `.claude/rules/glossary.md` so you recognize synonym reaches when they happen.

### While grilling

1. **Phrase every question and every option in canonical terms, verbatim.** When you reach for a word that's defined in either glossary, use the glossary's word — not a synonym, not a conversational paraphrase. The canonical term IS the conversation. Example: ask "Should this be a *sub-thread* or a *top-thread*?" — not "child thread or independent thread".
2. **Flag synonym reaches in either direction.** If the user types a synonym you recognize (e.g. *child thread* → **sub-thread**, *event store* → **EventBus**, *task* → **intent**, *recipe* → **knowhow**, *attachment* → **artifact**), acknowledge inline in one sentence and switch to the canonical: *"You said ‘child thread' — the canonical is ‘sub-thread'; I'll use that going forward."* If you catch yourself doing it, correct yourself before the user has to.
3. **Sharpen the glossary in the same turn when a concept crystallizes.** Three triggers:
   - **New concept emerges.** The conversation names something worth a glossary entry that isn't there yet. Propose the entry inline.
   - **Existing entry is vague.** The grill exposes that an entry's definition can't disambiguate a real case you're discussing. Propose a refinement.
   - **Glossary contradicts reality.** A term has been renamed or its meaning has shifted in the code/UI and the glossary hasn't caught up. Propose the update.

   For each, show the proposed entry (or refined wording) and get one-click approval via `AskUserQuestion`. Suggested option set:
   - *"Add as proposed"* (Recommended when wording is solid)
   - *"Skip, not a real term"*

   Two options, not three: per the question format above, an *"I'll type revised wording"* button is the escape the prompt textarea already provides, and tapping it just sends that label back as the answer. If the user wants different wording they type it, and it arrives as their answer.

   On approval, write the entry to the correct layer (user-facing → `system-knowhow/glossary.md`; dev-only → `docs/glossary.md`). On revision, take the user's wording. Per `.claude/rules/system-knowhow.md`, the glossary edit ships in the same commit as whatever code or plan it's grounding. **Don't file glossary edits as TODOs**, they're first-class grill work, not bookkeeping.

### Beyond grilling

This practice is not grill-specific. It applies to brainstorming (`superpowers:brainstorming`) and to any back-and-forth where you and the user are pinning down what to build. If you are in a design dialogue and this skill is not loaded, the three behaviours above still hold: phrase in canonical terms, flag synonym reaches in either direction, and propose the glossary edit in the turn the concept crystallizes.

### The point

Every grill session either reinforces the canonical vocabulary or improves it. Drift never accumulates. The next grill, by you, by another agent session, or by the user reading back through `docs/glossary.md`, starts from sharper ground. That's the multiplier.

## Record a settled design

When the dialogue is producing a design plan, run `lucidos planned start --kind design`
before writing it. This records Designing without opening the source edit gate.
Write the settled decisions to `docs/plans/YYYY-MM-DD-<slug>-design.md`; a design
plan has no implementation phases and includes `- **Status**: Proposed`. Commit the file, then run
`lucidos planned mark --plan <path> --kind design`.

Ask with your question tool (`AskUserQuestion` on Claude Code,
`ask_user_question` on the Lucidos MCP server on Codex). Put the design summary
and any changed decisions on the card itself. Offer `Approve design` first,
then a real design fork if one exists; otherwise offer `Approve design and plan here`.
Typed questions and suggestions mean revise the design, commit it, mark it again,
and ask again. A fork approves that variant after its file is revised and committed.

After approval, run `lucidos planned approve`. Design approval always records
plan-only scope: it permits landing design plan files alone and never source edits.
For `Approve design`, end the turn with the committed design alone so Lucidos can
propose it. For `Approve design and plan here`, keep the design on the branch and
run the `implementation-plan` skill here. That skill starts implementation
planning, records a new proposed marker, and asks for its own approval before any
source edit. Design approval does not approve implementation.

## See also

- `.claude/rules/glossary.md`: the canonical-term rule itself, carrying the synonym list, the cross-layer name-root rule, and the name-accuracy rule.
- `.claude/rules/system-knowhow.md` — drift enforcement: a glossary entry that lags behind the code is a `/harden` failure.
- `~/.claude/skills/grill-me/SKILL.md` — upstream plain-text grill (`mattpocock/skills`); this skill is the structured-choice + glossary-aware project variant.
