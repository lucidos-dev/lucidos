# Prose

**Always loaded** (no `paths:` frontmatter): this rule governs chat replies and
commit messages as well as file content, so no path can gate it. Comments also
go into brand-new files, and rules load on read, not on write.

Scope is every word we write: code comments, `docs/`, `system-knowhow/`, rules,
skills, UI strings, commit messages, and replies to the user.

## The rule

**Write plain English, concisely, in a logical order.** Lead with the claim,
then the reason. One idea per sentence. Say what is true now.

## What a comment is for

**The best comment is the one the code made unnecessary.** Before writing or
keeping one, ask whether the code already says it. If it does, delete the
comment. If it nearly does, rename the thing and then delete the comment.
Self-explanatory code beats an explained one, every time.

What survives states what a reader must not break here. Two other things get
written as comments and belong elsewhere:

- **A rejected alternative, and why not.** That is `docs/adr/`.
- **What this used to do, and when it changed.** That is the commit message and
  `docs/plans/`. Git blame already holds it, at no cost to the next reader.

Content that outgrows the block limit below is a doc, an ADR or a plan. Link to
it instead.

## Hard limits

Four, every one checked on added lines only:

| Limit | Value |
|---|---|
| Contiguous comment block | 20 lines |
| Sentence | 25 words |
| Paragraph | 6 sentences |
| An ISO date inside a comment | not allowed |

## Required, but checked at review

- **20 words for an imperative step.** The 25 above is the descriptive limit.
- **Active voice.** Passive only where the agent is genuinely unknown.
- **Noun clusters of at most 3 words.**
- **No filler.** Cut "it is worth noting", "importantly", and emphasis that only
  repeats the sentence.
- **Use the right shape.** A list is a list and a comparison is a table. Reserve
  paragraphs for arguments.

## Where the numbers come from

The sentence, paragraph and noun-cluster limits are ASD-STE100 Issue 9,
Simplified Technical English. ISO 24495-1:2023 supplies the frame: a reader
should find what they need, understand it, and use it. For anything left
unspecified here, follow Google's developer documentation style guide.

**STE's dictionary is not adopted.** It bans "verify", "check", "confirm" and
"ensure" in favour of "make sure", and those four are our canonical words.
Vocabulary belongs to `.claude/rules/glossary.md`.

ASD owns STE's copyright and its text may not be redistributed, so this file
cites the limits and quotes none of it.

## Not retroactive: no sweep unless commissioned

**Never start a repo-wide sweep on your own**: it is an unreviewable diff that
collides with every in-flight branch. The maintainer can commission one. It then
runs per file, one commit each, against a plan.

The rule binds **new and modified lines**, so the count decays as files are
touched. Rewording a line counts as adding it: touch a line and you own it.

## Enforcement

Two deterministic gates, over markdown and `//`-comment sources only. Both are
diff-scoped, added-lines-only, and hard failures:

- **Write time**: `.claude/hooks/prose.sh`, a `PreToolUse` hook on `Edit` and
  `Write`.
- **Review time**: `./scripts/check-prose.sh`, run by `/harden` Phase 4.5 for
  every diff. That is the layer covering Codex, which has no hooks.

Both share `scripts/lib/prose_scan.sh`, the single source of truth for the four
limits, tested by `scripts/lib/prose_scan_test.sh`. The review-only rules are an
angle in the `code-review` skill. Nothing but this text reaches a chat reply.
