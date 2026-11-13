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

**A comment never defends a shortcut, workaround or known-wrong behaviour.**
The next reader, human or agent, takes it as settled design and builds on it.
Fix the root cause. Failing that, register a temporary measure, or record a
known limitation in a plan or an ADR. An invariant ("callers must hold the
lock") is still fine.

- Bad: `// Polling each second is fine for now.`
- Good: `// Polls until SSE covers lists (docs/temporary-measures.md).`

Pilot: `docs/plans/2026-09-25-comment-ablation-pilot-results.md`.

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
Simplified Technical English. For anything left unspecified here, follow
Google's developer documentation style guide.

**STE's dictionary is not adopted.** It bans "verify", "check", "confirm" and
"ensure" in favour of "make sure", and those four are our canonical words.
Vocabulary belongs to `.claude/rules/glossary.md`.

## Not retroactive: no sweep unless commissioned

**Never start a repo-wide sweep on your own**: it is an unreviewable diff that
collides with every in-flight branch. The maintainer can commission one. It then
runs per file, one commit each, against a plan.

The rule binds **new and modified lines**, so the count decays as files are
touched. Rewording a line counts as adding it: touch a line and you own it.

## Enforcement

The four limits are hard failures on added lines, over markdown and
`//`-comment sources: `.claude/hooks/prose.sh` at write time, and
`./scripts/check-prose.sh` in `/harden`. Mechanism: `docs/glossary.md` § Prose
gate. The review-only rules are a `code-review` angle. Nothing but this text
reaches a chat reply.
