---
name: Workspace File Conventions
description: Workspace file layout (artifacts/, apps/, knowhow/, intents/, scripts/, config/, env vars) and the type scale to paste into standalone HTML reports.
---

# Workspace File Conventions

How files and data are organized in a Lucidos workspace.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/best-practices/workspace-layout` | artifacts/: User Data & Content (Fixed directories; App data storage; Generated content; Standalone HTML: paste this type scale, never recall one; What a standalone HTML document can do); apps/: App UIs & Logic; knowhow/: Shared Domain Knowledge; intents/: User Intents; scripts/: Shared Scripts; config/: Engine Configuration; Every subprocess call is a fresh process; Credentials in subprocesses: name what you use; Listing this machine's workspaces; The workspace database: `LUCIDOS_DATABASE_URL` |
| `system-knowhow/best-practices/environment-variables` | Environment variables: Per-Workspace Config; Key Rules; Images posted in a thread |
