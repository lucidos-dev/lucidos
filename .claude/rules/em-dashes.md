# Em Dashes

**Always loaded** (no `paths:` frontmatter): this rule governs prose, chat replies, and commit messages as well as file edits, so it cannot be gated on a touched file path.

## The rule

**An em dash (U+2014) takes a space on both sides.** Write `start <U+2014> another`, never `start<U+2014>another`: a monospace font, like the app's default, draws the dash one cell wide, so unspaced it reads as a hyphen. A line edge counts as a space.

**U+2015 HORIZONTAL BAR is banned in every form**, as a lookalike.

**No file type and no context is exempt.** That covers comments, docs, `CHANGELOG.md`, commit messages, log and UI strings, prompt and knowhow text. It also covers **the agent's own chat replies to the user**. That last one is why this rule is always loaded rather than gated on a path: no script can see a chat reply.

This file writes both as `<U+2014>` and `<U+2015>`. No escape hatch: document a genuine case here first, then narrow the check to exactly it, never a path exclusion.

## Use it sparingly

A spaced dash is allowed, not encouraged. A comma, a colon, parentheses or a split into two sentences often reads better:

| Dashed | Often better |
|---|---|
| `Git is the artifact store but **never the authority** <U+2014> events are.` | `Git is the artifact store but **never the authority**: events are.` |
| `"Pre-existing" is never an excuse <U+2014> if you see it, you own it.` | `"Pre-existing" is never an excuse. If you see it, you own it.` |

**Never swap a dash for a bare hyphen.** `start-another` invents a compound word. Rewrite the sentence instead.

## U+2013 EN DASH is NOT regulated

An en dash is legitimate in a numeric range (`3–5`, `2024–2026`, `Phases 1–3`). This rule does not cover it, and the deterministic checks deliberately ignore it. Do not widen them on a guess.

## Not retroactive: never attempt a sweep

**A repo-wide sweep is explicitly out of scope and must not be attempted.** The tree carried 29,046 lines with an em dash across 1,993 tracked files as of 2026-07-30, most of them unspaced. A sweep would be an unreviewable diff across every crate and would collide with every in-flight branch.

**The rule binds new and modified lines**, so the count decays as files are touched. Rewording a line that keeps an unspaced dash counts as adding one: touch a line and you own it.

## Enforcement

Two deterministic gates, both diff-scoped and added-lines-only, both hard failures rather than warnings (a warning is how 184 accumulated in one workflow file):

- **Write time**: `.claude/hooks/em-dashes.sh`, a `PreToolUse` hook on `Edit`, `Write` and `Bash`. The `Bash` arm covers `git commit -m`.
- **Review time**: `./scripts/check-em-dashes.sh`, run by `/harden` Phase 4.5 for every diff. This is what covers **Codex, which has no hooks**.

Both share `scripts/lib/em_dash_scan.sh`, whose `banned()` is the single definition of a banned dash; tested by `scripts/lib/em_dash_scan_test.sh`. Those three file headers document the mechanism, including which gate fails open and which fails closed, and why.

The chat spaces a slipped dash visually (`spaceUnspacedEmDashes`), which is no licence to write one. Why: ADR 0332.
