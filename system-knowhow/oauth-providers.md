---
name: OAuth Providers Registry
description: OAuth 2.0 provider rules, beside the endpoint rows in the sibling oauth-providers.json: connect_oauth_account, the oauth_client credential, the loopback redirect URI, PKCE vs a client secret, and Microsoft Entra AADSTS errors. Load before collecting an OAuth credential, when a connection fails, or to add a provider.
---

# OAuth providers registry

The **rows** live in the sibling data file
[`oauth-providers.json`](oauth-providers.json); this file is the prose that goes
with them. The engine reads the JSON (`core/oauth_registry.rs`) to prefill the
Connect form on **Settings → Accounts**, so a known provider asks only for a
Client ID. It also fills in any endpoint you did not pass to
`connect_oauth_account`. No provider is hardcoded, so adding one is a data edit.
You read the same rows, and you keep them up to date.

## How it's used

When a service needs OAuth client credentials:

1. Call **`connect_oauth_account`** with the provider name and the scopes. For a
   provider in [`oauth-providers.json`](oauth-providers.json), that is all it
   needs: the engine fills in the endpoints from the row. With no client
   credentials yet, it opens the credential modal, prefilled. The user enters
   only `client_id`, plus `client_secret` for a confidential/web client. Once
   the client is saved, the same call runs the authorization.
   **Do not call `request_credential(auth_type: "oauth_client")` first.** It is
   a second modal for the same value, and it once produced a duplicate credential.
2. For a provider the registry does **not** know, pass the endpoints yourself:
   `auth_url`, `token_url`, `userinfo_url`, plus `userinfo_method`,
   `authorize_params` or `redirect_uri` where the provider needs one. What you
   pass wins over the registry. That is also how a *derived provider* name runs
   on a known provider's endpoints (see the alias rule below).
3. The credential's JSON stores the values
   (`{client_id, client_secret?, auth_url, token_url, userinfo_url,
   userinfo_method?, authorize_params?, redirect_uri?}`). It is the
   **per-credential source of truth** for endpoints. Token refresh and
   re-authorization read them back from there and never consult the registry
   again: a credential fully describes its own flow. `client_secret` and
   `redirect_uri` are optional; the two sections below say what their absence
   means.

**Read the result; do not assume it all worked.** A provider can grant only part
of the request and still complete the authorization. `connect_oauth_account`
then reports the account as connected AND names each scope it did not get. The
connection is real but cannot do its job. Relay the missing scopes, have the
user enable them in the provider's console (the result carries its link and
instruction), then RECONNECT. Do not retry the same call first, and do not
refresh the token: neither picks up a newly enabled scope.

A provider in neither the registry nor your args leaves the user typing URLs by
hand, so do that only when you cannot find it. **Better: find it via
`web_search` and add a row to the JSON**, so the next connection is one step,
for you and for the Connect button.

### The credential is named for the provider, and typed `oauth_client`

An OAuth client registration is identified by its provider name **plus** its
auth type. Pass `dropbox` to `request_credential` with
`auth_type: "oauth_client"` and it is stored as exactly `dropbox`. The Add
Credential form in Settings > Accounts does the same.

The type marks it, so the name needs no namespace. The same provider can hold
two credentials: a plain `dropbox` API key and the `dropbox` app registration
are different rows. The OAUTH CLIENT badge in the list tells them apart, and
neither shadows the other.

When you tell the user which credential holds their client:

- Call it **the OAuth Client credential for `<provider>`**. In the list it shows
  the bare provider name with an OAUTH CLIENT badge and the note "App
  registration for the `<provider>` connected account".
- Never tell them to name it `oauth:<provider>`. That old storage key no longer
  exists. If you pass it anyway, the engine strips the prefix: you land on the
  right row, but the user sees a name they did not type.
- An old `oauth:<provider>` row that still shows means another credential held
  its unprefixed name when the rename ran. Have them check which of the two is
  live, delete the dead one, and re-save the survivor.

### Redirect URI

The provider's OAuth app must whitelist the exact loopback redirect URI Lucidos
sends. The **default** is the loopback-IP form:

```
http://127.0.0.1:14981/oauth/callback
```

Some providers (e.g. Spotify) reject `localhost` but accept the `127.0.0.1`
loopback IP, so the IP is the default. Others do the opposite: **the Microsoft
Entra portal's Redirect URIs box refuses `http://` + `127.0.0.1`** and accepts
only `https://…` or `http://localhost…`.

So the URI is **overridable per credential**. When the provider needs a
different host form, pass `redirect_uri` to `request_credential` /
`connect_oauth_account`. It pre-fills the modal, and the user can edit it. Only
these three values are accepted. The engine's listener owns the port and path,
and binds **both** loopback families, so all three work:

| Redirect URI | Use it when |
|---|---|
| `http://127.0.0.1:14981/oauth/callback` | **Default.** Omit `redirect_uri` to get this. |
| `http://localhost:14981/oauth/callback` | The provider rejects the IP literal (Microsoft's Web platform). |
| `http://[::1]:14981/oauth/callback` | Only if a provider demands the IPv6 literal. |

Anything else (a different port, a different path, a trailing slash, `https`)
is rejected when the flow starts, with an error that lists these three. Tell the
user to register the URI **exactly**, character for character.

### Confidential vs public client

Lucidos picks the OAuth client type from **one thing: whether the credential has
a `client_secret`.** There is no provider list for this.

| Credential | Lucidos sends | Register the app as |
|---|---|---|
| `client_secret` filled in | the secret, no PKCE | a **web / confidential** app |
| `client_secret` left blank | no secret, PKCE (`S256`) | a **desktop / native / public** app |

Both are correct, but they must match how the app is registered. Providers
reject a secret from a public client *and* a secret-less redemption from a
confidential one. Lucidos runs on the user's own machine, so the desktop/public
shape (RFC 8252) fits best when the provider offers it. Then tell the user they
can leave the client secret blank.

## Alias rule: dedicated connections

Some APIs reject an access token that *also* carries unrelated scopes. Google's
Health API, for example, 403s ("Request contains disallowed OAuth scope(s)") any
token that also holds calendar / drive / docs / fitness scopes. The fix is a
**dedicated connection under a distinct provider name** that requests only the
narrow scopes, on the **base provider's endpoints**.

A derived name is **not** in the registry and must not be added: aliases are ad
hoc, one per user need. So the engine cannot resolve one, and does not guess a
base provider from the spelling. Read the base provider's row from
[`oauth-providers.json`](oauth-providers.json) and pass its endpoints
explicitly, under the distinct name, so the token is stored separately:

```
connect_oauth_account(
  provider="<derived-name>",
  scopes="<the narrow scopes only>",
  auth_url="<base provider's auth_url>",
  token_url="<base provider's token_url>",
  userinfo_url="<base provider's userinfo_url>",
  base_url="<the narrow API's own base URL>")
```

`configure_email` reads a connection's issuer from its stored `token_url`, so an
alias on Google's or Microsoft's endpoints can back an email account.

Match a derived name to its base by what the connection is for, or ask the user.
The Connect button on **Settings → Accounts** does the same: type a name the
registry does not know and it asks which known provider the connection runs on,
then prefills from that row.

## Known providers

**The rows live in [`oauth-providers.json`](oauth-providers.json).** Read it for
a provider's `auth_url`, `token_url`, `userinfo_url`, `userinfo_method`,
`authorize_params`, `base_url`, and its console fields (`console_url`,
`client_type`, `setup_hint`, `permissions_hint`).

This file restates **no** row, and `the_knowhow_markdown_restates_no_registry_row`
fails the build if one comes back: two copies of an endpoint can disagree. Below
is what the optional columns mean, and the per-provider quirks no value can
express.

### The `userinfo_method` field

`userinfo_url` is what makes a *connected account* show **whose** account it is.
Without it, the account lists as "No email" and the connect tool reports it as
unnamed.

Almost every provider serves userinfo over **GET**, the default. Pass
`userinfo_method` only for an exception the provider's row names. Dropbox is
one: `users/get_current_account` is POST-only (Lucidos sends POST with no body
and no `Content-Type`, the shape Dropbox accepts). Dropbox also nests the
display name as `name.display_name` rather than a flat `name`; Lucidos reads
both shapes.

A wrong method costs only the account's name and email. The connection still
works, because Lucidos fetches userinfo best-effort after the token exchange
succeeds.

### The `authorize_params` column

Every provider spells *"issue a refresh token"* its own way, and a wrong
spelling stays invisible for hours. A token with no refresh token cannot be
renewed, so `refresh_oauth_if_needed` can only report *"OAuth token expired but
no refresh token available"*. Everything works on the day of connecting, and
nothing works the next morning.

The default is Google's: `access_type=offline&prompt=consent`. Lucidos sends it
whenever the row says _(default)_ or the field is blank. Pass an explicit value
only where the provider's row gives one, and pass it **verbatim**. An explicit
value REPLACES the default rather than adding to it, so Lucidos sends exactly
what the row says.

- **Dropbox** needs `token_access_type=offline`. Google's two parameters do
  nothing for it, so with the default a Dropbox connection gets a four-hour
  access token and no refresh token.
- The value is `key=value&key=value`. Percent-encode a value that itself
  contains `&` or `=`.
- The flow owns `client_id`, `redirect_uri`, `response_type`, `scope`, `state`,
  `code_challenge` and `code_challenge_method`, and refuses any of them here.
  Set `redirect_uri` on the credential, and pass scopes to
  `connect_oauth_account` when you connect. `state` is generated per
  authorization and must come back on the callback. A pinned value would break
  the callback (see "One authorization at a time" below).
- Write `none` for a provider that rejects a parameter it does not recognize.
  That sends neither default.
- A migration backfilled `token_access_type=offline` on each **Dropbox client
  connected before this column existed**. A reconnect from Settings → Accounts
  then renews correctly with no edit. Any other provider that needs a value must
  be set by hand (or by you, on the credential) before reconnecting.

### One authorization at a time

The callback listener binds a **fixed** loopback port (14981), because the
redirect URI must be registered with the provider ahead of time. So a workspace
engine can have **at most one authorization in flight**, whatever the provider.
That holds whether `connect_oauth_account` started it or a Connect / Reconnect /
*Grant access* button did.

Two consequences:

- **Starting a new authorization cancels the previous one.** If the user
  abandoned a consent screen (or never finished a flow you started), the next
  one supersedes it rather than failing. The abandoned flow reports *"This
  authorization was canceled, most likely because a newer one was started"*.
  That is expected, not an error to chase.
- **A port clash that survives that is another program.** Almost always it is a
  second Lucidos workspace part-way through connecting an account. Workspaces
  run concurrently, each with its own engine, and share this one machine-wide
  port. The error says so. Finish or abandon the other one; this credential
  needs no fix.

Each authorization also carries its own random `state`, and the listener ignores
any callback that does not echo it back. So nothing can redeem a stale redirect
from a superseded flow, or a request from anything else that reaches the
loopback port. There is nothing to configure; this is why `authorize_params`
refuses `state`.

### Notes on scopes

- **Google**: scopes are full URLs (`https://www.googleapis.com/auth/<api>`).
  `openid email profile` are also valid. `userinfo_url` may instead be
  `https://openidconnect.googleapis.com/v1/userinfo` if you request `openid`.
- **Microsoft**: scopes look like `https://graph.microsoft.com/Mail.Read` or
  short names like `offline_access User.Read`. Include `offline_access` to get a
  refresh token. The token response will not echo it back, so the refresh token
  is what proves it was granted (see "what the token response echoes" below).
  Its app registration also needs extra care.
- **GitHub**: scopes are short names (`repo read:user`). GitHub tokens do not
  expire and have no refresh token. That is expected.
- **Spotify / Dropbox**: short scope names per their docs. Dropbox's account
  scope is `account_info.read`, which its POST userinfo endpoint needs. A
  connection without it still works, but reports no email.
- **Dropbox for backups** needs four:
  `files.content.write files.content.read files.metadata.read account_info.read`.
  Write covers the folder create, the upload and the retention delete. Read
  covers restoring. Metadata covers the backup listing that drives pruning and
  the health card. Request all four. Dropbox grants no scope the app itself is
  not permitted, so see the App Console section below.

### Dropbox: the App Console decides what may be asked for

In Dropbox, enabling a scope on the app is a separate step from requesting it.
Every way to get it wrong looks identical to the user, so walk them through all
three:

1. **The Permissions tab of their app in the Dropbox App Console is the maximum
   AND the default set.** An authorization request can narrow that set, never
   widen it. A call that needs a scope the app is not permitted fails with
   *"Your app … does not have the required scope"*.
2. **A ticked box is not saved until they press Submit.** The tab keeps the
   ticks as unsaved page state, and its **Submit** button sits at the bottom,
   below the fold. It is easy to tick four boxes, navigate away, and change
   nothing. Have them scroll down and press Submit, then reload the tab and
   confirm the ticks survived.
3. **Ticking a box changes nothing that already exists.** Neither an issued
   access token nor the user's existing grant picks up a newly enabled scope.
   After a permissions change they MUST reconnect the account from
   Settings → Accounts (the Backup page's *Grant access* button does the same
   for a backup provider). A refresh does not help: it renews only the scopes
   the token already has.

So the order is: enable the permissions in the App Console, press Submit, then
connect. If they connected first: enable, Submit, then reconnect. "Tick the box"
alone leaves them looking at the same error.

## Microsoft (Entra): redirect URI platform buckets

Entra does not store one flat list of redirect URIs. Each one lives in a
**platform bucket**, and the bucket decides which client type may redeem a code
with it:

| Platform in the portal | Client type | Redemption |
|---|---|---|
| **Web** | confidential | must send `client_secret` |
| **Mobile and desktop applications** | public | must send **no** secret; PKCE instead |
| Single-page application | public + CORS | not what Lucidos is |

`/authorize` accepts a URI from **any** bucket, so a wrong bucket authorizes
fine and fails only at the token exchange. Two symptoms, one cause:

- **`AADSTS90023`, `invalid_request`: *"The provided value for the input
  parameter 'redirect_uri' is not valid"*, after the browser already said
  "Authorization successful!"** The URI sits in a bucket that does not match
  the client type in use. Classic case: the URI is under *Mobile and desktop
  applications* while Lucidos sends a `client_secret`.
- **`AADSTS50011`, a redirect-URI mismatch at the consent screen.** The string
  matches nothing registered. Usually it is the `127.0.0.1` ↔ `localhost`
  difference, because the portal keeps the IP form out of the Web bucket.

Pick one of the two coherent setups and make both halves agree:

**Desktop / public (recommended, since Lucidos runs on the user's machine):**
1. Portal → *Authentication* → add a **Mobile and desktop applications**
   platform with `http://127.0.0.1:14981/oauth/callback`.
2. Set *Allow public client flows* to **Yes**.
3. In Lucidos, leave **Client Secret blank**, and omit `redirect_uri` (the
   default IP form is what's registered).

**Web / confidential:**
1. Portal → *Authentication* → add a **Web** platform with
   `http://localhost:14981/oauth/callback` (the box rejects the `127.0.0.1`
   form: a portal limitation, not a protocol one).
2. Create a client secret under *Certificates & secrets*.
3. In Lucidos, enter the **Client Secret** and pass
   `redirect_uri="http://localhost:14981/oauth/callback"`.

Do **not** register the same callback in both buckets. Entra picks one
arbitrarily when URIs differ only by bucket, so the failure turns intermittent.

## Microsoft (Entra): what the token response echoes

A token issued for a **resource** (`https://outlook.office.com/…`,
`https://graph.microsoft.com/…`) comes back with a `scope` naming that
resource's own scopes and nothing else. `offline_access`, `openid`, `profile`
and `email` are never in that list, however the consent went.

So **a scope absent from the echo is not a refused scope**. For
`offline_access`, the refresh token is the authoritative signal: if one was
issued and stored, the scope was granted. Lucidos reads it that way on the
account card and in the `connect_oauth_account` result. It warns only when the
refresh token is truly absent, the case that breaks renewal.

Do not send a user to the Entra portal over an unechoed `offline_access`.

A real refusal still surfaces. Ask for a **resource** scope the app
registration lacks and it is missing from the echo, where nothing else vouches
for it.

## Adding a new provider

Adding a known provider is a **knowhow edit, not an engine change**:

1. `web_search` for the provider's OAuth 2.0 authorization + token endpoints
   (and userinfo endpoint if it has one).
2. Add a row to `oauth-providers.json` with its `base_url` and, so the Connect
   button can help too, its `console_url`, `client_type` and `setup_hint`.
3. Note any scope quirks under "Notes on scopes".

The next `request_credential` / `connect_oauth_account` for that provider then
pre-fills from your new row.
