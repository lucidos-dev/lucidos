# 0818: Relay coding-agent sign-in through the thread's CLI profile

- **Status**: Accepted
- **Date**: 2026-11-26
- **Amended by**:
  - [0820](0820-thread-terminal-for-every-thread.md): sign-in becomes a terminal suggestion. The helper and paste form retire.
  - [0823](0823-mcp-sign-in-on-tool-call.md): a Claude Code MCP sign-in waits for a tool call on the server. Codex posts on its report.

Amends the coding-agent exclusion in [ADR 0622](0622-gateway-owns-oauth-callback.md).
Plan: `docs/plans/2026-11-26-coding-agent-sign-in-from-the-thread.md`.

## Context

The CLI owns the login and its credential store. An MCP server can require
sign-in after a coding-agent thread has started. The provider can redirect to
a loopback listener on the CLI host even when the browser is on another device.

## Decision

Handle Claude Code and Codex MCP sign-in, and direct Claude subscription and
Console sign-in, with a form request in the thread. Claude gateway trust stays
interactive in its own CLI. Codex model login stays in the Codex CLI.

Launch a helper with the thread's CLI executable, working directory, sandbox
and account environment. Use the live or remembered launch context. On a
Claude cache miss, recover only with a recorded backend session and Claude
account pin. On a Codex cache miss, require a regular turn to reestablish the
launch context because a saved session ID alone cannot prove `CODEX_HOME`.

Use a PTY for Claude MCP because the CLI refuses the flow over pipes. Allocate
it with rustix and set close-on-exec atomically. Disable echo and canonical
line buffering so long redirect addresses reach the CLI intact. Direct
Claude login and Codex MCP use pipes.

The owner starts sign-in. Use the browser locality verified at ingress
according to ADR 0622. A remote relay or an unstamped browser gets the paste
field immediately. A local browser can use CLI loopback where offered, with
paste as a fallback. Hosted code entry uses paste from the start.

Accept a paste only for a live request and from the device that opened it. Relay the address or code to the
helper's stdin and keep only the authorization URL, structured CLI status
and request metadata. The CLI exchanges and stores credentials.

## Rationale

The provider's client registration, redirect and credential storage belong to
the CLI. Reusing the thread's profile puts the login where its next subprocess
can find it. The helper runs alongside the turn, so a successful sign-in
refreshes the subprocess before its next turn without interrupting a running
turn. The gateway still owns the callback for Lucidos account connections.

## Consequences

The durable form request survives a client reconnect. An engine restart
expires a request whose helper is gone. A typed `/mcp` shows the last full CLI
status report. `/login` opens the supported flow or explains the CLI steps.
Both are transcript commands without a model turn.

The CLI must offer a supported sign-in flow. Loopback and Keychain behavior on
macOS require verification with an isolated account or VM.

## Alternatives considered

**Forward the CLI's random port.** Requires host access and a tunnel from the
browser's device for each flow.

**Exchange credentials in Lucidos.** Would require Lucidos to implement the
CLI's provider and profile semantics and handle the returned tokens.
