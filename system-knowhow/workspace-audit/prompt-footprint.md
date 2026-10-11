---
name: Workspace audit: prompt footprint
description: The prompt-footprint audit section: workspace-grown prompt sections over their ceilings, clipped descriptions, unused apps and reusable widgets, and knowhow docs not loaded by name.
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
lucidos workspace-prompt-footprint show > /tmp/prompt-footprint.json
jq -r '.sections[] | "| \(.id) | \(.chars) | \(.over_ceiling) |"' /tmp/prompt-footprint.json
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
  carries the whole section.
- **The total over its ceiling.** `over_total_ceiling` is true. Name the two
  largest sections. Severity: **smell**.
- **A clipped description.** An item with `clipped_chars` above 0. The prompt
  cuts the description, so the agent routes on a part the author did not
  choose. Severity: **stale**. Recommend a shorter description that keeps the
  routing words.
- **An unused item.** An app or reusable widget whose `usage.verdict` is
  `unused`: nothing opened or showed it for the whole window. Give its
  `last_used_days_ago`, or "never". Severity: **nit**.
- **A knowhow doc not loaded by name.** `usage.verdict` is
  `not-loaded-by-name`: no `load_knowhow`, `read_file` or
  `lucidos knowhow read` reached it in the window. Never call it unused. A
  read through the shell leaves no trace, so it may still be in use.
  Severity: **nit**.
- **`not-yet-judged` is not a finding.** The evidence does not cover the window
  yet. Report the count in one line, so nobody reads silence as "all used".
  Deleting a thread deletes the loads and shows it held, so a deletion inside
  the window leaves knowhow and reusable widgets not judged.
- **A knowhow doc an intent loads counts as used.** `execute_intent` reads an
  intent's `knowhow:` docs, and an app intent's whole app folder, with no
  `load_knowhow` call to record.

**The largest items go in the report, but they are not findings.** List the
five largest across every section by `chars`, so the user sees where the
characters go.

## Remediation

Fixes run only on request, as the root's § Remediation says. The fixes split
by whether they can be undone.

| Finding | Fix | In "Do all suggested fixes" |
|---|---|---|
| a clipped app or reusable widget description | rewrite the `description` in its `manifest.json`, keeping the words a request would use | yes |
| a clipped knowhow description | rewrite the frontmatter `description`, under the clip, keeping the routing words | yes |
| an unused reusable widget | stop reusing it: `lucidos widgets stop-reusing --app-id <id>` | yes |
| a knowhow doc not loaded by name | move it under a group doc as a reference, named by id from that doc | yes |
| an unused app | delete it, behind its own card | no |

- **Never delete in bulk.** A deletion is never part of "Do all suggested
  fixes". Ask one card per item, naming the item and when it was last used.
- **Never offer to delete a knowhow doc.** Its verdict cannot prove it
  unused. Delete one only when the user names it and asks.
- **Never raise a ceiling to clear a finding.** The three preferences change
  only when the user says so in their own words. A finding the user accepts
  stays in the report.
- **A section over its ceiling has no fix of its own.** Its items do: shorten,
  stop reusing, move or delete them, as above.
