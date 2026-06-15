# 0005 — Codex defaults to the `codex app-server` protocol; `codex exec` stays as the escape hatch

- **Status:** Accepted
- **Date:** 2026-06-12

## Context

ADR 0004 integrated Codex as per-turn `codex exec --json` processes —
deliberately the cheapest second mover. That protocol cannot do three things
the engine's Claude Code integration treats as table stakes:

1. **Permission cards.** `codex exec` runs `approvalPolicy: never`; a command
   that needs to escalate past the sandbox fails silently instead of asking.
2. **Per-token streaming.** Items arrive whole (`item.completed`), so Codex
   threads render chunky.
3. **Graceful interrupt.** Cancel kills the per-turn child; partial work is
   lost and the turn closes with a synthesized Result.

`codex app-server` (verified against codex-cli 0.139.0; schema via
`codex app-server generate-json-schema`) is a persistent child speaking
line-delimited JSON-RPC over stdio: `thread/start` / `thread/resume`,
`turn/start` per input, `item/agentMessage/delta` streaming notifications,
`turn/interrupt`, and server→client approval requests
(`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`)
under `approvalPolicy: on-request`. It is upstream-experimental and shaped
for IDE integrations; its surface has moved between codex releases.

## Decision

1. **Two drivers behind one `CodexRuntime`, selected by
   `LUCIDOS_CODEX_PROTOCOL`.** Default **`app-server`**
   (`runtime/codex_app_server.rs`); `exec` (`runtime/codex.rs`) stays fully
   wired as the escape hatch. Both share `CodexConfig`, the sandbox profile
   (`workspace-write`, network on, shared git dir writable), the `lucidos`
   MCP server (`ask_user_question`), and the Codex thread id — the two
   protocols read the same on-disk rollout, so an existing thread resumes
   fine after a flip.
2. **Approvals bridge onto the existing permission machinery.**
   `RunningAgent` gains `permission_rx:
   Option<mpsc::UnboundedReceiver<AgentPermissionRequest>>`; a select arm in
   `run_session/run.rs` spawns one waiter task per request (the loop never
   blocks on the user) and drives the same dedup / session-allow / emit /
   broadcast core the CC MCP HTTP path uses — extracted to
   `cc_permission::prompt_coding_agent_permission`, called by both paths.
   The user's PermissionCard click resolves the broadcast; the driver
   replies `accept` / `decline` to the JSON-RPC request. CC and the Codex
   exec driver carry `permission_rx: None`.
3. **Approval policy flips to `on-request` (app-server only).** Sandbox
   escalations raise a card instead of failing silently. The exec escape
   hatch keeps `never` + sandbox-as-guard (ADR 0004 §4).
4. **Interrupt rides the protocol.** `ControlRequest::Interrupt` →
   `turn/interrupt {threadId, turnId}`; the turn ends with
   `turn/completed {status: interrupted}` and partial work survives. The
   engine's 8s `INTERRUPT_ESCALATE_AFTER` still hard-kills via the
   cancellation token when codex ignores the request.
5. **Streaming needs no engine change.** `item/agentMessage/delta` maps onto
   `AgentEvent::Message`; the engine's buffer/flush loop already tolerates
   arbitrary chunk sizes. The `item/completed` full-text echo emits only the
   un-streamed remainder so text never duplicates.

## Rationale

The app-server protocol is the only Codex surface that can host the
permission flow — and the permission flow is what makes `on-request`
approval safe to enable, which in turn is what lets a Codex session do
things the sandbox would otherwise silently block. Streaming and graceful
interrupt come along for free on the same protocol. Keeping exec wired (not
deleted) bounds the risk of betting on an experimental upstream surface: one
env var rolls a workspace back to the shipped-and-stable model.

## Consequences

- Codex threads get PermissionCards (`tool_name: command_execution` /
  `file_change`), per-token streaming, and cancel-preserves-partial-work.
- A codex upgrade that breaks the app-server contract is mitigated three
  ways: the negotiated `initialize` response is logged, the protocol mapper
  is pinned by stub tests, and `LUCIDOS_CODEX_PROTOCOL=exec` restores the
  per-turn model without a rebuild.
- The engine still terminates the agent subprocess at idle
  (`IdleAction::ExitSubprocess`) — "persistent" means within a turn-set, not
  across idle gaps; resume goes through `thread/resume`.

## Alternatives considered

- **Stay on exec and accept the gaps** — rejected: silent sandbox failures
  are the worst of the three gaps; users see a Codex turn "finish" without
  doing what it was asked, with no signal that an approval was needed.
- **Delete the exec driver once app-server lands** — rejected: app-server is
  explicitly experimental upstream; betting the only Codex path on it
  removes the rollback when (not if) its shape moves.
- **Drive approvals through the MCP permission server (like CC)** —
  rejected: codex has no `--permission-prompt-tool` designation; its
  approvals are first-class JSON-RPC requests on the app-server protocol,
  so the bridge is the natural seam and reuses the engine machinery anyway.
