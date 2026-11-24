# 0779: Backup defines stale, overdue and provider readiness once and leaves the user directory out of archives

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0015](0015-restore-in-the-workspace-picker.md), [0520](0520-backups-seal-to-an-age-recipient.md)

## Context

The backup module in `crates/lucidos-core/src/core/backup/mod.rs` feeds several
surfaces. These are the Settings page, the agent, the workspace picker, the
app-shell banner and the gateway restore path. They must agree on what a backup
status means. Archives must also restore safely onto a machine that already has
its own state.

This record restates the design comments of
`crates/lucidos-core/src/core/backup/mod.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- One constant, `BACKUP_STALE_AFTER_SECONDS`, defines stale as 24 hours.
  `api::backup::build_backup_status` and `api::backup::build_last_successful`
  both read it.
- The frontend renders the stale boolean and does not derive its own 24 hours.
- A separate overdue threshold drives the app-shell banner. Stale only colours
  a status line.
- Provider readiness is one value that says whether the OAuth account is
  connected and holds every required scope. The Settings page
  (`api::backup::list_providers`) and the agent's `get_backup_status` share it.
- Connected and holding every required scope is a derived method, not a stored
  boolean.
- `run_backup` persists the last run's terminal outcome on both paths under
  `prefs::BACKUP_LAST_RUN`.
- The `events` table is the durable run history. The run list is a read model
  over persisted `BackupCompleted` and `BackupFailed` events.
- Backup archives exclude the user-level `~/.lucidos/`. The workspace `.git/`
  stays in. `.lucidos/`, `data/postgres*` and `data/.backupignore` stay out,
  per `is_excluded_workspace_path`.
- `restore_archive_into` restores into a workspace directory and database that
  the gateway has already provisioned. It decrypts, decompresses, moves the
  workspace files in and runs `pg_restore` into `database_url`. It returns the
  warnings the user must see.
- `restore_archive_into` takes the archive from disk, runs no
  `init-workspace.sh` and runs no migrations.
- It always drops any `user_dir/` from the staging tree before moving files in.

## Rationale

- Two surfaces that each define stale can disagree about the same word. One
  threshold keeps them aligned.
- Stale and overdue are different concepts. A manual-only workspace is
  routinely a day old, and nagging at 24 hours would train users to dismiss the
  banner.
- If only the Settings page knew provider state, an agent could report a backup as fully configured. Its upload leg could still have no account behind it. One shared
  value means the human and the agent are told the same thing.
- A stored boolean beside the scope list it comes from can disagree with that
  list. A method cannot. Two reconciliations in the Dropbox scopes and refresh
  plan made this point.
- The `BackupCompleted` and `BackupFailed` SSE events are ephemeral. The
  persisted record and the events table let the page survive engine restarts.
- Legacy archives carry `user_dir/`. Applying it on restore would clobber the
  target's gateway registry and other machine-global state.
- Backing up `~/.lucidos/` is multi-GB of dead weight. The gateway's `deleted/`
  stashes alone can be several GB.
- The gateway owns provisioning and the workspace directory, so restore only
  fills it. The engine the gateway spawns afterward runs `sqlx::migrate!()` at
  construction, which upgrades an older-schema restore.

## Consequences

- A change to either threshold is made in one place and reaches every surface.
- Archives stay small, and the gateway's own state is never in them.
- Restore does not use `~/workspaces/{name}`. The directory may live under the
  gateway app-data dir.
- Restoring a legacy archive silently discards its `user_dir/` content.
- Archive filenames follow `{ARCHIVE_PREFIX}{name}-{timestamp}{suffix}`. The
  suffix is `.age` for an archive sealed to the backup recipient and `.enc` for
  a legacy one.
- The gateway's `registry::parse_workspace_name_from_archive` spells the same
  filename shape and cannot import these constants. The two must be kept in
  step by hand.

## Alternatives considered

- Each surface keeping its own stale rule was rejected, because surfaces could
  disagree.
- Warning at the stale line was rejected, because it would train users to
  dismiss the banner.
- Storing a connected boolean next to the scope list was rejected, because the
  two can disagree.
- Relying on the ephemeral SSE events alone was rejected, because they do not
  survive an engine restart.
- Extracting `user_dir/` over `~/.lucidos` on restore was rejected, because it
  clobbers machine-global state.
- Including `~/.lucidos/` in archives was rejected, because restore discards it
  and it is multi-GB of dead weight.
- Running provider download, `init-workspace.sh` and migrations inside restore
  was rejected. That was the old cloud `restore_backup` shape, and the gateway
  and engine now own those steps.
