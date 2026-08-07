---
name: Backups
description: Workspace backups: what is included, the encrypted archive, the cron schedule in the user's timezone, the encryption key that must be stored to restore, retention, run history, staleness, the provider scopes each backend needs, and which Settings page owns which half. Load for "when is my next backup", "back up to Dropbox", "change the backup time", "is my backup stale", or a restore.
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
| `backup_provider` | `google_drive` \| `dropbox` | Where to upload. Independent of `backup_schedule`: a destination stays configured with the schedule `off`, and the Backup page opens on it. The account itself is connected in **Settings → Accounts**, not here. |
| `backup_retention` | `1`–`50` | How many recent backups to keep; older ones are pruned after each success. |

Set them with `set_preference` — e.g. `set_preference(key="backup_schedule",
value="0 0 3 * * *")` then `set_preference(key="backup_provider",
value="google_drive")`. The change re-registers the schedule immediately (no
restart). Enabling a schedule with no connected account will let backups run but
the upload fails.

## Where each half is configured (do not mix these up)

Two different Settings pages own two different halves, and telling the user the
wrong one is the single most common way this flow goes wrong:

| What | Where | What lives there |
|---|---|---|
| **The backup itself** | Settings → System → Backup | Provider dropdown, *Back up now*, the schedule, retention, the encryption key, and a health card (last run, last cloud backup, staleness). The dropdown opens on the configured `backup_provider` and **writes** it: picking one there is the same act as `set_preference(key="backup_provider", …)`. |
| **The provider account** | **Settings → Accounts** | The *Connected accounts* list. This is the ONLY place a Google / Dropbox account is connected, and the only place its OAuth app registration is stored. |

The Backup page has no account UI at all. It shows a red line linking to
Settings → Accounts when the selected provider has no connected account. So:

- Never tell the user to "connect Dropbox in Settings → System → Backup". There
  is nothing to connect there and they will not find it.
- Setting `backup_provider` does NOT connect anything. Check
  `get_backup_status` after setting it: it reports whether that provider's
  account is connected, and backups are not actually working until it says so.
- If it is not connected, connect it yourself with `connect_oauth_account`
  (see `system-knowhow/oauth-providers.md`), or send the user to
  Settings → Accounts. Do not report the setup as complete before then.

## What each provider's account needs

A connected account is not automatically a *working* account: it also has to
carry the scopes the backup uses. The Backup page reports that as its own state
(connected but not ready) and offers **Grant access**, which re-runs the
authorization with the right scopes.

**Both surfaces name the scopes that are missing**, and they name the same ones:
the page reads "<provider> is missing the `files.metadata.read` permission" and
`get_backup_status` lists them on its `Provider:` line. Use them. A grant that
came back one scope short looks exactly like an authorization that never
happened if you only report "not granted", and the remedy differs: the short
grant usually means the permission is not enabled in the provider's own console,
so pressing *Grant access* again changes nothing until that is fixed. See the
Dropbox App Console rule below.

**Google Drive** needs `https://www.googleapis.com/auth/drive.file`. A Google
account connected for calendar or mail alone will not upload.

**Dropbox** needs four scopes, and one extra step nothing else in Lucidos has:

| Scope | Used for |
|---|---|
| `files.content.write` | Creating the backups folder, uploading, pruning old backups |
| `files.content.read` | Downloading an archive when restoring |
| `files.metadata.read` | Listing backups, which drives pruning and the health card |
| `account_info.read` | Naming the connected account |

The extra step: **the Permissions tab of the user's app in the Dropbox App
Console has to permit each of those first**, because an authorization request can
only narrow what the console allows, never widen it. And **enabling a permission
there does not change an account that is already connected**: the existing token
and grant keep the scopes they were issued with, so after changing the console
the user must reconnect (Settings → Accounts, or *Grant access* on the Backup
page). A token refresh will not do it, since refreshing renews the scopes the
token already has.

So when a Dropbox backup fails with *"does not have the required scope
'files.content.write'"*, the fix is both halves in order: enable the permissions
in the App Console, then reconnect. Telling the user only to tick the box leaves
them looking at the same error.

While you are there: a Dropbox client registration also needs
`authorize_params: token_access_type=offline`, or the connection carries no
refresh token and stops working within hours. See
`system-knowhow/oauth-providers.md`.

## Reading status — `get_backup_status`

Call **`get_backup_status`** (read-only, no arguments) to report:

- the schedule + the **next** scheduled run (computed in the user's timezone),
- the provider and retention, **and whether that provider's account is
  connected** (the upload leg fails until it is, so treat a not-connected
  verdict as "backups are not set up yet"),
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
