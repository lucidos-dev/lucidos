# 0684: Codex side questions use ephemeral app-server forks

- **Status**: Accepted
- **Date**: 2026-11-14

## Context

ADR 0318 refused Codex side questions because its protocols offered no suitable
call. Codex now supports ephemeral `thread/fork`. A real CLI probe forked idle and active sessions through a separate app-server.
Neither fork persisted the side question or interrupted the main turn.

## Decision

Ask a short-lived app-server's ephemeral fork, with `excludeTurns: true`,
read-only sandboxing and a synchronous catch-all PreToolUse deny hook.
Disable shell, hosted search, image generation, browser, computer, connector,
plugin and MCP tool sources. Scope hook trust to the exact normalized declaration
in session flags. Verify its command, matcher, enabled state, synchronous execution
and trusted or managed status through `hooks/list`, rejecting load errors.

Decline approvals and interrupt tool attempts. Retry declined commands and patches with the
shared refusal up to the turn cap. Never retry after a tool completed.
This path always uses app-server, including for main sessions using exec.

## Rationale

An ephemeral fork reads the saved context without appending to the main
rollout. `excludeTurns` avoids hydrating response history and is required by
the CLI for an ephemeral paginated fork; it does not remove model context.
The outer Lucidos sandbox remains in force. The driver shares the deadline,
refusal text, cost capture, and process-group cleanup contract with other
side-question implementations.

## Consequences

Codex threads with a saved session can answer side questions while their main
turn runs. Images use local blob paths. Usage is recorded under Side Question
at the resolved model. An unsupported CLI fails with update instructions;
there is no persistent or interrupt-only fallback. Forks may miss the main session's prompt cache.
This supersedes only ADR 0318's Codex refusal, not its later recording decisions.

An interrupt alone is insufficient: read-only commands can finish before it
arrives, even with `approvalPolicy: never`. Hosted tools can run on the provider
before any local hook. The hook and disabled sources protect both boundaries.

## Alternatives considered

- `codex exec resume`: rejected because it appends to the real rollout.
- Asking the live app-server: rejected because it couples the side question to
  the main turn and cannot supply the same isolation guarantee.
- A fresh contextless session: rejected because it cannot answer from the
  thread's saved conversation.
