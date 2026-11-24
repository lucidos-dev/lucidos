# 0795: Engine code drives coding agents only through the `AgentRuntime` trait

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0004](0004-codex-as-second-coding-agent.md), [0005](0005-codex-app-server-protocol.md), [0371](0371-sub-agent-steps-fold-under-their-agent-row.md), [0487](0487-coding-agent-start-failures-fail-loudly.md)

## Context

Lucidos drives CLI coding agents such as Claude Code and Codex. Each CLI has
its own output format, process handling and permission protocol.

The engine needs one way to run a session whatever the agent is.

This record restates the design comments of
`crates/lucidos-engine/src/runtime/agent_runtime.rs`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- `AgentRuntime` wraps a CLI coding agent behind a channel-based interface.
  Engine code that drives a session uses only this trait.
- Each implementor parses its CLI's stdout into the canonical `AgentEvent`
  enum. It accepts user input on `AgentInput` and control requests on
  `ControlRequest`. It watches a `CancellationToken` to know when to kill the
  process.
- Agent-specific JSON formats and process management stay inside the
  `runtime::*` modules.
- A sub-agent's prose and a session's progress note are separate event kinds.
  Neither is folded into `Message`.
- A driver reports how the agent's process ended as a fact and never judges it.
  Whether it was a start failure, a crash or a clean close is decided once, in
  `agent_session::lifecycle`.
- Permission requests reach the engine by one of two routes, depending on the
  agent's protocol.

## Rationale

- A whole block of prose from a Claude Code sub-agent is that sub-agent's
  narration. It is named by the `Agent` call that spawned it and is never the
  session's reply, so it does not share `Message`.
- A progress note is the provider's summary of text the agent wrote before a
  tool call. The user read only the note, while the agent read its whole text.
  That difference keeps it apart from `Message`.
- Drivers report facts and the verdict is made in one place. This keeps the
  start failure, crash and clean close decision out of every driver.
- Codex raises permission requests in its own protocol. These are the
  `item/commandExecution/requestApproval` and `item/fileChange/requestApproval`
  JSON-RPC requests. Claude Code raises its prompts out of band over HTTP.

## Consequences

- The engine run loop consumes Codex requests from
  `RunningAgent::permission_rx`. It reuses the same
  `CodingAgentPermissionRequest`, PermissionCard and broadcast machinery that
  the Claude Code MCP HTTP path uses.
- The engine answers through `respond`, where `true` accepts and `false`
  declines. The driver then replies to the JSON-RPC request.
- Claude Code prompts arrive through `lucidos mcp-permission-server` and
  `/api/v1/internal/permission-prompt`. Its `RunningAgent` therefore carries
  `permission_rx: None`.
- Narration, progress notes and replies stay distinguishable in the event
  stream.
- A new driver only has to emit facts and canonical events. It does not decide
  how a process ending is classified.
