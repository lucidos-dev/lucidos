# 0814: Wait for the engine API listener before spawning coding-agent sessions

- **Status**: Accepted
- **Date**: 2026-11-25

## Context

Engine recovery queued coding-agent sessions before the API listener bound.
Their Bash hooks could not verify standing denies and refused the commands.
The sessions ended their turns even though the engine became reachable later.

## Decision

The engine holds the session spawn gate before recovery.
It opens the gate only after axum reports that all configured TCP listeners are listening.
Both Claude Code and Codex pass through the gate before starting their process.

## Rationale

The hook's standing deny check needs the engine API before the agent can run its first Bash command.
Startup can enqueue session work immediately because the wait lives in the spawn task.
The engine can finish recovery and start serving independently.

## Consequences

The gate starts open for callers that construct the engine without the binary's startup sequence.
A bind failure ends startup with the gate still held.
The hooks retry connection refusals for up to three seconds and fail closed if verification fails.

## Alternatives considered

- **Bind before recovery.** Recovery assumes the engine is serving no requests.
  A bound socket without an accept loop would turn connection refusals into request timeouts.
- **Extend the hook retry alone.** In observed restarts, the gap between session launch and listening exceeded 30 seconds.
  A retry covering that gap would exceed the hook timeout.
