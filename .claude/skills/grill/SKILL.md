---
name: grill
description: Interview the user relentlessly about a plan or design until reaching shared understanding, resolving each branch of the decision tree by asking each question via the AskUserQuestion tool with multiple-choice options. Default skill when the user wants to stress-test a plan, get grilled on their design, or mentions "grill me". Project-scoped variant for Lucidos — actively uses the project glossaries when phrasing questions and sharpens them in-flight when concepts crystallize, so vocabulary stays aligned across the codebase, UI, and user-facing prose.
---

# Grill (Lucidos)

Project-scoped grill skill for the Lucidos repo. Interview relentlessly via `AskUserQuestion`, AND: actively **use** the project glossaries while grilling, and **sharpen** them in the same conversation when concepts crystallize.

## Intent

> Interview me relentlessly about every aspect of this plan until we reach a shared understanding. Walk down each branch of the design tree, resolving dependencies between decisions one-by-one. For each question, provide your recommended answer.
>
> Ask the questions one at a time.
>
> If a question can be answered by exploring the codebase, explore the codebase instead.

## Question format

Ask each question via the `AskUserQuestion` tool, not as plain text.

- Phrase the question clearly and end with a question mark.
- Provide 2–4 plausible options. Make the recommended option first and append " (Recommended)" to its label.
- Each option needs a short `description` explaining the trade-off or implication.
- Keep `header` to a short chip-style label (max 12 chars).
- Use `multiSelect: true` only when choices genuinely aren't mutually exclusive.

The user can always type a free-text answer in the prompt instead of picking, so don't pad with junk options just to hit four: three sharp ones beats four with filler. That escape is the prompt textarea, NOT an option, so never add an "Other" / "Let me type it" choice. Lucidos renders exactly the options you pass, and picking one sends its label back as the answer, so that button would report "Other" as the user's decision.

**Plain-text fallback:** if a question is genuinely open-ended and no 2+ plausible options exist (e.g. "paste the error message", "what's the API key"), ask in plain text. Use this fallback sparingly — most design questions have natural option sets.

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
   - *"Add with revised wording — I'll type it"*
   - *"Skip — not a real term"*

   On approval, write the entry to the correct layer (user-facing → `system-knowhow/glossary.md`; dev-only → `docs/glossary.md`). On revision, take the user's wording. Per `.claude/rules/system-knowhow.md`, the glossary edit ships in the same commit as whatever code or plan it's grounding. **Don't file glossary edits as TODOs** — they're first-class grill work, not bookkeeping.

### The point

Every grill session either reinforces the canonical vocabulary or improves it. Drift never accumulates. The next grill — by you, by another CC session, by the user reading back through `docs/glossary.md` — starts from sharper ground. That's the multiplier.

## See also

- `.claude/rules/glossary.md` § "Active use during design dialogue" — the same active-use-and-sharpen rule generalized to brainstorming and any design back-and-forth.
- `.claude/rules/system-knowhow.md` — drift enforcement: a glossary entry that lags behind the code is a `/harden` failure.
- `~/.claude/skills/grill-me/SKILL.md` — upstream plain-text grill (`mattpocock/skills`); this skill is the structured-choice + glossary-aware project variant.
