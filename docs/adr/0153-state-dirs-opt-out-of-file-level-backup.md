# 0153: State directories opt out of file-level backup

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

A **file-level backup** is a backup taken by the operating system or a
third-party product that copies files off the machine. Time Machine, restic and
borg are the shapes people run. It is not a Lucidos **backup**, which is the
workspace's own encrypted archive.

Lucidos ships two secrets to every install:

- `<workspace>/.lucidos/backup.key`, the AES-256 key that decrypts every cloud
  backup that workspace ever uploaded.
- The `credentials` table, which holds provider API keys, OAuth client secrets
  and email passwords in plaintext.

The archives themselves hold up. They are AES-256-GCM, and the tar drops
`.lucidos/` (`is_excluded_workspace_path` in `core/backup/mod.rs`), so an
archive never carries its own key. The hole is the local copy on disk.

Measured with `tmutil isexcluded` on 2026-08-10 and again on 2026-08-29:

| Path | State |
|---|---|
| `<workspace>/.lucidos` | Included |
| `<workspace>/.lucidos/backup.key` | Included |
| `~/.lucidos` | Included |
| `~/.lucidos/gateway` | Included |

So a Time Machine snapshot carries the key and the credential store side by
side. Anyone holding the external disk holds both halves. This is a product
defect, not a local misconfiguration.

Where the credential store sits depends on the shape:

| Shape | Postgres backend | Data directory |
|---|---|---|
| macOS `.app` | `PgBackend::Embedded` | `<app-data>/pgdata` |
| headless tarball | `PgBackend::Embedded` | `<gateway-data>/pgdata` |
| dev checkout | `PgBackend::Docker` | the `lucidos-pg-shared` volume |

Both shipped shapes keep the cluster in a directory Lucidos itself creates, at
`initdb_cluster` in the gateway's `postgres.rs`. Only the dev checkout puts it
somewhere Lucidos does not own.

## Decision

Four parts.

**1. Lucidos sets a file-level backup exclusion on its own state directories.**
It sets the exclusion when the directory is created. It re-checks on every
start, so an install that predates this change converges on its next boot. The
re-check is silent when the exclusion is already correct.

**2. Two targets, and they are directories.** `<workspace>/.lucidos/` and
`<gateway-data>/pgdata/`. Never `<workspace>/data/`, never
`<app-data>/workspaces/`, and never `~/.lucidos` as a whole, because each of
those holds content the user authored.

A Postgres major upgrade adds a third, and it is the same target under a new
name. `move_data_dir_aside` renames the old cluster to `pgdata.foreign-<major>-
<stamp>` and keeps it for ever, so that copy holds the credential store too. It
is marked by the function that preserves it, not by the caller, so no later
caller can keep a data dir and forget.

**3. The backup key stays a file. It does not move to the macOS Keychain.**

**4. Encrypting the credential store at rest is accepted as the right answer,
and is not this change.** It is what Linux gets, since Linux has no exclusion
mechanism worth the name. Scoped below.

## Rationale

**Exclusion is the cheap half, and it is the difference between the key leaving
the machine and not.** It needs no key management, no new recovery path and no
migration of stored data. On macOS the mechanism is one extended attribute,
`com.apple.metadata:com_apple_backup_excludeItem`, holding the 61-byte binary
plist that `CSBackupSetItemExcluded` writes. Setting it on a directory covers
the whole subtree: probed on 2026-08-29, one call flipped a directory, a child
directory and a leaf file from Included to Excluded.

**Directories, never individual files.** An atomic write is write-tmp then
rename, which replaces the inode and drops the xattr with it. A file-level
exclusion would therefore evaporate on the next save, silently, which is the
worst failure a security control can have. A directory's xattr survives, and
covers whatever gets written inside it later.

**A file-level copy of a live Postgres cluster was never a backup.** It is a
torn read of a running server. Lucidos's own backup runs `pg_dump` before the
tar, so the database already has an encrypted, off-machine recovery path that
restores. Excluding `pgdata` removes a false sense of safety and a real
confidentiality hole in one step.

**The key stays a file because a Keychain item cannot reach every shape we
ship.** Three costs sink it:

- **Linux has no Keychain.** The nearest thing is the Secret Service D-Bus API,
  which needs a running `gnome-keyring` or `kwallet`. The headless tarball's
  Linux shape is `systemd --user` on a server with no session bus, so the
  fallback would be a file anyway. That is three code paths for one secret,
  where the least-tested one runs on the platform with the least support.
- **Our own builds invalidate the ACL.** A Keychain item's access control lists
  the binary that may read it. Release artifacts are ad-hoc signed by
  construction (ADR 0034), so every update presents a new identity. The user
  would face an "allow access" prompt after each update, or lose the item.
- **A lost item is every backup lost.** A file next to the workspace is
  recoverable by the user with a text editor. A Keychain entry deleted by a
  keychain reset takes every archive with it, and the archives are the thing the
  key exists to protect.

**At-rest encryption is the portable answer, so it is the one that must exist on
Linux.** An exclusion expresses an intent ("this directory is not eligible for
file-level backup") that macOS happens to have a word for. Linux has no word for
it, so the intent has to be met by making the data unreadable instead. Deciding
that now is what keeps the design from being macOS-only even though this step
is.

## Consequences

**We keep** the key file's shape, its 0600 mode, the "Show backup key" reveal in
Settings, and every existing restore path. No key is read, moved or rotated by
this change.

**We give up Time Machine as an accidental key escrow.** Today a full restore of
a home directory brings `backup.key` back. After this change it does not. That
is the point, and it converts a confidentiality hole into an availability
requirement: the user must record the key themselves. Settings then Backup
already reveals it for exactly that purpose. The requirement is not new, because
restoring onto a fresh machine never had a `.lucidos/` to read the key from.

**We give up a file-level copy of the live cluster.** Recovering the database
now means a Lucidos backup, which is the mechanism that actually restores.

Four residuals stay open, and each is named rather than assumed away.

**The Docker volume, dev only.** On a dev checkout the cluster lives in the
`lucidos-pg-shared` Docker named volume, whose backing store is
`~/Library/Containers/com.docker.docker/Data`. That is another product's
container directory, and Lucidos must not write attributes inside it.

The user action is to exclude the Docker data directory themselves. System
Settings then General then Time Machine then Options does it, and so does
`tmutil addexclusion`. No shipped install is affected, because both shipped
shapes use the embedded cluster. At-rest encryption of the credential store is
what closes this half for good, which is part of why it is accepted above.

**`~/.lucidos` stays Included.** It mixes machine-local secrets (`local-token`,
the port registry, the paired-device store) with `knowhow/`, which the user
authored and which has its own git repo. A directory-level exclusion cannot
separate the two, and excluding the directory would drop the user's knowhow from
their backup. Moving the machine-local files under a dedicated subdirectory
would resolve it, and is a follow-up rather than part of this change.

**APFS local snapshots still hold everything.** Time Machine snapshots the whole
volume locally, then copies the non-excluded files to the destination. An
exclusion keeps the data off the external disk, which is the threat here. It
does not keep it out of a local snapshot, which stays on the internal disk under
FileVault.

**Linux gets nothing from this change.** `CACHEDIR.TAG` is the only marker with
broad support, and the alternative below says why it does not fit. The helper is
a no-op there by design.

### Scope of the deferred at-rest encryption

Recorded so the follow-up starts from a decision rather than a blank page.

- Envelope-encrypt the secret-bearing columns only: `credentials.auth_value`,
  and the token columns on `oauth_accounts` and `email_accounts`. Leave every
  other column readable, so a query by service name still works.
- The data key is a separate 0600 file in the excluded state directory, so the
  two halves are not adjacent on the same backup medium.
- It costs the SQL migrations that rewrite `auth_value` today. Four of them do,
  and each would have to move into the engine.
- It costs recovery: a lost data key means re-authorizing every provider. That
  is far cheaper than a lost backup key, which loses archives instead.

## Alternatives considered

**Move the key to the macOS Keychain.** Rejected on the three costs in the
Rationale. On Linux it is not merely worse, it is absent: the design would ship
a file fallback that carries the whole security property on the platform where
nobody tested it.

**Encrypt the credential store now, instead of the exclusion.** Rejected on
sequencing, not on merit. It is a schema and key-management change with a real
recovery story, and it would leave `backup.key` exposed for as long as it takes.
The exclusion is a few lines and covers both halves on every shipped install
today.

**`tmutil addexclusion -p`, the sticky path exclusion.** Rejected. It writes to
the Time Machine preference plist, needs elevated privileges, and applies to a
path rather than to the item. A workspace the user moves would silently lose
its exclusion. The per-item xattr travels with the directory and needs no
privileges.

**Write `CACHEDIR.TAG` into `.lucidos/` for Linux.** Rejected as dishonest. The
tag declares a directory to be regenerable cache. Borg, restic and `tar
--exclude-caches` act on that declaration. The key file is not regenerable, so
the tag would invite a backup tool to skip the one thing the user cannot
reconstruct. Portable protection comes from encryption, not from a marker that
means something else.

**A one-off script, or a line in the docs.** Rejected. It fixes the machine it
runs on and no other. Every workspace created afterwards is Included again, and
the defect ships to every new user.

**Exclude `~/.lucidos` and `<app-data>` wholesale.** Rejected. Both hold user
content: `~/.lucidos/knowhow` in the first, and the packaged install's
`workspaces/` tree with its artifacts in the second. Excluding a directory that
holds artifacts trades a confidentiality bug for a data-loss bug.

## Verification

```sh
tmutil isexcluded ~/workspaces/*/.lucidos ~/workspaces/*/.lucidos/backup.key
tmutil isexcluded ~/workspaces/*/data
```

The first line must report `[Excluded]` on every path. The second must still
report `[Included]`: artifacts are user content and stay in the user's backup.
On a packaged install, add the cluster:

```sh
tmutil isexcluded ~/Library/Application\ Support/*/pgdata
```
