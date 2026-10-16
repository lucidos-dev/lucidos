---
name: Workspace audit: intents
description: The intents audit section: frontmatter, knowhow ids that resolve, and user-facing tone.
---

# Workspace audit section: intents

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

No grep decides this section. Its checks read files and events directly,
so it adds no row to the receipts.

## Checks

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

Scope is **every `.md` file the registry reads** (per `system-knowhow/intent-registry.md`): `apps/<id>/intents/`, `apps/<id>/triggers/`, and `triggers/<slug>/`. Trigger `.md` files are intents too. If an ID in the engine's "Available Intents" list has no file under `intents/`, look in the sibling `triggers/` directory before calling it a phantom.

- `name` present.
- `knowhow:` IDs in the frontmatter (if any) resolve to existing files. Severity **broken**. An ID is the path under `data/knowhow/` (or `system-knowhow/`) without `.md`, INCLUDING any subdirectory. The usual drift is a bare basename for a file in a subdirectory: `'nightly-pipeline-trigger'` for `data/knowhow/lucidos-ops/nightly-pipeline-trigger.md` (correct id: `lucidos-ops/nightly-pipeline-trigger`). Resolve each id against the full relative path under `data/knowhow/` (and `system-knowhow/` for prefixed ids).
- Reads in user terms, not engineer terms (same test as triggers).
- **Do not flag** a missing `data/triggers/<slug>/<slug>.md` for a *standalone scheduled trigger*. Its `run.intent` (in the `TriggerCreated` payload) is enough for scheduled firing. An on-disk procedure file is warranted only for dual use: scheduled firing **and** on-demand `execute_intent`. A pure scheduled orchestrator nothing calls manually is correct as-is.
