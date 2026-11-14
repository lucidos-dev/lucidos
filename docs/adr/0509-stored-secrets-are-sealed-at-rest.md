# 0509: Stored secrets are sealed at rest with a per-workspace key, and agents get a role that cannot read them

- **Status**: Accepted
- **Date**: 2026-10-17
- **Amends**: ADR 0481 (what `LUCIDOS_DATABASE_URL` signs in as) and ADR 0153
  decision 3 (where the backup key lives)
- **Amended by**: [0516](0516-subprocesses-get-only-the-secrets-they-name.md)
  (which secrets a subprocess gets)

## Context

Agents, scripts and triggers receive `LUCIDOS_DATABASE_URL` (ADR 0481). Until
this change it signed in as the engine's own role, `lucidos`, read-only by
default. That role can `SELECT` every stored secret: API keys, OAuth tokens,
email passwords, the VAPID private key, MCP server env values, the Claude
gateway tokens (ADR 0508). It can also
connect to every other workspace database in the shared cluster. A `pg_dump`
of a workspace carried every secret in plain text, and so did a copy of the
data directory or its WAL.

The user asked for two layers: encrypt every stored secret, and give agents a
database role that cannot read secrets at all.

## Decision

1. **Every secret column holds only sealed values.** A sealed value is text,
   `lsec1:<key id>:<base64 nonce and ciphertext>`. It is AES-256-GCM under one
   32-byte key per workspace database, with `table.column` as associated data.
2. **`SECRET_COLUMNS` in `core/secret_store/` is the one list of secret
   columns.** Each column has a `CHECK (value LIKE 'lsec1:%')` constraint. The
   stores bind `Secret<C>`, a type that seals as it binds and opens as it
   decodes.
3. **The key lives in a key store.** That is the macOS Keychain for an engine
   inside the signed `.app`, and a 0600 key file under `~/.lucidos/secret-keys/`
   everywhere else. `LUCIDOS_SECRET_KEY_STORE=keychain|file` overrides the
   choice.
4. **The backup key moves into the same key store.** A legacy
   `<ws>/.lucidos/backup.key` moves in on the next read.
5. **Subprocesses sign in as the workspace's agent role, `<database>_agent`**
   (shortened with a hash when Postgres's 63-byte limit needs it).
   It is a member of the `lucidos_agents` group. It can select no secret column
   and connect to no other database. The engine never hands a subprocess the
   owner role.

## Rationale

**Two layers, because each covers the other's gap.** The agent role stops a
casual `SELECT` by privilege. Sealing makes every copy of the data useless
without the key: a dump, a file copy, WAL, or a role nobody thought of.

**The database says which key it needs.** Every sealed value carries its key
id. At boot the engine compares those ids against the key store:

| Key in the store | Sealed values in the database | Result |
|---|---|---|
| none | none | Mint a key, seal any plain values, boot |
| present | any, all with its id | Seal any plain values, boot |
| none | some | **Boot fails** |
| present, another id | some | **Boot fails** |

A failed boot names the expected key id and the two ways out. `lucidos-engine
secrets import-key --key-file <file>` puts the right key in place. `lucidos-engine
secrets discard` deletes every value sealed with another key, mints a key when
there is none, and logs what went. It judges by key id, so a damaged value under the right id stays.
No other path drops a secret. There is no `export-key` command on purpose: on
macOS it would let any same-user process print the key through the binary the
Keychain trusts.

**The plan's canary table was dropped.** The plan had a `secret_store` table
holding a key id and a sealed canary. The key id on every value already answers
the same question, with no row to keep in step.

**The Keychain only for the signed `.app`** (`key_store::is_in_app_bundle`).
The Keychain trusts the binary that created an item. The DMG's engine carries a
Developer ID signature that stays the same across updates (ADR 0034). The
headless tarball and dev builds are ad-hoc signed, so each upgrade or rebuild is
a new identity. The Keychain would then prompt, and a headless service cannot
answer a prompt. So those shapes use the key file.

**This reverses ADR 0153 decision 3 for the `.app` only.** ADR 0153 kept the
backup key a file for three reasons, and each is answered now:

- Linux has no Keychain: Linux keeps a key file.
- Ad-hoc builds break the Keychain's trust: only the Developer ID `.app` uses it.
- A lost Keychain item loses every backup: the user must keep the key either
  way, and Settings still reveals it.

**No key is ever overwritten.** A mint only creates, so a rival minter's key
wins and both use it. A restore or an import that replaces a different key
first parks it under `<name>.<its key id>`. Keys are named by database, and two
clusters on one machine can share a database name.

**The key directory stays out of Time Machine.** It holds the secrets key and
the backup key, so it gets the same exclusion as `.lucidos/` (ADR 0153).

**Backups carry the secrets key.** Each archive gains a `secret-store.key`
entry beside the dump, inside the archive's backup-key encryption. Restore files
it into the new workspace's key store, never into the workspace directory. An
archive from before this change has a plain dump, and its first boot seals it.

**The agent role is rebuilt at every boot.** The engine creates the role if it
is missing, mints a fresh password that lives only in engine memory, and
re-applies its grants from `SECRET_COLUMNS`. The role gets `CONNECT` on its own
database, and `CONNECT` is revoked from `PUBLIC`. It gets full table privileges
everywhere except the secret tables, where it gets column `SELECT` on the
non-secret columns and no writes. A new table or secret column gets the right
grants on the next start. The gateway's `pg_hba.conf` admits `+lucidos_agents`
by scram, and the gateway drops the role with its database.

## Consequences

**Protected against:**

- a `pg_dump`, a data directory copy, or WAL leaking secrets;
- an agent or script reading a secret through `LUCIDOS_DATABASE_URL`, by
  accident or by a casual query;
- an agent reading another workspace's database through its URL.

**Not protected against:**

- **A same-user process on Linux, the headless tarball or a dev build.** It can
  read the key file, and it can connect as `lucidos` through peer auth or the
  dev cluster's password file. ADR 0481 already states the second half.
- **A same-user process on the packaged Mac, partly.** Reading the Keychain item
  prompts the user. If they click "Always Allow", that process can read the key.
- **Secrets the engine hands out by design**: a `CRED_*` or `OAUTH_*` variable
  a subprocess names (ADR 0516), the `http_request` tool, and proxy credential
  injection.
- **The engine's memory**, and anyone holding the backup key and an archive.
- **`push_subscriptions.endpoint`**, which stays plain because a unique index
  looks it up. It cannot send a push alone: that needs the VAPID key, which is
  sealed.

**Migrations can no longer edit secret contents in SQL.** A migration sees only
ciphertext. A change to a secret's contents runs in Rust, after the key loads.

**SQL cannot compare two secrets either.** Every seal draws a fresh nonce, so
the same token seals differently each time. A write guarded by "the row still
holds this token" locks the row, opens it in Rust and compares there. The Claude
gateway's refresh and its forget-the-rejected-token path work this way.

**A new secret column must join `SECRET_COLUMNS`, with its `CHECK`, in the same
change.** A guard test scans every column named like a secret. It fails on one
found in neither the list nor its short allow-list of non-secrets.

**The VAPID key left `preferences`** for its own `vapid_keys` table, so no
generic table holds a secret.

**Key rotation is not built.** The key id on every value leaves room for it.

## Alternatives considered

**Encrypt only the backup archive (status quo).** It left every live copy of
the data, and every role that can read it, holding plain secrets.

**Postgres-side encryption (`pgcrypto`, or transparent data encryption).**
`pgcrypto` puts the key in the query text, where logs and `pg_stat_statements`
see it. Stock Postgres has no transparent data encryption, and it would not
stop a role with `SELECT` anyway.

**Linux Secret Service (GNOME Keyring, KWallet).** A headless `systemd --user`
service has no unlocked session keyring to rely on, so the fallback would be a
file anyway. One file path is better than two paths for one secret.

**A 1Password key store.** Declined in its own design thread. The service
account token would sit on the same machine and fetch the key, so it adds no
protection there. Every boot would also need the network.

**Revoke the secret columns from a read-only `lucidos` URL instead of a new
role.** Column privileges bind roles, not connections, and the engine needs
those columns. Only a second role can hold fewer privileges.
