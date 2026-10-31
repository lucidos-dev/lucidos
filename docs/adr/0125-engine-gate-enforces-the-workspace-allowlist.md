# 0125: The coding-agent gate enforces the workspace allowlist, bounded by derive_allow_pattern

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

A permission card offers three allow buttons. "Allow for this thread" bound
immediately. The two "Always allow" buttons did not. They append a pattern to
`<workspace>/.lucidos/cc-allowed-tools` (ADR 0095). That file reaches Claude Code
only as `--allowedTools`, read once at spawn and frozen for the process's life.
The engine's own pre-CC gate never read it.

So a click bound the NEXT coding-agent session. The user clicked, was asked again
seconds later, clicked again, and read the feature as broken. The chat lane never
had this problem: its guard reads `agent-allowed-commands` fresh on every prompt.

## Decision

`cc_permission::prompt_coding_agent_permission` reads `cc-allowed-tools` on each
prompt and answers from it, exactly as the chat lane does. A stored pattern
covers a request **only where `derive_allow_pattern` would have produced it for
that same request**, at `Broad` or `Narrow`. The gate sits BELOW the unattended
fast path, so it only ever answers for an interactive session.

## Rationale

The file is already workspace-wide and permanent. Reading it makes the engine
agree with it rather than lag it by one spawn, which is what the button says. It
adds no state: the filesystem stays the source of truth, and a removed line stops
being honoured on the very next prompt.

Binding the honour rule to `derive_allow_pattern` is what keeps this from being a
widening. The engine never becomes more permissive than the respawned subprocess.
`None` at both persisted scopes is the codebase's own record that Claude Code
ignores a pattern, and three cases fall out of that one rule:

- a bare `Edit` / `Write` / `NotebookEdit` / `ExitPlanMode` line covers nothing
  (`BROAD_ALLOW_INEFFECTIVE`), which is why the card hides those buttons;
- a `Bash` command touching `.claude/` or `.git/` is not covered, because CC
  keeps routing that shape through the prompt whatever the flag says;
- the Codex backend tools are not covered, since no Codex driver reads the file.

Matching is exact-string, against patterns the engine derives itself. A richer CC
pattern a user hand-wrote (`Bash(git diff:*)`, a wildcard) never matches, and the
card renders. The gate widens only through shapes its own derivation produces.

Placing it below the unattended gate is the security-load-bearing half. A
trigger-rooted session has no human, and ADR 0002 denies it a catastrophic
command regardless of any grant. A workspace grant a human clicked elsewhere must
not answer for a run nobody is watching.

## Consequences

- An "Always allow" click binds every live thread in the workspace at once, not
  just the clicking one, and not just after a respawn.
- Commands keep the per-segment rule through `command_guard::grant_covers_command`,
  so `Bash(git:*)` still refuses `git status && rm -rf /`.
- A bare `Bash` grant now covers every command in a live interactive session. That
  is what the button always meant, and what the next spawn already did.
- The gate reads a small file per carded request. It runs after the DB work the
  same request already does, and only where a card was about to render.
- `hydrate_session_allows` is untouched and stays session-only. A restart
  respawns the subprocess, which re-reads the file at both gates.

## Alternatives considered

**Seed the granted pattern into `PermissionState::session_allows` at click time.**
The shape the bug report suggested, and rejected on four counts.

It is a second recording path for state the file already owns. It binds only the
clicking thread. It needs a sync rule, or a deleted line stays honoured in
memory. And a broad `Bash` seeded there would reach CC's protected paths, which
the derivation rule above refuses.

**Honour every line in the file verbatim.** Rejected: a hand-written bare `Edit`
line would then auto-allow a `.git/` write, which CC itself cards and which never
appears in the diff reviewed before Apply.

**Let the gate answer for unattended sessions too.** Rejected: it would let a
workspace grant override `decide_unattended` and auto-allow a catastrophic
command in a trigger run.
