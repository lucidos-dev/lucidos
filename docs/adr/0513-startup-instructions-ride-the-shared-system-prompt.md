# 0513: Claude Code startup instructions ride the shared system prompt, verified against Claude Code's own record

- **Status**: Accepted
- **Date**: 2026-10-17

## Context

ADR 0491 made the appended system prompt identical across the sessions of one
workspace. A fresh session still wrote about 26k tokens to the prompt cache at
its start. Claude Code puts CLAUDE.md, the unscoped rules, env and skills into
one first user message, with a cache marker only at its end. The CLAUDE.md
header names the worktree path, so every worktree rewrites the whole message.
anthropics/claude-code#93499 reports the same mechanism.

The measurements, on Claude Code 2.1.300 with a live appended prompt:

| Arm | Read | Written |
|---|---|---|
| Today | 24,290 | 26,330 |
| `--exclude-dynamic-system-prompt-sections` | 24,179 | 26,320 |
| Startup instructions in the append | 43,031 | 7,207 |

The full setup is in
`docs/plans/2026-10-17-claude-code-startup-instructions-ride-the-shared-prompt.md`.

## Decision

For a Claude Code session, Lucidos moves the startup instructions into the
system prompt, after the shared prompt. It does so only when it has verified
that its copy of Claude Code's loading matches Claude Code's own record. Any
other session runs as before.

## Rationale

The system tier is the only place the cache shares across worktrees. A Rust
mirror of the loading (user file, the worktree and its parents, unscoped rules,
imports, comment stripping, trimming) can fill it. A mirror drifts when Claude
Code changes, so it is never trusted alone.

Every Claude Code transcript records an `instructions` attachment: the files it
loaded, in order, with their content. The engine compares that record with the
mirror after the first turn of a session that kept Claude Code's loading.

- The **mirror key** hashes the Claude Code version with the mirror's output,
  paths taken relative. A match marks the key verified. A mismatch marks it
  failed and logs the first differing entry.
- Only a verified key moves. A new version or a changed file gives a new key, so
  it re-verifies before it moves.
- The ledger is a cache under `.lucidos/`. Losing it costs one unshared session
  per key.

A moved session lists every mirrored file in `claudeMdExcludes`, in a
per-session settings file. The block labels each file relative to the worktree
or config directory, so two worktrees with the same files render the same bytes.

## Consequences

- Fresh sessions after the first read about 43k tokens and write about 7k.
- A fresh spawn decides from the ledger. A resumed spawn starts the way its
  session began. Claude Code freezes the system prompt (ADR 0491), so it never
  reads a block appended on resume.
- The ledger records each moved session, and a recorded one moves again. Any
  other resumed session keeps Claude Code's loading, so no instruction is lost.
  A lost ledger entry only loads a moved session's files twice.
- Side questions resume their session, so they follow the same rule.
- A worktree path with glob syntax never moves, since `claudeMdExcludes` holds
  glob patterns and a misread pattern would load the files twice.
- A move that cannot write its settings file keeps Claude Code's loading.
- Only a session with a Lucidos system prompt moves. A model probe never does.
- Instructions arrive with system-prompt authority instead of user-message
  authority.
- The measure is temporary: it ends when Claude Code serves CLAUDE.md from the
  cache across sessions itself (`docs/temporary-measures.md`).

## Alternatives considered

**`--exclude-dynamic-system-prompt-sections`.** Rejected: measured, it buys
nothing. Claude Code already keeps cwd and git status out of the system tier in
this setup.

**Trust the mirror without verification.** Rejected: a Claude Code release that
changes the loading would then silently drop or double an instruction. The
transcript check makes the mirror fail closed.

**Move the files for every session from the first.** Rejected: the first session
of a key has nothing verified to rest on. It costs one unshared session.

See ADR 0491 for the system tier this builds on.
