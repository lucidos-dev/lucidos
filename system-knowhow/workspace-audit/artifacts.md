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

- **Spawn diagnostics mistaken for verified attribution.** A CLI fallback note explains why a spawn arrived directly.
  Check exported attribution against `system-knowhow/lucidos-cli.md` § `lucidos spawn-thread`; the note does not verify its claimed source.

- **Raw host logs stored as artifacts.** Flag recipes copying raw host captures into workspace artifacts as **broken**.
  Use the owner-approved transfer flow in `system-knowhow/lucidos-cli.md` § `lucidos diagnostic-log prepare-host-transfer`; keep only metadata and integrity status.

- **Saved update status treated as live channel availability.** A gateway status export is a snapshot.
  Check current availability with `lucidos update-channel list`; see `system-knowhow/lucidos-cli.md` § `lucidos update-channel`.
  Flag a saved Local directory treated as proof of a usable feed as **stale**.

- **Ceiling handoff exports assumed to wait.** Check exported `SessionHandedOff.starts` against `system-knowhow/lucidos-cli.md` § `lucidos successor start`.
  Flag reports inferring next-input start from the ceiling cause alone as **stale**.

- **Stop inferred from a session export.** Read durable Stop state through the events described in `system-knowhow/glossary.md` § Stopped.
  A later session alone does not prove the owner resumed the thread.

- **Plan approval confused with implementation.** A committed plan may have approval for later only.
  Check marker scope and lifecycle events before treating an applied plan artifact as implemented code.

- For scripts generating artifacts from the workspace database, apply `system-knowhow/workspace-audit/scripts`' table-policy check without enumerating exported content.

- **Device sessions saved as evidence.** Check rig output against `system-knowhow/lucidos-cli.md` § `lucidos workspaces device-session`.
  Flag a session copied into a report or tracked artifact as **broken**; keep only the private runtime file.
  Lasting consent never makes the credential safe to include in evidence.
  Check recorded mint procedures with `system-knowhow/workspace-audit/cross-cutting`'s direct-port device-authority check.

- **Event exports mixed with live metadata.** Thread-scoped CLI event reads print current liveness on stderr; stdout remains historical JSON.
  Keep the streams separate when exporting event artifacts; see `system-knowhow/lucidos-cli.md` § `lucidos events query`.

- **Saved main-check status treated as live repair authority.** A status export is a point-in-time report.
  Check its conclusions against current `lucidos main-check status --json`, including the repair owner and current findings.
  Flag reports claiming a waiting repair already has a running thread as **stale**.

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
