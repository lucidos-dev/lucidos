# 0161: A credential's scope is a set of base URLs, and every member is exact

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

ADR 0144 made a credential's `base_url` a binding, and ADR 0157 put the check at
one chokepoint. Both wrote the scope as a single value, because the credential
row held one.

**Some providers split one key across several hostnames.** A Binance HMAC key
pair signs spot calls at `api.binance.com` and futures calls at
`fapi.binance.com`. Helius issues one API key for two hosts. Two upstreams need
two `apis.json` entries naming one credential, and the gate then refuses every
call through the second one:

```
credential 'binance' is scoped to https://api.binance.com
and will not be sent to https://fapi.binance.com
```

**The shape could not be expressed at all**, so this was not a setting anybody
had got wrong. `credential_base_url_matches` compares one recorded URL, and the
column held one. ADR 0144's safety valve could not help either:
`infer_scope_if_empty` fills only an empty column, and these rows all had a
value.

**There was no reachable recovery path.** Settings offered one Base URL field
and the `lucidos` CLI offered no credential surface at all. The LLM tool that
touches a credential only requests a new one. So the only fix was an `UPDATE` in
Postgres.

Found while fixing the other half of the same gate, in the session recorded by
ADR 0157's amendment. That session flagged it rather than widening a scope
silently.

## Decision

**1. A credential carries a set.** `credentials.base_url TEXT` becomes
`base_urls TEXT[]`. `credential_scope_covers` asks whether ANY member covers the
outbound URL, and each member is judged by the unchanged
`credential_base_url_matches`: same scheme, host, effective port, and a path
prefix on a segment boundary.

**2. Nothing is inferred from the spelling of a host.** No wildcard, no suffix
match, no registrable-domain rule, and no opt-in flag for one. The user names
`https://api.binance.com` and `https://fapi.binance.com`. So a second member
widens a credential by exactly one host.

**3. The migration writes at most one member per row.** A scoped row becomes a
one-member set. A blank one becomes an empty set, which is still refused
everywhere. `20260829132711_credential_scope_is_a_set.sql`.

**4. The startup inference stays single-valued.** `main.rs` already declines
when two `apis.json` entries name one credential with different upstreams, and
it keeps declining. It also never appends to a non-empty set.

**5. The field is `base_urls` in every layer.** The column, the Rust structs,
the wire body, the TypeScript type and the CLI flag. "Credential scope" stays
the prose noun the glossary defines. `scopes` is not used, because an
`oauth_client` row already carries OAuth `scopes`.

**6. Two surfaces edit it, and both are the user's.** Settings renders the field
as a list, one row per host. `lucidos credentials list` and
`lucidos credentials set-base-urls` are the script and coding-agent route, over
a narrow `PUT /api/v1/credential-base-urls` that changes the scope and nothing
else.

**7. A create or update body may still carry a singular `base_url`.** It lands
as a one-member set. Permanent back-compat, not a temporary measure.

## Rationale

**A set is the smallest change that makes the real shape sayable.** The gate was
right and its data model was too narrow. Every alternative below either loosens
what a member means or leaves the user with no way to say what they want.

**Exactness is the whole value of the gate.** `apis.json` is writable over the
API, so the outbound `base_url` is caller data. A suffix rule turns one declared
host into an open-ended family, and an attacker who can write `apis.json` picks
a member of that family. The user gains nothing they cannot get by naming the
second host, and they lose the property that a scope says what it covers.

**Refusing to infer a multi-scope at boot is the same reasoning.** Filling both
hosts from two `apis.json` entries would derive a widening from the exact file
the gate defends against. Trust on first sight is defensible for an EMPTY scope,
where the alternative is a credential that works nowhere. It is not defensible
for adding a host to one that already works. The user now has a button and a
command, so declining costs one deliberate action instead of a hand edit.

**The recovery path is half the fix.** Severity here came as much from being
unrecoverable as from being refused. A gate the user cannot adjust is a gate
they route around, and the way around this one was to store the key twice.

**`base_urls`, not `scopes`.** Same-root-per-concept is the naming rule, and the
concept's name is *credential scope*. But `scopes` is already taken in this
exact neighbourhood: an `oauth_client` credential's `auth_value` carries OAuth
`scopes`, and `connect_oauth_account` takes a `scopes` argument. A
`credential.scopes` beside `credential.auth_value.scopes` would read as one
thing. The field names what it holds, and the glossary entry carries the
concept.

**Narrow verb for the CLI write.** The whole-row edit body names the auth type
and defaults the auth header. A script widening a scope through it therefore
clobbers fields it never meant to touch. `set-base-urls` also replaces rather
than appends. The command therefore states the resulting set, and a reader of
the shell history sees what the credential now covers.

## Consequences

- **An affected workspace has to act once, and now can.** The upgrade does not
  widen anything, so a `binance-futures` entry keeps answering 502 until the
  user adds the host in Settings or with the CLI. The refusal names the whole
  declared set and the command that changes it.
- **`find_by_url` ranks on the matching member, not the row.** A credential
  holding many hosts must not out-rank a narrower one just for holding a longer
  set. Ties keep the first row in name order, so the answer is stable.
- **A git clone is offered the credential at every host it names.**
  `StoredGitCredential::entries_from_credential` yields one entry per member,
  and the existing longest-first match is unchanged.
- **A member that is not a URL with a host is refused at the write.** Stored, it
  could only ever be refused at the gate, as a 502 far from the form it was
  typed into.
- **An empty set stays a legal, fail-closed state.** A `secret` carries it, and
  so does a row the startup pass has not scoped. It is refused everywhere.
- **The handshake script's own scope is untouched.** The `base_url` column of
  `.lucidos/approved-handshake-scripts` binds a minted token rather than a
  credential, and stays single-valued. ADR 0157's consequence about a script
  serving two hosts still holds.
- **`ScopeBinding::Pinned` stays single-valued**, since the engine chose that
  upstream from an input no API caller can write.
- **Editing a scope is still app-reachable, and always was.** An app UI is
  same-origin, so it can call the credential routes exactly as the user can.
  ADR 0156 states why no app principal separates them, and this change adds no
  lever an app did not already have through `PUT /api/v1/credentials`.

## Alternatives considered

- **Suffix or wildcard matching on the host, off by default.** Rejected under
  decision 2. It buys one keystroke over naming the second host and gives up the
  property the gate exists for. `*.binance.com` also covers hosts the provider
  has not created yet, which nobody reviewed.
- **Same registrable domain is close enough.** Rejected, and worse than the
  wildcard: it is implicit, it needs a public-suffix list to be correct at all,
  and on a shared-domain provider it is a very wide grant.
- **Keep one `base_url` and let the user store the key twice.** Rejected. It is
  what a user does today to get around this, and it doubles the rotation work
  while leaving two rows that can drift.
- **A `credential_scopes` join table.** Rejected. A short ordered list per row is
  what an array column is for. A second table adds a join to every credential
  read, for nothing this change needs.
- **Keep `base_url` and add `extra_base_urls`.** Rejected. Two columns answer one
  question, which is the drift the credential name prefixes already taught us
  about, and every reader would have to consult both.
- **Widen the boot inference to fill both hosts.** Rejected under decision 4. It
  derives a widening from `apis.json`, which is the file the gate defends
  against.
- **Put the CLI surface through the capability parity manifest.** Rejected. That
  generates an LLM tool and an SDK namespace alongside the command, and neither
  has a caller. `lucidos handshake` is the precedent for a hand-written,
  CLI-only security verb.
- **Drop the singular `base_url` from the request bodies.** Rejected. It would
  break a script or app a user already wrote, for a field the engine can accept
  in one place at no ongoing cost.
