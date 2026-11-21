# 0015 — Restore lives in the workspace picker: local-file only, run via the engine `restore-archive` subcommand

- **Status** — Accepted
- **Date** — 2026-06-17

## Context

Backup *restore* used to live in Settings → Backup, inside a running workspace:
you picked a cloud provider (Google Drive / Dropbox), listed your backups,
selected one, pasted the backup key, and the engine restored it into a brand-new
workspace (`core::backup::restore_backup` → `init-workspace.sh` →
`~/workspaces/{name}` → `pg_restore`). That entry point is awkward for the thing
people actually want — restoring when they have *no* workspace yet, or want a
fresh one. The natural home is the **workspace picker** (the gateway's `/~/`
surface), alongside create / rename / delete.

But the picker can't reuse the old flow:

- **The gateway has no per-workspace OAuth.** Provider tokens live in each
  workspace's Postgres (`oauth_accounts`); the picker has no workspace, so it
  cannot list or download from a cloud provider.
- **The gateway must not link the engine crate** (ADR 0014 §1). The restore logic
  (decrypt / unpack / `pg_restore`) lives in `lucidos-engine`.
- **The gateway owns provisioning now.** A restored workspace must be registered
  in the gateway registry and provisioned the gateway way (Docker / embedded
  Postgres), not via `init-workspace.sh` + `~/workspaces/`.

## Decision

Restore is a **picker-only** action that restores from a **local encrypted
`.enc` file** the user drops or picks (no cloud provider in the picker). The
gateway provisions a new workspace, then **shells out to the engine binary's new
one-shot `lucidos-engine restore-archive` subcommand** to do the decrypt / unpack
/ `pg_restore` into the provisioned dir + database. The Settings → Backup restore
UI is removed; Settings keeps backup *creation* + scheduling.

The picker asks for the **backup key** and derives the workspace name from the
archive filename (`lucidos-backup-{name}-{ts}.enc`), asking for a name **only on
a collision** with an existing workspace.

## Rationale

- **Reaches the engine code without an engine dependency.** The gateway already
  spawns the engine by path (`LUCIDOS_ENGINE_BIN`); a one-shot subcommand is the
  same seam. ADR 0014 §1 stays intact.
- **Single provisioning + single boot.** The gateway provisions Postgres once,
  the CLI restores into it, then the gateway spawns the engine server — whose
  construction runs `sqlx::migrate!()`, upgrading an older-schema backup. No
  double-provision, no restart-to-migrate dance.
- **Local file sidesteps the OAuth problem entirely** and matches the minimal
  "just give us the key" UX. (Users already have the `.enc` — they downloaded it
  from their provider, or it's the artifact a future "export" produces.)
- **Don't clobber machine-global state.** The old restore extracted the archive's
  `user_dir/` over `~/.lucidos`; in the gateway world that would overwrite the
  gateway registry (`~/.lucidos/gateway/config/workspaces.json`). The new
  `restore_archive_into` drops `user_dir/` — it restores only the workspace dir +
  database.

## Consequences

- **Kept:** backup creation, providers, scheduling, retention, the backup key
  surface, `BackupProgress`/`BackupCompleted`/`BackupFailed` (backup-only).
- **New:** `core::backup::restore_archive_into`, the `lucidos-engine
  restore-archive` subcommand, gateway `POST /~/api/v1/control/workspaces/restore`
  (multipart upload) + `GET/DELETE /~/api/v1/control/restore-status`, a
  single-slot in-memory `RestoreStatus` on the gateway, picker UI.
- **Removed:** engine `Restore{Progress,Completed,Failed}` SystemEvents,
  `engine.restore_state` / `core::backup::RestoreState`, the cloud
  `core::backup::restore_backup` + `init_workspace` + `resolve_restore_workspace_path`,
  and the `/api/v1/backup/restore*` / `start-workspace` / `validate-workspace-name`
  routes + their frontend wiring.
- **Given up (for now):** cloud-provider restore from the picker, and restoring
  the archive's `~/.lucidos` content. Both are out of scope; see Alternatives.
- A failed restore cleans up only what the attempt created (the provisioned
  Postgres + the fresh dir); the registry entry is committed only after the
  archive is in place, so a pre-commit failure leaves nothing registered.

## Alternatives considered

- **Cloud-provider restore in the picker.** Rejected: needs OAuth the picker
  doesn't have. Would require a new user-global account store or borrowing tokens
  from an already-running connected workspace — a separate, larger subsystem.
- **Pre-create an empty workspace, then have its engine restore over itself via
  an HTTP endpoint.** Rejected: the engine would boot on an empty DB (build
  projections), get `pg_restore`'d out from under itself, and need a restart to
  rebuild — and for the embedded backend, re-provisioning would move the cluster
  to a new port between the two steps. The CLI-then-boot order avoids all of it.
- **Move the restore logic into the gateway crate.** Rejected: violates ADR 0014
  §1 (the network-facing process must not link the engine's heavy core).
- **Keep restore in Settings too.** Rejected by the product decision ("move to
  picker only") — one entry point, no duplicate surface to keep honest.
- **Restore `user_dir/` selectively (exclude `gateway/`).** Rejected as scope:
  machine-global knowhow/cache restore is ambiguous and risky; dropping
  `user_dir/` entirely is the safe default.
