---
name: Workspace audit: prompt footprint
description: The prompt-footprint audit section. Sections over their ceilings, clipped descriptions, unused apps and widgets, knowhow not loaded by name, and triggers listing an MCP server they never call.
---

# Workspace audit section: prompt-footprint

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

It reads the *workspace prompt footprint*: what this workspace's own content
adds to every chat turn. The engine measures it with the builders a turn uses,
so never recompute a size yourself.

## Scan

Run this once, on its own, beside the root's grep scan:

```bash
lucidos workspace-prompt-footprint show > "${TMPDIR:-/tmp}/prompt-footprint.json"
jq -r '.sections[] | "| \(.id) | \(.chars) | \(.over_ceiling) |"' "${TMPDIR:-/tmp}/prompt-footprint.json"
```

Add one receipts row, `prompt-footprint`, with the number of sections the
report listed. Paste a table of every section into the report: its id, its
chars, the section ceiling, and whether it is over. Below it, give the total
against the total ceiling, and the *system prompt footprint* for scale.

The three numbers come from the user's *preference*s:
`workspace_prompt_footprint_section_ceiling`,
`workspace_prompt_footprint_total_ceiling` and
`workspace_prompt_footprint_unused_days`. The JSON carries the values in force.
Quote them from there, never from memory.

## Checks

For each finding, capture: **location** (the section id, then the item id),
**what's wrong**, **which reference owns the rule**, **suggested fix**.

- **A section over its ceiling.** `over_ceiling` is true. Name the section and
  its three largest items by `chars`. Severity: **smell**, since the turn still
  carries the whole section. The suggested fix depends on the section's items:
  - Some are clipped, unused or not loaded by name: the fix is theirs, as
    § Remediation says.
  - None are: its items are in use, and no item fix can clear it. The fix
    names the setting instead. See "A ceiling the items cannot clear" below.
- **The total over its ceiling.** `over_total_ceiling` is true. Name the two
  largest sections. Severity: **smell**. Choose the fix as for a section, but
  look at the items of every section. When none is clipped, unused or not
  loaded by name, the fix names the setting.
- **The MCP tool schemas are never part of the total.** `mcp-tool-schemas` has
  `when: mcp-enabled`: only a thread that enables a server carries its tools
  (ADR 0430). Its `chars` is the worst case, every server enabled in one
  thread. Never name it in the total finding. Each of its items is one server,
  with `mcp_threads.enabled` and `mcp_threads.called` over the window.
- **A trigger that lists an MCP server its fires never call.** A listed server
  rides every request of every fire. Take each live trigger's `mcp_servers`
  from `lucidos triggers list`. For each listed server, count the trigger's
  threads that called one of its tools in the window:

  ```bash
  psql "$LUCIDOS_DATABASE_URL" -At -c "SELECT count(DISTINCT e.thread_id) FROM events e JOIN thread_summaries ts ON ts.thread_id = e.thread_id WHERE ts.trigger_id = '<trigger id>' AND e.event_type = 'ToolCalled' AND e.payload->>'name' LIKE 'mcp\_\_<server id>\_\_%' AND e.created >= now() - make_interval(days => <unused_days>)"
  ```

  Report a server at zero when the trigger fired in the window. Severity:
  **nit**.
- **A ceiling the items cannot clear.** The ceiling is a *preference* the user
  can raise. Write the suggested fix with these four parts:
  - the key: `workspace_prompt_footprint_section_ceiling` for a section,
    `workspace_prompt_footprint_total_ceiling` for the total
  - the value in force: `section_ceiling` or `total_ceiling` from the JSON
  - a value with headroom: today's `chars` (or `total_chars`) plus a quarter,
    rounded up to the next thousand. For 20,900 chars, suggest 27,000.
  - that the choice is the user's: "Raise it if this size is what you want."
- **A clipped description.** An item with `clipped_chars` above 0. The prompt
  cuts the description, so the agent routes on a part the author did not
  choose. Severity: **stale**. Recommend a shorter description that keeps the
  routing words.
- **An unused item.** An app or reusable widget whose `usage.verdict` is
  `unused` for the whole window: nobody opened the app, and no reply embedded
  the widget and nothing pinned it. Give its `last_used_days_ago`, or "never".
  Severity: **nit**.
- **A knowhow doc not loaded by name.** `usage.verdict` is
  `not-loaded-by-name`: no `load_knowhow`, `read_file` or
  `lucidos knowhow read` reached it in the window. Never call it unused. A
  read through the shell leaves no trace, so it may still be in use.
  Severity: **nit**.
- **`not-yet-judged` is not a finding.** The evidence does not cover the window
  yet. Report the count in one line, so nobody reads silence as "all used".
  Deleting a thread deletes the loads, embeds and pins it held, so a
  deletion inside the window leaves knowhow and reusable widgets not judged.
- **A knowhow doc an intent loads counts as used.** `load_knowhow` on an
  intent reads its `knowhow:` docs, and an app intent's whole app folder, but
  records a load for the intent id only.

**The largest items go in the report, but they are not findings.** List the
five largest across every section by `chars`, so the user sees where the
characters go.

## Remediation

Fixes run only on request, as the root's § Remediation says. Every fix here
is inline: a file edit, a deletion, a CLI call or an `update_trigger` call.
None needs a coding agent. The fixes split by whether they can be undone.

| Finding | Fix | In "Do all suggested fixes" |
|---|---|---|
| a clipped app or reusable widget description | rewrite the `description` in its `manifest.json`, keeping the words a request would use | yes |
| a clipped knowhow description | rewrite the frontmatter `description`, under the clip, keeping the routing words | yes |
| an unused reusable widget | stop reusing it: `lucidos widgets stop-reusing --app-id <id>` | yes |
| a knowhow doc not loaded by name | move it under a group doc as a reference, named by id from that doc | yes |
| a trigger listing an MCP server its fires never call | remove the id from its `mcp_servers` with `update_trigger`, sending the rest of the list | yes |
| an unused app | delete it, behind its own card | no |
| a ceiling the items cannot clear | raise the ceiling, only when the user picks its own card option | no |

- **Never delete in bulk.** A deletion is never part of "Do all suggested
  fixes". Ask one card per item, naming the item and when it was last used.
- **Never offer to delete a knowhow doc.** Its verdict cannot prove it
  unused. Delete one only when the user names it and asks.
- **Never raise a ceiling on your own.** Never change any of the three
  preferences to clear a finding. A finding the user accepts stays in the
  report.
- **Offer a raise as its own card option, after the report.** Offer it only for
  a ceiling the items cannot clear. Name the key, the value in force and the
  suggested value. It never joins "Do all suggested fixes". When the user picks
  it, call `set_preference` with that key and value.
- **When some items have fixes, offer those and no raise.** Shorten, stop
  reusing, move or delete them, as above.
