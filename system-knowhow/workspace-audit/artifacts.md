---
name: Workspace audit: artifacts
description: The artifacts audit section: structural rules only, never the user content itself.
sources: ["system-knowhow/best-practices.md", "system-knowhow/lucidos-cli.md"]
---

# Workspace audit section: artifacts

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

No grep decides this section. Its checks read files and events directly,
so it adds no row to the receipts.

## Checks

- **Event exports mixed with live metadata.** Thread-scoped CLI event reads print current liveness on stderr; stdout remains historical JSON.
  Keep the streams separate when exporting event artifacts; see `system-knowhow/lucidos-cli.md` § `lucidos events query`.

- **Draft exports remain user content.** A single-draft CLI read includes its exact compose snapshot; list rows contain previews without that snapshot.
  Check exported report structure against `system-knowhow/lucidos-cli.md` § `lucidos threads drafts`, without enumerating saved text or attachments.
  Flag reports claiming a list preview is the complete saved draft as **stale**. Reading or exporting a draft never authorizes submission or deletion.
  A draft whose `drafted_by` names a thread is still the owner's to send, edit or discard.
  An agent sends one only with the owner's authority (§ `lucidos threads submit-draft`).

- **Workspace cleanup is not artifact deletion.** Use Manage workspaces for frozen cleanup and Trash recovery; never substitute filesystem removal.

- **Snapshot archives treated as artifacts.** Check snapshot procedures against `system-knowhow/backups/workspace-snapshots.md`.
  Flag instructions to move archives into `data/artifacts/` as **broken**; the external snapshot store owns them.

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

Don't enumerate user content. Per `system-knowhow/best-practices.md`:

- No `data/artifacts/artifacts/`.
- No `data/artifacts/themes/` (or `data/artifacts/looks/`, its name before the rename), `data/artifacts/config/`, `data/artifacts/auth-modules/` or `data/artifacts/scripts/`. `lucidos data write` once filed those trees under `artifacts/`, as the agent's file tools did for themes. Nothing reads them there: a theme never shows, and an `apis.json` never loads. Severity: **broken**. Owns the rule: `system-knowhow/lucidos-cli.md` § `lucidos data path`.
  - Flag a file only when its content fits the tree: a theme JSON, `apis.json`, a signer `.wasm`, or a handshake script. An artifact project that shares the name is not a finding.
  - Recommend writing each file again at its real path with `lucidos data write`, without the `artifacts/` segment, which also runs the engine's checks. Then delete the copy under `artifacts/`.
- No HTML artifact that expects the shell's authority. A previewed or served HTML file runs sandboxed at an opaque origin. Its engine calls, its `fetch()` of sibling files and its browser storage all fail. Grep the artifact's own inline `<script>` blocks in `data/artifacts/**/*.html` for `/api/v1`, `new EventSource(`, `fetch(`, `localStorage`, `sessionStorage` and `parent.document`. Skip a vendored library file.
  - An unguarded storage access throws and stops the script. Severity: **broken**. A `fetch` inside a `catch` that falls back leaves an empty or stale report. Severity: **stale**.
  - Recommend writing the data into the file when it is written, or making it an app.
  - Owns the rule: `system-knowhow/best-practices.md` § What a standalone HTML document can do.
- No bulk imports under `data/artifacts/imported/<service>/` that match the "dumped repo / archive" anti-pattern (file count + size are the tell). Suggest moving bulk to `.lucidos/tmp/` or `~/.lucidos/data/`.
- No orphaned `imported/<service>/` directories. Flag for review (don't auto-delete).
- App data sits under `data/artifacts/<app-id>/`, not at the artifacts root.
