---
name: Workspace audit: scripts
description: The scripts audit section: CLI usage for data writes, events and API calls, and hardcoded machine paths.
---

# Workspace audit section: scripts

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

Append these lines to the root's scan preamble, before its receipts tail:

```bash
scan machine-path "/(Users|home)/[^\"'[:space:]]+" $all
```

## Checks

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

Per `system-knowhow/lucidos-cli.md`:

- Writes to `data/` go through `lucidos data write`, not raw HTTP and not open-coded paths under `$LUCIDOS_WORKSPACE/data/`. Do not flag a script's own runtime state (a cursor, a last-seen id) written directly under `data/artifacts/<plugin-id>/` or `data/triggers/<slug>/state/`. That is the intended pattern, per `system-knowhow/plugins.md` § "Where a plugin keeps its runtime state".
- Domain events go through `lucidos events emit` / `lucidos events query`.
- External API calls go through `lucidos proxy <name>` when the workspace owns a credential for the service. The patterns are the `cross-cutting` section's. A script adds one consequence: a credential in argv also lands in shell history.
- No hardcoded absolute paths to a specific workspace.

Per `system-knowhow/best-practices.md`:

- Script lives with its sole consumer (single-app script in `apps/<id>/scripts/`, not shared `data/scripts/`).
