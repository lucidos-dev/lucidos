# 0522: Credential secrets may be 1Password references, resolved through `op` on use

- **Status**: Accepted
- **Date**: 2026-10-18
- **Builds on**: [0509](0509-stored-secrets-are-sealed-at-rest.md),
  [0516](0516-subprocesses-get-only-the-secrets-they-name.md)

## Context

Users who keep their secrets in 1Password had to paste a copy into Lucidos.
The copy then lived in two places, and a rotation in 1Password left the Lucidos
copy stale.

User-entered secrets also lived in three tables: `credentials`,
`email_accounts.password` and `mcp_servers.env`. A reference has to work in one
place, so that had to come first.

Plan: `docs/plans/2026-10-17-public-key-backups-and-1password-references.md`,
decisions 3 and 4.

## Decision

1. **Every secret a user enters lives only in `credentials`.** IMAP and SMTP
   read the `email_password` credential, and an MCP env entry may name a
   credential. A boot step moves the old values and drops
   `email_accounts.password`.
2. **A credential's secret field holds a stored value or a *secret reference*,**
   `op://vault/item/[section/]field`, never both. The reference sits inside the
   sealed `auth_value` payload, so ADR 0509 applies unchanged.
3. **The engine resolves a reference through the `op` command line tool**, with
   a 1Password service account token, only when a consumer uses that
   credential.
4. **The service account token reaches only the `op` child.** It is stored as
   an `onepassword_service_account` credential, and no injection path returns
   it.
5. **The service account has its own host-only routes,**
   `GET|PUT|DELETE /api/v1/onepassword` and `POST /api/v1/onepassword/test`.
   `GET` never returns the token. The other three take only the owner at a
   registered device, and the generic credential routes refuse the type.

## Rationale

**Why `op`.** Service accounts have no REST API. They work only through `op`
and the Go, JavaScript and Python SDKs. The only REST API is a self-hosted
Connect server, which few users run.

**`op` is not an OS-specific command.** The engine already runs external tools
(`git`, `pg_dump`, `claude`), and `op` is optional and cross-platform. So it is
not the kind of command `CLAUDE.md` bans. Resolution sits behind a
`SecretResolver` trait, so a Connect backend can follow.

**A cleared environment and a timeout.** The child gets
`OP_SERVICE_ACCOUNT_TOKEN`, `PATH` and `HOME` only, and 20 seconds. Its stderr
is masked before any reason quotes it, and the token's `ops_` shape is in
`TOKEN_SHAPES`.

**Lazy, per use.** Under ADR 0516 a subprocess gets only the credentials its
text names. So a reference resolves only when a spawn names it, and a
1Password outage touches only those spawns.

**The cache tells the two failures apart.** A resolved value stays in engine
memory for 10 minutes, never on disk, in the database or in a log. A transport
failure (no `op`, timeout, network) serves the last good value for up to 24
hours. A definitive failure (not found, access denied) drops the value at once,
because a deleted secret must not live on. A missing service account is
definitive, and any credential write clears the cache.

**It never fails open.** A webhook whose signing secret cannot resolve answers
503, so the sender retries. A call never goes out without its credential. The
user gets one notice per credential per outage, naming the reason.

**Saving resolves once,** so a typo never lands. Revealing a referenced field
shows the reference, never the value.

**The `op` path lives inside the sealed service account payload,** not in a
preference. The agent's database role can write `preferences`, and whoever
picks the binary receives the token.

**The service account has its own routes.** A change to the `op` path then
keeps the stored token, so the token never travels back to the browser. A `PUT`
with a blank token keeps the stored one.

**Only the owner writes or uses the service account.** An agent that could set
the `op` path would hand the stored token to its own binary. One that could
swap the account would choose the value behind every reference. So the routes
use `require_owner_device`, as thread deletion does (ADR 0192), and refuse an
app document.

**The column drop runs from a boot step, not a migration.** sqlx runs every
migration before the secrets key loads, and a migration sees only ciphertext.
Its `table.column` associated data also stops SQL from copying a sealed value
across columns. The boot step that runs `VALIDATE CONSTRAINT` on ADR 0509's
sealed columns is the precedent. An old
dump restored later moves and drops the same way on its first boot.

## Consequences

- A credential can follow a 1Password rotation with no edit in Lucidos.
- A referenced secret depends on 1Password being reachable within 24 hours.
- Tokens Lucidos mints itself (OAuth, push, VAPID) stay where they are. They are
  not user-entered.
- A hosted Lucidos ships `op` in its image, with one sealed service account per
  workspace.
- A moved MCP env value is a credential named `mcp-<server id>-<VAR>`.
  Removing the server, or a re-register that drops the pointer, deletes it,
  unless another server still names it. A credential with any other name
  stays. Ownership is the name alone, so a hand-made credential with that
  name goes too.

## Alternatives considered

**The 1Password desktop app integration (Touch ID).** Rejected. An unattended
engine cannot answer the prompt.

**A Connect server backend.** Deferred. The `SecretResolver` trait leaves room.

**Resolve every reference at boot, or on every spawn.** Rejected. It runs `op`
for credentials nobody uses, and turns a 1Password outage into a boot or a
general spawn failure.

**Keep the resolved value next to the reference.** Rejected. It would put the
plaintext back in the database, which is the copy references exist to remove.

**Store the `op` path in a preference.** Rejected, for the reason under
Rationale.

**References to other password managers.** Deferred.
