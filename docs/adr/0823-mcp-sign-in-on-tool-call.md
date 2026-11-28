# 0823: Claude Code MCP sign-in waits for a tool call on the server; Codex posts on its report

- **Status**: Accepted
- **Date**: 2026-11-27
- **Amends**: [0818](0818-coding-agent-sign-in-cards.md) (its plan's decision 3: a card once per session per target)
- **Plan**: `docs/plans/2026-11-27-mcp-sign-in-only-when-a-tool-needs-it.md`

## Context

ADR 0818's plan posted an MCP sign-in as soon as the CLI's status report named
a server `needs-auth`. ADR 0820 turned that card into a terminal suggestion but
kept the trigger. A user can configure many MCP servers, and most tasks touch
none of them. So nearly every coding-agent thread opened with a sign-in demand
for a server the task never used, and the card said nothing about why.

## Decision

For Claude Code, a status report alone posts no sign-in suggestion. The agent
may call a tool named `mcp__<server>__<tool>` on a server the latest report
marks as needing a sign-in. Then the engine posts one suggestion, once per session per server, and
its label says that a tool call needed the server. A typed `/mcp` still posts
one per server needing a sign-in, because the user asked.

Codex hides a signed-out server's tools and offers no pseudo-tool, so its model
can never make that call. For Codex, the status report itself posts one
suggestion, once per session per server, labelled like the `/mcp` one.

## Rationale

**A tool call is the proof that the task needs the server.** The status report
only says the server exists and is signed out. Both CLIs name MCP calls with
the same `mcp__<server>__` prefix, so a call before the first report counts
for either CLI.

**A card that names its reason is one the user can judge.** "A tool call needs
the X MCP server" tells the user why the card is there and whether it matters.

## Consequences

- A Claude Code thread whose task never uses a signed-out server shows no
  sign-in card.
- A turn can start before the first status report. A tool call made before
  that report waits for it, then posts if the report marks its server.
- When two server names share a prefix (`calendar`, `calendar__work`), the
  longest name that matches the call wins.
- A Codex thread still opens with a card for each signed-out server, as
  before this ADR. The owner chose that over a Codex user never seeing one
  unless they typed `/mcp`.
- The Claude model-login suggestion is unchanged: it already waits for a
  failed turn and a `loggedIn: false` status.

## Alternatives considered

**Keep the status-report trigger and dedupe per workspace.** Rejected: it still
asks first and explains never, and a workspace-wide dedupe hides the card from
the thread that does need it.

**Post only on `/mcp`.** Rejected: a task that does reach the server would then
fail with no way in, unless the user already knows the command.

**Parse a tool's error text for auth failures.** Rejected: text matching breaks
the 0818 rule that detection uses the CLI's structured report.
