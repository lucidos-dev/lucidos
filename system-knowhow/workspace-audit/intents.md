---
name: Workspace audit: intents
description: The intents audit section: frontmatter, knowhow ids that resolve, intents shadowed by a same-id knowhow doc, and user-facing tone.
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
- **An intent shadowed by a same-id knowhow doc.** Severity **broken**.
  `load_knowhow(<id>)` runs an intent, but it resolves the id as knowhow
  first. So a doc with the same id wins, and the intent is never reached. Owns
  the rule: `system-knowhow/intent-registry.md`. For each intent id, look for
  a doc that loads under it:
  - `data/knowhow/<id>.md`, for any intent;
  - `data/apps/<app>/knowhow/<name>.md`, for an app intent `<app>/<name>`.

  Suggested fix: rename the doc or the intent, then update every reference to
  the renamed id.
- Reads in user terms, not engineer terms (same test as triggers).
- **Do not flag** a missing `data/triggers/<slug>/<slug>.md` for a *standalone scheduled trigger*. Its `run.intent` (in the `TriggerCreated` payload) is enough for scheduled firing. An on-disk procedure file is warranted only for dual use: scheduled firing **and** an on-demand run through `load_knowhow`. A pure scheduled orchestrator nothing calls manually is correct as-is.
