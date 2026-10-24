# 0564: Codex command cards persist Claude Code's narrow Bash line; unchanged tracked repo scripts reach the judge

- **Status**: Accepted
- **Date**: 2026-10-23

## Context

A Codex thread carded `/bin/zsh -lc 'lucidos hardened expected-tier && ./scripts/merge-drivers.sh && git merge main --no-edit'`.
The workspace had the command guard, its judge and its coding-agent coverage all on.
The judge never ran. `head_safety` refuses any path-qualified head, and a
refusal skips the judge and every grant. The user also had no durable grant
for Codex: `derive_allow_pattern` returned `None` for every Codex tool at both
persisted scopes, so the card offered only "Allow once" and "Allow for this
thread".

## Decision

1. **A trusted repo script is judged, never refused and never granted.** A
   path head is trusted when it resolves, symlinks followed, to a tracked file
   inside the session's own worktree. Its bytes must also hash to the blob at
   the same path on the default branch (`refs/heads/<branch>`). Trust moves the
   head from a refusal to an unrecognised head, like `make`. The judge then
   decides it. It never settles `Safe` alone, and no grant covers it.
2. **Trust has limits.** A relative head needs an absolute request `cwd`.
   Codex sends one, Claude Code's `Bash` does not. A line with any directory
   change (`cd`, `pushd`, `popd`, `chdir`, `source`, a quoted form) drops
   relative trust. Any git or disk failure reads as untrusted. The unattended
   lane never reads trust.
3. **A Codex command card persists Claude Code's narrow line.** "Always allow
   `git` commands" writes `Bash(git:*)` to `cc-allowed-tools`, the same line a
   Claude Code card writes. The engine's persisted gate honours it for a Codex
   `command_execution`, per segment through `grant_covers_command`. Codex
   gets no Broad button, and a bare `Bash` line never covers a Codex command.

## Rationale

The refusal exists because an agent can write a script and run it in one line.
A tracked file whose bytes match the default branch was not written by this
session, so the reason does not apply. It is still arbitrary code, so it gets
the same treatment as any unknown program: the judge. A grant was the first
design, matched by basename. It lost because the line itself can rewrite the
script in an earlier segment, or swap a symlink, before the head runs.

One line per head keeps one meaning in one file: "`git` commands in this
workspace", whichever agent asks. Codex asks only when a command leaves its OS
sandbox, so a blanket grant would approve exactly the commands worth seeing.

## Consequences

- Repo scripts the user already merged stop carding in a Codex thread when the
  judge calls them safe.
- A user who grants `git` on a Codex card also stops Claude Code carding `git`.
- No grant covers a catastrophic command. `grant_covers_command` checks the
  hard-block first, so a `Bash(rm:*)` line never answers for `rm -rf /`.
- Claude Code's own buttons and its `.claude/` / `.git/` protected-path rule
  are unchanged (ADR 0125).
- A changed or new script still cards every time until it lands on the
  default branch.

## Alternatives considered

- **Trust any path inside the worktree.** Rejected: that is the write-then-run
  case the refusal exists for.
- **Let a grant cover a trusted script by basename.** Rejected after review: a
  rewriting earlier segment or a symlink lets the line ride an existing grant.
- **A separate `command_execution(<head>:*)` persisted line.** Rejected: two
  lines for one meaning, and no Codex driver reads the file, so only the
  engine's gate would know it.
- **A bare "Always allow" on Codex cards.** Rejected: too wide for commands
  that already escaped a sandbox.
