# 0327: A repo's relative additionalDirectories reach past a worktree through --add-dir, not --project-config-root

- **Status**: Accepted
- **Date**: 2026-09-29

## Context

A repo can grant its Claude Code sessions a directory next to it, in its
committed `.claude/settings.json`:

```json
"permissions": { "additionalDirectories": ["../sibling-repo/"] }
```

Claude Code resolves a relative entry against its working directory. In the
user's clone, that is the sibling the repo's authors meant. A Lucidos
coding-agent session runs in a worktree under the workspace. There the entry
names a directory that does not exist, and the grant silently does nothing. A
team's plan-publishing workflow hit this: its sessions could not reach the
sibling knowledge repo they were told to write to.

## Decision

At spawn, the engine reads the worktree's `.claude/settings.json`. It takes
each relative `additionalDirectories` entry that climbs out of the repo and
resolves it against the main checkout. Each result goes to Claude Code as
`--add-dir`. The code is `engine/repo_directory_grants.rs`, and the plan is
`docs/plans/2026-09-29-repo-directory-grants-in-worktrees.md`.

## Rationale

- **Parity, not a new privilege.** The same entry grants the same directory
  when the user runs `claude` in their clone. Lucidos restores that.
- **The repo stays the source of truth.** Nothing new to configure in Lucidos.
  Adding or removing a directory is a change to the repo, reviewed like any
  other.
- **Only entries that leave the repo move.** An entry inside the repo
  (`docs/`) already resolves to the worktree's own copy, which is correct.
  Re-anchoring it would point the agent at the user's working tree.
- **The main checkout comes from the worktree itself** (`git rev-parse
  --git-common-dir`). The session and its side-question copy compute the same
  flags from the same working directory, which keeps the prompt cache warm.

## Consequences

- A grant never sits inside, or contains, the main checkout, the repo's shared
  `.git`, the worktree or the workspace. So a relative entry that climbs to
  `..`, `/` or the home directory is refused and logged. Absolute and `~/`
  entries are left to Claude Code, which resolves them the same from anywhere.
- The main checkout is what `git worktree list` names first, which holds for a
  submodule too. Git records none for a `--separate-git-dir` clone asked from a
  linked worktree, so such a repo gets no grants.
- Anything unknown grants nothing and never fails the spawn: a failed git
  probe, an unreadable settings file, a target that does not exist.
- `.claude/settings.local.json` is not read. It is gitignored, so a worktree
  never has one.
- Codex does not read Claude Code settings, so nothing changes for it.

## Alternatives considered

- **Claude Code's hidden `--project-config-root <dir>` flag.** It exists for
  exactly this host shape, but it moves every project config read to the main
  checkout: settings, hooks, skills, agents and `.mcp.json`. A branch that
  edits its own hooks or skills would then run the main checkout's copy.
  Claude Code also refuses background sessions under it. Too wide for a
  problem about one setting.
- **A per-repo directory list in Lucidos.** A second place to maintain the same
  fact, which drifts from the repo's own settings and helps nobody outside
  Lucidos.
- **Place worktrees next to the repo**, so `../` means the same thing. Worktrees
  live in the workspace by design: the engine owns them there, cleans them up,
  and keeps them off the user's projects directory.
- **Leave it to each repo** to write paths that survive a worktree. The repo
  cannot: an absolute path differs per machine, and the settings file has no
  variable for the main checkout.
