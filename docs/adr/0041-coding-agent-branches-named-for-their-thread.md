# 0041: Coding-agent branches are named for their thread, not a timestamp

- **Status**: Accepted (the accepted allocation race is superseded by 0076)
- **Date**: 2026-08-04

## Context

Every branch an *agent session* created was named `claude-code/<YYYYMMDD-HHMMSS>-<6-hex>`,
with app coding-agent threads on `claude-code/app/<app_id>/<ts>-<uuid>`. Two
problems compounded:

- **The name said nothing about the work.** `git branch -a` in the Lucidos source
  repo, and in every *external repo* Lucidos touches, was a wall of timestamps.
  Matching a branch to the thread that owns it meant a database lookup.
- **The name said `claude-code` even for Codex.** The prefix predates the second
  backend (ADR 0004) and was kept deliberately, so a Codex thread's branch
  claimed to be a Claude Code one.

ADR 0004's "Deliberate no's" had recorded the prefix as a non-rename, grouped
with `EventChannel::ClaudeCode`, `source = 'claude_code'` and the `cc_*` field
names, on the grounds that they are load-bearing wire/DB/recovery surfaces and a
rename would be migration-sized.

## Decision

Coding-agent branches are named

```
lucidos-<coding-agent>-<app|repo>-<scope-name>-<slug>[-<n>]
```

| Thread kind | Branch |
|---|---|
| Lucidos source | `lucidos-claude-code-repo-lucidos-fix-auth-timeout` |
| App | `lucidos-claude-code-app-habit-tracker-add-streaks` |
| External repo | `lucidos-codex-repo-example-repo-fix-auth` |

`<coding-agent>` is `CodingAgent::as_str()`. `<slug>` is the thread's display
name (title, else first message) kebab-cased and capped at 48 chars on a word
boundary, falling back to `thread-<8 hex>` when nothing survives. `-<n>` starts
at `2` and is allocated against the branches that exist at that moment, so a
name freed by a deleted branch is reused. The name is minted once, when the
branch is created, and a later rename of the thread does not move it.

**Existing `claude-code/*` branches are never renamed**, and every
prefix-sensitive consumer accepts both shapes (`git_ops::is_coding_agent_branch`).

The rest of ADR 0004's bullet stands: `EventChannel::ClaudeCode`,
`source = 'claude_code'`, the `cc_*` columns and the `/api/v1/claude-code/*`
routes keep their names.

## Rationale

**The branch prefix is the one member of ADR 0004's bullet with no persisted
schema behind it.** A branch name is a per-branch string: old branches keep
theirs, new branches get the new shape, and the two coexist with no migration
and no backfill. The three names that stay are exactly the ones that would
require rewriting every persisted row and every client. The bullet grouped four
things that turned out not to be alike.

**Recovery's prefix scan, the specific objection ADR 0004 raised, is one
predicate.** It became `is_coding_agent_branch`, which accepts both prefixes and
is unit-tested against user branches (`feature/lucidos-integration` is not one).
Recovery still applies the workspace-marker check on top, so the prefix is a
filter, not an authorization.

**`lucidos-` rather than `coding-agent-` or a per-agent prefix.** In an external
repo the branch sits among the user's own; the segment that matters most there
is the one saying Lucidos made it. It is also the single string consumers match
on, so widening the family later is one edit.

**A scope segment on every branch, including Lucidos-source ones.** `repo-lucidos`
partly repeats the `lucidos-` prefix, which is the honest cost of one uniform
shape. The alternative, dropping the scope for the most common thread kind, buys
~13 characters and makes the name shape conditional on thread kind, which every
reader and every future parser then has to know about.

## Consequences

- `git branch -a` reads as a list of work. A branch, its thread, and its agent
  are legible without a database.
- **A new correctness hazard, fixed in the same change.** Sibling branches now
  share a prefix (`…-fix-auth` versus `…-fix-auth-2`), and
  `is_merge_of_branch_into_main` matched merge subjects with a bare
  `line.contains(branch)`. Merging the `-2` branch would have reported the first
  as already merged, and `apply_change` would have resolved the user's Apply as
  an idempotent no-op: told it already landed, commits never reaching main.
  Matching is now token-bounded on both halves. The same hazard was already
  latent for `claude-code/foo` versus `claude-code/foobar`.
- **An unanswered ref listing must not be read as "the name is free."** Allocation
  reads `git for-each-ref`, and a timeout there is routine on a saturated host.
  It falls back to `<base>-<6 hex>`, unique by construction. Same rule as
  `GitAnswer`, applied to a listing rather than a yes/no.
- **A millisecond-wide allocation race is accepted.** Two spawns that allocate the
  same name in the same instant both try `git worktree add -b`, and the loser
  fails with "a branch already exists", which the spawn surfaces as a retryable
  error. Closing it would mean creating the ref during allocation and handing
  `worktree add` a pre-existing branch, which blurs "did this attempt create it"
  exactly where `cleanup_failed_spawn` needs the answer to decide what it may
  delete.
- **That race was reversed by ADR 0076.** Four of six siblings spawned in one
  response died on it. The name now carries the thread's short id, and the
  create retries. 0076 leaves the `cleanup_failed_spawn` answer alone.
- **Names are lowercase-only.** Git refs are case-sensitive but loose refs are
  files, so on a case-insensitive filesystem (macOS default) a mixed-case slug
  could collide with its own lowercase twin.
- The e2e sweep's disposable-workspace half matches `lucidos-*` by name; the half
  that runs against the shared canonical repo still discriminates purely by
  worktree path, because a real session's branch now carries that same prefix.

## Alternatives considered

**Keep `claude-code/` and append a slug** (`claude-code/<slug>-<ts>`). Preserves
every existing prefix match for free, but keeps telling the user that a Codex
thread is a Claude Code one, which is the half of the problem that has no
workaround.

**Namespace with slashes** (`lucidos/<agent>/<slug>`). Reads well in tools that
render refs as a tree. Rejected: git stores loose refs as files, so
`lucidos/claude-code/fix-auth` and `lucidos/claude-code/fix-auth/2` are a
directory/file conflict, and the duplicate numbering would have had to avoid
exactly the shape it wants. The flat form has no such constraint.

**A high-water-mark counter for duplicates** (never reuse a number). Would make a
branch name globally unique over the repo's history. Rejected: it needs durable
state that a branch name should not depend on, and the reuse it prevents is
harmless, since the earlier branch is gone by then.

**Rename existing branches to the new shape.** Rejected outright. A live branch
name is recorded in `changes.branch_name` and in every `SessionStarted` event,
and may be checked out in a live worktree. Renaming buys consistency in
`git branch -a` and risks stranding in-flight work.

**Pre-create the ref during allocation** to close the concurrency race. Rejected:
see Consequences. The race's failure mode is loud, non-destructive and
retryable; the fix's failure mode is a failure-path cleanup that no longer knows
what it owns.
