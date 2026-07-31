# No Em Dashes

**Always loaded** (no `paths:` frontmatter): this rule governs prose, chat replies, and commit messages as well as file edits, so it cannot be gated on a touched file path.

## The rule

**Never write an em dash.** The character is **U+2014 EM DASH**. It is banned outright, with no exceptions and no contexts where it is acceptable.

**U+2015 HORIZONTAL BAR is banned with it.** It is a visual lookalike: a check written for U+2014 alone lets it through, and prose containing it reads as a violation to every human who sees it.

Both characters are written below as `<U+2014>` and `<U+2015>` rather than typed, so this file stays clean under its own rule. That convention is also why the gates below need no exemption for the files that define them.

## What to write instead

A comma, a colon, parentheses, or a split into two sentences. Pick whichever the sentence actually wants.

Real prose from `CLAUDE.md`, before and after:

| Before | After |
|---|---|
| `Git is the artifact store but **never the authority** <U+2014> events are.` | `Git is the artifact store but **never the authority**: events are.` |
| `"Pre-existing" is never an excuse <U+2014> if you see it, you own it.` | `"Pre-existing" is never an excuse. If you see it, you own it.` |

The first takes a colon because the clause after the dash explains the one before it. The second splits, because both halves stand alone. A mid-sentence aside takes a comma; a true bracketed aside takes parentheses.

## Scope: everything

This is the part that gets it wrong. There is **no file type and no context that is exempt**:

- Code comments, in every language.
- Doc prose: `docs/**`, `system-knowhow/**`, `README.md`, `CLAUDE.md`, `.claude/rules/**`, ADRs, plans.
- `CHANGELOG.md` entries.
- **Commit messages**, subject and body.
- `echo` output and error strings inside shell scripts, and `log!` / `format!` / `panic!` strings inside Rust.
- System-prompt text, knowhow text, LLM tool descriptions, UI strings.
- **The agent's own chat replies to the user.** No script can see these, which is why this rule is always loaded rather than gated on a path.

Also: marketing copy, release notes, PR-style summaries, and anything else generated on the user's behalf.

## Non-goal: this is not retroactive

**A repo-wide sweep is explicitly out of scope and must not be attempted.** As of 2026-07-30 the tree carries **29,046 lines with an em dash across 1,993 tracked files** (worst offenders: `CHANGELOG.md` 342, `scripts/release.sh` 210, `.github/workflows/install-smoke.yml` 184, `crates/lucidos-engine/src/engine/agent_session/prompts.rs` 159). U+2015 appears zero times.

Two reasons that count stays where it is. A sweep of that size would be an unreviewable diff across every crate, with no way to tell a safe substitution from a wrong one at a glance. And it would collide with every in-flight branch, since it touches nearly every file in the repo.

**The rule binds new and modified lines**, so the count decays as files are touched. Both gates below are diff-scoped for exactly this reason: a line you did not touch is not your problem, and a line you touched is. Rewording a line that keeps its dash counts as adding one.

## U+2013 EN DASH is NOT banned

An en dash is legitimate in a numeric range (`3–5`, `2024–2026`, `Phases 1–3`). It is not covered by this rule, and the deterministic checks deliberately ignore it. Do not widen them on a guess.

## Enforcement

Prose alone is what already failed here, so there are two deterministic gates. Both are diff-scoped and added-lines-only.

- **Write time (primary).** `.claude/hooks/no-em-dashes.sh`, a `PreToolUse` hook on Claude Code's `Edit`, `Write` and `Bash` tools. It refuses the write before the text exists, naming the file, the line and the offending text. The `Bash` arm covers `git commit -m`, which reaches disk through neither `Edit` nor `Write` and is invisible to any diff afterwards. It fails **open** on infrastructure trouble (no `jq`, unparseable payload) so a hook bug cannot brick a session.
- **Review time (coverage).** `./scripts/check-em-dashes.sh`, run by `/harden` Phase 4.5 for every diff, docs-only ones included. This is what covers **Codex, which has no hooks**, plus hand edits and anything else the hook did not see. It fails **closed**: a scan that cannot run exits non-zero rather than reporting clean.

Both share `scripts/lib/em_dash_scan.sh`, which is the single source of truth for the two characters and the advice text. Tested by `scripts/lib/em_dash_scan_test.sh`.

Failures are hard, never warnings. A warning is precisely how 184 of them accumulated in one workflow file.

## There is no escape hatch

None exists, and none is needed. The tree was searched for the case that would plausibly justify one, a test fixture that deliberately asserts on U+2014, and there is none: every em dash in a test file today sits in a comment or in a UI string the test compares against source, and both sides move together when that string is rewritten.

The files that define this rule and its gates would be the other candidate, and they dodge it by referring to the characters by codepoint (in prose) or by byte escape (in shell) rather than embedding them.

If a genuine case ever appears, document it here first, then narrow the check to exactly that case. Do not add a blanket path exclusion.
