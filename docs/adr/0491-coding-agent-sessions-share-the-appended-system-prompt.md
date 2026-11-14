# 0491: Coding-agent sessions share the appended system prompt; per-session values ride the first message

- **Status**: Accepted
- **Date**: 2026-10-16

## Context

ADR 0084 keeps the chat system block free of per-turn and per-thread values, so
threads in one workspace share its cache entry. The coding-agent prompt had no
such rule. Lucidos appends it with `--append-system-prompt-file`. It carried the
session's branch, its file-link example and the external base ref. It also
carried the wake duty, the integrated parent, the owner note and the whole
thread history.

A probe on Claude Code 2.1.300 measured the cost. Claude Code keeps the append
inside a cached dynamic block of about 12.7k tokens. Changing only the branch
name inside the append dropped a warm session's read from 25,416 to 12,691
tokens. Claude Code's own per-session bytes (working directory, git status,
CLAUDE.md) ride the first user message, not the system tier.

A second finding shaped the fix. Claude Code freezes the system prompt at
session start: a `--resume` with a changed append reads the old one. So a
per-turn value rebuilt into the append reached the model on the first turn only.
The owner note and the history were already stale after turn one.

The full evidence and the baseline sit in
`docs/plans/2026-10-16-claude-code-sessions-share-the-system-tier.md`.

## Decision

The appended system prompt is a function of workspace, session flavor, coding
agent and build only. Per-session values form a *session context block* that
leads the first user message of a session that starts fresh. Per-turn values
ride the message of each turn they apply to.

## Rationale

The cache keys on prefix bytes, so one per-session byte costs every session the
whole dynamic block. The user message is the only place a value can vary without
that cost, and Claude Code already puts its own per-session bytes there.

The runtime that starts the session delivers the block, not the engine. The
engine cannot know in time whether a resume will hold. The Codex app-server
falls back to a fresh thread only after the engine has queued the first input.
So `SpawnArgs` carries the block, and each runtime puts it ahead of the first
input of any session it starts fresh, including after a fallback.

A resumed session gets no block. Its transcript already holds the first message
that carried one, and the frozen system prompt would ignore a new one anyway.

## Consequences

- The branch sentence leaves the base flavors. They say "the branch your
  SESSION CONTEXT names", and the block names it.
- The history in the block holds the messages before the current one. It no
  longer repeats the message being sent.
- The owner note ("THE OWNER DID NOT OPEN THIS TURN") rides every turn it
  applies to, live follow-ups included. That fixes its staleness after turn one.
- A slash command goes verbatim, since any prefix would break it. The block
  waits for the first input that takes a prefix. A rejected turn leaves it owed.
- Codex gets the same split, since it reads the same `SpawnArgs.system_prompt`.
- The app flavor keeps its app id and manifest in the system prompt. Sessions of
  one app share; sessions of different apps do not.
- A test builds the prompt for two sessions that differ in every per-session
  value and asserts equal bytes. It breaks if such a value re-enters the prompt.

## Alternatives considered

**`RunningAgent` reports whether its spawn resumed, and the engine prepends.**
Rejected: the Codex app-server learns its resume failed after the first input
is already queued, so the report would come too late.

**Keep the branch in the prompt and move only the history.** Rejected: the probe
showed the branch alone costs the whole dynamic block.

**Rebuild the append on every resume.** Rejected: Claude Code ignores a changed
append on `--resume`, so it buys nothing.

**A placeholder branch in the file-link example.** Rejected: an agent pastes a
placeholder literally often enough to break links. The block carries the real
example instead.
