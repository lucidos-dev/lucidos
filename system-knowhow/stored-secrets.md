---
name: Stored secrets
description: Where secrets live: encrypted in the database, the key in the Keychain or a key file, hidden from your database URL. Load for "are my credentials encrypted", "is the secret in the Keychain", or whether a backup carries the key.
---

# Stored secrets

Every secret Lucidos stores sits **encrypted in the workspace database**. The
key that opens them sits **outside** the database. Neither half is useful
alone: a database dump, a copy of the data directory or a stolen backup without
its key shows only ciphertext.

## What is encrypted

These columns hold only encrypted values:

| Table | Column | Holds |
|---|---|---|
| `credentials` | `auth_value` | every *credential* value: API keys, tokens, passwords, OAuth client secrets |
| `oauth_accounts` | `access_token`, `refresh_token` | connected account tokens |
| `claude_gateway_accounts` | `access_token`, `refresh_token` | Claude gateway sign-in tokens |
| `mcp_servers` | `env` | each MCP server environment value |
| `vapid_keys` | `private_key_pem` | the Web Push signing key |
| `push_subscriptions` | `auth` | each device's push secret |

An encrypted value is text shaped `lsec1:<key id>:<ciphertext>`, using
AES-256-GCM. Each of these columns has a `CHECK` constraint that refuses
anything else, so Postgres itself rejects a plain secret.

`credentials.auth_header` is NOT a secret. It holds the header *name*, such as
`Authorization`.

## Where the key lives

One 32-byte key per workspace database, named after the database
(`lucidos_<workspace>`):

- **The signed macOS app** keeps it in the **Keychain**: service
  `Lucidos workspace secrets`, account `lucidos_<workspace>`.
- **Everything else** (Linux, the headless install, a dev build) keeps it in a
  key file, `~/.lucidos/secret-keys/lucidos_<workspace>.key`, readable by its
  owner only.

The Keychain holds the **key**, not the secrets. The secrets stay in the
database.

The engine refuses to start when the Keychain or key file lacks the key the
database's values name. So a running engine proves its key matches.

## Why you cannot see them

`LUCIDOS_DATABASE_URL` signs in as the workspace's agent role,
`lucidos_<workspace>_agent`. That role has no `SELECT` on the secret columns.
Postgres hides columns a role cannot read, so `information_schema.columns`
does not even list `credentials.auth_value`. **A missing column does not mean
the secret is stored elsewhere.** It means this role cannot see it.

So never conclude "the secrets are not in the database" from the agent role's
view. Check what the agent role CAN see instead:

```sh
# Every secret column carries a CHECK that requires the encrypted form
psql "$LUCIDOS_DATABASE_URL" -Atc "select conrelid::regclass, conname
  from pg_constraint where contype = 'c'
  and pg_get_constraintdef(oid) like '%lsec1:%' order by 1"

# The agent role cannot read the credential values (expects f)
psql "$LUCIDOS_DATABASE_URL" -Atc "select has_column_privilege(current_user,
  'credentials', 'auth_value', 'SELECT')"

# The key is in the Keychain (macOS app). Attributes only: never pass -w
security find-generic-password -s "Lucidos workspace secrets" \
  -a "$(psql "$LUCIDOS_DATABASE_URL" -Atc 'select current_database()')"
```

Confirming each stored value's `lsec1:` prefix needs the database owner, which
no agent gets. The `CHECK` constraints already guarantee it.

## Backups carry the key

Every backup archive holds the key as `secret-store.key`, beside the database
dump and inside the archive's encryption (see `system-knowhow/backups`). A
restore files it in the Keychain or key file under the **restored** database's
name, never in the workspace folder.

To prove a backup really carries the key on the same machine, restore it under
a **new workspace name**. The new database gets its own Keychain item or key
file, filled only from the archive. If the archive lacked the key, the new workspace
would refuse to start. A restore over the same name proves nothing, because
the old key is already there.

## What this does not protect

- A script that names a credential (`CRED_*`, `OAUTH_*`) gets its plain value,
  by design.
- Proxy requests send the credential to the base URLs it covers.
- On the packaged Mac, another program can read the Keychain item only after
  the user approves the Keychain prompt. Elsewhere, any program running as the
  same user can read the key file.
