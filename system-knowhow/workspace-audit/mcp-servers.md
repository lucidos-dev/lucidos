---
name: Workspace audit: MCP servers
description: The MCP servers audit section. Registered servers whose launch line the user never approved, or whose start command the command guard's static check does not call safe.
sources: ["system-knowhow/running-python.md", "crates/lucidos-engine/src/engine/tools/mcp.rs", "crates/lucidos-engine/src/core/mcp_servers.rs", "crates/lucidos-engine/src/engine/command_permission.rs"]
---

# Workspace audit section: mcp-servers

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

An *MCP server* reruns its command line on every start. A server registered
before the command guard read that line was never checked. This section
finds those servers. `system-knowhow/running-python.md` § The command guard
owns the rule.

## Scan

Not a grep, a tool call. Call `mcp(action="list")` once, every pass. Add an
`mcp-servers` row to the receipts, with the number of servers it listed.

Each server carries one `Launch` line:

```
  Launch (<approved|not approved>, static check: <verdict>): <masked command line>
```

The line masks every env value. Quote it as printed, and never try to recover
a value. A server whose line reads `Launch: unknown` could not be checked.

## Checks

For each finding, capture: **location** (the server id), **what's wrong**,
**which reference owns the rule** (link, don't quote), **suggested fix**.

- **Static check `catastrophic, the guard refuses it`.** The command would
  destroy the machine if it ran. Severity: **broken**. Fix: remove the server.
- **Static check `may perform …`.** The command looks like an irreversible
  side effect, such as a mutating request or a cloud change. Severity:
  **smell**. Fix: ask the user to read the line and confirm they meant it.
- **`not approved`.** Nobody approved this exact line. With the guard on, its
  next start asks. With the guard off, it starts unchecked. Severity:
  **nit**. Fix: ask the user to read the line, then approve it by clicking
  Start under **Settings → MCP Servers**.
- **Static check `needs a judgment at start`** on an approved line is not a
  finding. The user already said yes to it.
- **`Launch: unknown`.** Report "could not check `<id>`'s launch line", never
  skip it. Severity: **smell** (the audit's coverage is incomplete).

A server can raise two findings, for example `not approved` and `may
perform …`. Report both under the one server.

## Remediation

Fixes run only on request, as the root's § Remediation says. None needs a
coding agent, and none joins "Do all suggested fixes": each one is the user's
judgment about a command.

| Finding | Fix |
|---|---|
| a catastrophic launch line | `mcp(action="remove", id="<id>")`, behind its own card naming the server and its line |
| a line that may perform a side effect | show the line, and ask the user whether to keep it |
| an unapproved line | show the line. The user approves it by clicking Start under **Settings → MCP Servers** |

- **Never approve a line for the user.** The approval records the user's yes.
  Only their click on Start or Enable, or a yes on the permission card, may
  record it.
- **Never edit a server's command to clear a finding.** Set it up again with
  `mcp(action="setup", ...)` only when the user asks. The guard then judges
  the new line.
