---
name: Workspace audit: knowhow
description: The knowhow audit section: frontmatter, naming, placement, references no doc names, and orphaned docs.
---

# Workspace audit section: knowhow

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

No grep decides this section. Its checks read files and events directly,
so it adds no row to the receipts.

## Checks

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

Per `docs/taxonomy.md` (frontmatter shape) and `system-knowhow/best-practices.md` (file placement):

- Frontmatter has `name` (required) and `description` (recommended, since semantic discovery uses it).
- Filename is descriptive, not generic.
- App-scoped knowhow doesn't reference things outside its app; shared knowhow doesn't name specific apps.
- **A file below the listing depth that no doc names.** Per `system-knowhow/building-knowhow.md` § "Where the file goes" (mirrored in `docs/taxonomy.md` § "Knowhow: Docs and References"), a root lists `data/knowhow/<name>.md` and `data/knowhow/<group>/<name>.md`. Under `data/apps/<id>/knowhow/` and `data/triggers/<slug>/knowhow/` it lists one level only. A deeper file is a *reference* belonging to the doc above it.

  That shape is legitimate, so never flag depth alone. Flag only a reference **no doc names**: grep the sibling docs for its full id (the path under the root without `.md`). A named one is correct.

  An unnamed one is unreachable: it sits in no routing list, and nothing tells the LLM the id exists. Recommend naming it from the doc that should own it, or moving it up to the listed depth. Severity: **stale** (silently invisible). Nothing fails at runtime, so only this check reveals it.
- **Orphaned files under `data/knowhow/`**: a file no consumer names is potentially dead. Its id appears in NO trigger's stale `run.knowhow` (see the `triggers` section), NO intent's `knowhow:` frontmatter (see the `intents` section), and NO app `manifest.json`/`config/*.json`. The usual cause is a trigger that lost `run.knowhow` when the preload was retired, with the content never moved into its intent. Recommend one fix: (a) inline the procedure into a trigger's intent, (b) move the file into `data/triggers/<slug>/knowhow/` if trigger-specific, or (c) delete it. Severity: **stale** (review). Reference: `system-knowhow/triggers.md`.

Knowhow bodies also carry pre-proxy API patterns. Run the `cross-cutting` section's credential check over the fenced code blocks here too. Knowhow is where a leak *spreads*: it tells the next session how to call the API, so the finding returns after the code is clean.
