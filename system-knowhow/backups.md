---
name: Backups
description: How Lucidos backs up a workspace — what's included (workspace files + .git + a database dump, NOT ~/.lucidos), the encrypted-archive pipeline, the schedule (a cron expression in the USER'S timezone), and how the agent reads status with get_backup_status and changes the schedule/provider/retention with set_preference. Covers the encryption key (auto-generated, must be stored to restore), retention/pruning, run history + duration, staleness, and that restore happens from the workspace picker (not the engine). Load when the user asks about backups — "when's my next/last backup", "how big/long are backups", "back up to Dropbox", "change the backup time", "is my backup stale".
---

# Backups

A **backup** is one encrypted archive of a workspace, uploaded to a cloud
provider. Restoring it recreates the workspace's files and database on a fresh
machine.

## What a backup contains

The pipeline is: `pg_dump` the workspace database → `tar` the workspace files →
`zstd` compress → `AES-256-GCM` encrypt → upload to the provider.

Included:

- The workspace **database** (a `pg_dump` custom-format archive at the archive
  root) — the event store and all projections.
- The workspace **files** under the workspace dir, **including `.git/`** (artifact
  version history).

Excluded:

- **`.lucidos/`** — ephemeral runtime/cache, rebuildable.
- **`data/postgres/`** and any `data/postgres.*` siblings — the live PGDATA is
  captured via `pg_dump`, not copied.
- **`~/.lucidos/`** (the user-level shared dir) — NOT backed up. It is
  machine-global state (the gateway registry, deleted-workspace stashes, caches)
  that restore would have to discard anyway, so backing it up was pure dead
  weight. (Implication: user-level integration data under `~/.lucidos` is not
  protected by a workspace backup.)
- Anything matched by the workspace's optional **`data/.backupignore`**
  (gitignore-style, workspace-relative paths).

## The schedule (in the user's timezone)

The schedule is a **6-field cron expression** — `second minute hour day-of-month
month day-of-week` — interpreted in the **user's timezone** (the `timezone`
preference), exactly like triggers. So `0 0 3 * * *` ("daily at 03:00") fires at
03:00 **local** time, not UTC. Changing the `timezone` preference re-aligns the
backup automatically (no restart).

The schedule, provider, and retention are ordinary agent-settable preferences:

| Key | Value | Meaning |
|---|---|---|
| `backup_schedule` | a 6-field cron, or `off` | When automatic backups run (user's timezone). `off` disables them. |
| `backup_provider` | `google_drive` \| `dropbox` | Where to upload. The account must be connected in Settings → System → Backup (OAuth). |
| `backup_retention` | `1`–`50` | How many recent backups to keep; older ones are pruned after each success. |

Set them with `set_preference` — e.g. `set_preference(key="backup_schedule",
value="0 0 3 * * *")` then `set_preference(key="backup_provider",
value="google_drive")`. The change re-registers the schedule immediately (no
restart). Enabling a schedule with no connected provider will let backups run but
the upload fails until the user connects the provider in Settings → System →
Backup.

## Reading status — `get_backup_status`

Call **`get_backup_status`** (read-only, no arguments) to report:

- the schedule + the **next** scheduled run (computed in the user's timezone),
- the provider and retention,
- the **last** run with its **duration** and (on success) filename + size,
- a **recent run history** (start/finish/size for each — the durable record lives
  in the `BackupCompleted` / `BackupFailed` events),
- whether backups are **stale** (none recent).

Use it to answer "when's my next/last backup?", "how big/long are my backups?",
or to check before changing the schedule.

## Encryption key

Backups are encrypted with a per-workspace key. The first scheduled backup
auto-generates one if none exists and notifies the user to store it — it **cannot
be recovered** and is **required to restore**. The user can view/copy it in
Settings → System → Backup.

## Restore

Restore is **not** an engine operation and not something the agent does: it
happens from the **workspace picker** (the gateway provisions a new workspace and
unpacks the archive into it). Point the user there.
