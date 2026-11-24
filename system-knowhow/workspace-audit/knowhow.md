---
name: Workspace audit: knowhow
description: The knowhow audit section: frontmatter, naming, placement, references no doc names, and orphaned docs.
sources: ["docs/taxonomy.md", "system-knowhow/best-practices.md", "system-knowhow/best-practices/*.md", "system-knowhow/building-knowhow.md", "system-knowhow/triggers.md", "system-knowhow/triggers/*.md", "crates/lucidos-core/src/core/knowhow.rs"]
---

# Workspace audit section: knowhow

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

No grep decides this section. Its checks read files and events directly,
so it adds no row to the receipts.

## Checks

Resolve app-scoped knowhow IDs through the engine loader. A matching file alone
does not prove that its ID resolves from the intended scope.

- Check database-write recipes against `system-knowhow/best-practices/workspace-layout.md` § The workspace database.
  Apply `system-knowhow/workspace-audit/scripts`' table-policy check to fenced code examples too.

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

Per `docs/taxonomy.md` (frontmatter shape) and `system-knowhow/best-practices.md` (file placement):

- Frontmatter has `name` (required) and `description` (recommended, since semantic discovery uses it).
- Filename is descriptive, not generic.
- App-scoped knowhow doesn't reference things outside its app; shared knowhow doesn't name specific apps.
- **A file below the listing depth that no doc names.** Per `system-knowhow/building-knowhow.md` § "Where the file goes" (mirrored in `docs/taxonomy.md` § "Knowhow: Docs and References"), a root lists `data/knowhow/<name>.md` and `data/knowhow/<group>/<name>.md`. Under `data/apps/<id>/knowhow/` and `data/triggers/<slug>/knowhow/` it lists one level only. A deeper file is a *reference* belonging to the doc above it.
  That shape is legitimate, so never flag depth alone. Flag only a reference **no doc names**: grep the sibling docs for its full id (the path under the root without `.md`). A named one is correct.

  An unnamed one is unreachable: it sits in no routing list, and nothing tells the LLM the id exists. Recommend naming it from the doc that should own it, or moving it up to the listed depth. Severity: **stale** (silently invisible). Nothing fails at runtime, so only this check reveals it.
- **A knowhow file over 24 KB.** Per `system-knowhow/building-knowhow.md` § "Size", a coding agent's shell cuts a larger file before its end.
  List them with `find knowhow apps/*/knowhow triggers/*/knowhow -name '*.md' -size +24576c` from `data/`.
  Severity: **stale**. Recommend the index-plus-parts split that section describes, and name the file's size.
- **A recipe that treats a program grant as permission to write outside the workspace.**
  Check it against `system-knowhow/triggers/grouping-triggers.md` § Program grants.
  Mark it **broken** if it promises the program grant also permits a sandboxed write.
- **A recipe that requests a read for routine trigger progress.** Flag it as **stale**. Recommend `read: false` for ordinary progress and a specific reason when the user should read the outcome. See [Triggers](../triggers.md).
- **A recipe that tells the agent to make a SQL write to the workspace database.** Flag it as **broken**.
  The agent role writes no table. Name the command from `system-knowhow/best-practices/workspace-layout.md` § "The workspace database: `LUCIDOS_DATABASE_URL`".
  In a machine-made workspace the write still works, so say only that it announces nothing.
- **A recipe that relies on trigger category grants.** Flag it as **broken**.
  Point the owner to **Check my workspace** in the release notice or the blocked run's program grant request.
- **Orphaned files under `data/knowhow/`**: a file no consumer names is potentially dead. Its id appears in NO trigger's stale `run.knowhow` (see the `triggers` section), NO intent's `knowhow:` frontmatter (see the `intents` section), and NO app `manifest.json`/`config/*.json`. The usual cause is a trigger that lost `run.knowhow` when the preload was retired, with the content never moved into its intent. Recommend one fix: (a) inline the procedure into a trigger's intent, (b) move the file into `data/triggers/<slug>/knowhow/` if trigger-specific, or (c) delete it. Severity: **stale** (review). Reference: `system-knowhow/triggers.md`.

Knowhow bodies also carry pre-proxy API patterns. Run the `cross-cutting` section's credential check over the fenced code blocks here too. Knowhow is where a leak *spreads*: it tells the next session how to call the API, so the finding returns after the code is clean.

## Diagnostic evidence

When this audit needs runtime evidence, follow [Agent Sandbox](../agent-sandbox.md) for bounded diagnostic access.
A maintainer bundle omits legacy log text; its absence does not establish an audit finding.
