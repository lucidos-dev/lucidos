---
name: Backups
description: Workspace backups: coverage, the encrypted archive, the schedule, the encryption key needed to restore, retention, staleness, and provider scopes. Load for "when is my next backup", "back up to Dropbox", "change the backup time", "is my backup stale", or a restore.
---

# Backups

A **backup** is one encrypted archive of a workspace, uploaded to a cloud
provider. Restoring it recreates the workspace's files and database on a fresh
machine.

## What a backup contains

The pipeline is: `pg_dump` the workspace database → `tar` the workspace files →
`zstd` compress → `AES-256-GCM` encrypt → upload to the provider.

Included:

- The workspace **database**: the event store and all projections, as a
  `pg_dump` custom-format archive at the archive root.
- The workspace **files** under the workspace dir, **including `.git/`** (artifact
  version history).

Excluded:

- **`.lucidos/`**: ephemeral runtime/cache, rebuildable.
- **`data/postgres/`** and any `data/postgres.*` siblings: `pg_dump` captures
  the live PGDATA instead.
- **`~/.lucidos/`** (the user-level shared dir). It holds machine-global state
  (the gateway registry, deleted-workspace stashes, caches) that restore would
  discard anyway. So a workspace backup does not protect user-level integration
  data under `~/.lucidos`.
- Anything matched by the workspace's optional **`data/.backupignore`**
  (gitignore-style, workspace-relative paths).
- **Symlinks**: the backup does not follow them, so their targets are not
  archived. Following every link would upload files from anywhere on the
  machine. The exception is a top-level **`data`** symlink that relocates the
  whole tree to another disk: the backup follows it. `.backupignore` matches the
  link's path inside the workspace, so it cannot refuse a link's target.

## The schedule (in the user's timezone)

The schedule is a **6-field cron expression** (`second minute hour day-of-month
month day-of-week`) in the **user's timezone** (the `timezone` preference), like
triggers. So `0 0 3 * * *` ("daily at 03:00") fires at 03:00 **local** time, not
UTC. Changing the `timezone` preference re-aligns the backup with no restart.

The schedule, provider, and retention are ordinary agent-settable preferences:

| Key | Value | Meaning |
|---|---|---|
| `backup_schedule` | a 6-field cron, or `off` | When automatic backups run (user's timezone). `off` disables them. |
| `backup_provider` | `google_drive` \| `dropbox` | Where to upload. Independent of `backup_schedule`: a destination stays configured with the schedule `off`, and the Backup page opens on it. The account itself is connected in **Settings → Accounts**, not here. |
| `backup_retention` | `1`–`50` | How many recent backups to keep; older ones are pruned after each success. |

Set them with `set_preference`, for example `set_preference(key="backup_schedule",
value="0 0 3 * * *")` then `set_preference(key="backup_provider",
value="google_drive")`. The schedule re-registers immediately, with no restart.
With no connected account, scheduled backups still run but the upload fails.

**A backup keeps the computer awake while it runs**, as all Lucidos work does
(ADR 0366). On macOS that is an idle-sleep assertion named after the workspace,
visible in `pmset -g assertions`. Lid close, an explicit Sleep or a dying
battery still sleep the Mac. So the upload survives a sleep anyway. The
resumable upload retries the step that lost its connection. A failed run starts
with how long the computer slept, for example "The computer slept for 13
minutes during this backup."

## Where each half is configured (do not mix these up)

Two Settings pages own two halves. Naming the wrong one is the most common way
this flow goes wrong:

| What | Where | What lives there |
|---|---|---|
| **The backup itself** | Settings → System → Backup | Provider dropdown, *Back up now*, the schedule, retention, the encryption key, and a health card (last run, last cloud backup, staleness). The dropdown opens on the configured `backup_provider` and **writes** it: picking one there is the same act as `set_preference(key="backup_provider", …)`. |
| **The provider account** | **Settings → Accounts** | The *Connected accounts* list. This is the ONLY place a Google / Dropbox account is connected, and the only place its OAuth app registration is stored. |

The Backup page has no account UI. When the selected provider has no connected
account, it shows a red line linking to Settings → Accounts. So:

- Never tell the user to "connect Dropbox in Settings → System → Backup". There
  is nothing to connect there.
- Setting `backup_provider` does NOT connect anything. Call `get_backup_status`
  afterwards: backups do not work until it reports the account connected.
- If it is not connected, connect it yourself with `connect_oauth_account`
  (see `system-knowhow/oauth-providers.md`), or send the user to
  Settings → Accounts. Do not report the setup as complete before then.

## What each provider's account needs

A connected account must also carry the scopes the backup uses. The Backup page
shows a missing scope as its own state (connected but not ready). It offers
**Grant access**, which re-runs the authorization with the right scopes.

**Both surfaces name the same missing scopes**: the page reads "<provider> is
missing the `files.metadata.read` permission", and `get_backup_status` lists them
on its `Provider:` line. Report them by name, not as "not granted". A grant one
scope short usually means the provider's own console does not enable that
permission. Pressing *Grant access* again changes nothing until the user fixes
that (see the Dropbox App Console rule below).

**Google Drive** needs `https://www.googleapis.com/auth/drive.file`. A Google
account connected for calendar or mail alone will not upload.

**Dropbox** needs four scopes, plus one extra step:

| Scope | Used for |
|---|---|
| `files.content.write` | Creating the backups folder, uploading, pruning old backups |
| `files.content.read` | Downloading an archive when restoring |
| `files.metadata.read` | Listing backups, which drives pruning and the health card |
| `account_info.read` | Naming the connected account |

The extra step: **the Permissions tab of the user's app in the Dropbox App
Console must permit each scope first**. An authorization request can only narrow
what the console allows, never widen it. And **enabling a permission there does
not change an account that is already connected**: the token keeps the scopes it
was issued with. So after a console change the user must reconnect (Settings →
Accounts, or *Grant access* on the Backup page). A token refresh will not do it.

So when a Dropbox backup fails with *"does not have the required scope
'files.content.write'"*, give both halves in order: enable the permissions in the
App Console, then reconnect. Ticking the box alone leaves the same error.

A Dropbox client registration also needs
`authorize_params: token_access_type=offline`. Without it the connection carries
no refresh token and stops working within hours. See
`system-knowhow/oauth-providers.md`.

## Reading status: `get_backup_status`

Call **`get_backup_status`** (read-only, no arguments) to report:

- the schedule + the **next** scheduled run (computed in the user's timezone),
- the provider and retention, **and whether that provider's account is
  connected** (the upload fails until it is, so treat "not connected" as
  "backups are not set up yet"),
- the **last** run with its **duration** and (on success) filename + size,
- a **recent run history** (start/finish/size for each; the durable record lives
  in the `BackupCompleted` / `BackupFailed` events),
- whether backups are **stale** (none recent).

Use it to answer "when's my next/last backup?", "how big/long are my backups?",
or to check before changing the schedule.

The **workspace picker** shows one line per workspace row: "Backed up 3h ago", or
a warning when the last good backup is stale, never happened, or was never set
up. The line goes quiet for a workspace that is not running. It answers "is my
data safe?" at a glance; `get_backup_status` holds the detail.

## Encryption key

A per-workspace key encrypts each backup. The first backup that needs one
creates it: a manual backup, turning a schedule on, or a scheduled run. Lucidos
then notifies the user to store it. It **cannot be recovered** and is **required
to restore**. The user can view and copy it in Settings → System → Backup.

## Restore

Restore is **not** an engine operation, and the agent does not do it. It
happens in the **workspace picker**: the gateway provisions a new workspace and
unpacks the archive into it. Point the user there.
