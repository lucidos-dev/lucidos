# No Em Dashes

**Always loaded** (no `paths:` frontmatter): this rule governs prose, chat replies, and commit messages as well as file edits, so it cannot be gated on a touched file path.

## The rule

**Never write an em dash.** The character is **U+2014 EM DASH**. It is banned outright, with no exceptions and no contexts where it is acceptable. **U+2015 HORIZONTAL BAR is banned with it**: it is a visual lookalike, so a check written for U+2014 alone lets it through.

**No file type and no context is exempt.** Code comments, doc prose, `CHANGELOG.md`, commit messages, `echo` and `log!` strings, UI strings, system-prompt and knowhow text, and **the agent's own chat replies to the user**. That last one is why this rule is always loaded rather than gated on a path: no script can see a chat reply.

Both characters are written here as `<U+2014>` and `<U+2015>` rather than typed, so this file stays clean under its own rule. That convention is why the gates need no exemption for the files that define them, and there is no escape hatch: if a genuine case ever appears, document it here first and narrow the check to exactly that case, never a blanket path exclusion.

## What to write instead

A comma, a colon, parentheses, or a split into two sentences. Pick whichever the sentence actually wants.

| Before | After |
|---|---|
| `Git is the artifact store but **never the authority** <U+2014> events are.` | `Git is the artifact store but **never the authority**: events are.` |
| `"Pre-existing" is never an excuse <U+2014> if you see it, you own it.` | `"Pre-existing" is never an excuse. If you see it, you own it.` |

The first takes a colon because the clause after the dash explains the one before it. The second splits, because both halves stand alone. A mid-sentence aside takes a comma; a true bracketed aside takes parentheses.

## U+2013 EN DASH is NOT banned

An en dash is legitimate in a numeric range (`3–5`, `2024–2026`, `Phases 1–3`). It is not covered by this rule, and the deterministic checks deliberately ignore it. Do not widen them on a guess.

## Not retroactive: never attempt a sweep

**A repo-wide sweep is explicitly out of scope and must not be attempted.** The tree carried 29,046 lines with an em dash across 1,993 tracked files as of 2026-07-30. A sweep would be an unreviewable diff across every crate and would collide with every in-flight branch.

**The rule binds new and modified lines**, so the count decays as files are touched. Rewording a line that keeps its dash counts as adding one: touch a line and you own it.

## Enforcement

Two deterministic gates, both diff-scoped and added-lines-only, both hard failures rather than warnings (a warning is how 184 accumulated in one workflow file):

- **Write time**: `.claude/hooks/no-em-dashes.sh`, a `PreToolUse` hook on `Edit`, `Write` and `Bash`. The `Bash` arm covers `git commit -m`.
- **Review time**: `./scripts/check-em-dashes.sh`, run by `/harden` Phase 4.5 for every diff. This is what covers **Codex, which has no hooks**.

Both share `scripts/lib/em_dash_scan.sh`, the single source of truth for the two characters and the advice text; tested by `scripts/lib/em_dash_scan_test.sh`. Those three file headers document the mechanism, including which gate fails open and which fails closed, and why.
