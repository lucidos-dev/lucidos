---
name: OAuth Providers Registry
description: OAuth 2.0 provider rules, beside the endpoint rows in the sibling oauth-providers.json: connect_oauth_account, the oauth_client credential, the loopback redirect URI, PKCE vs a client secret, and Microsoft Entra AADSTS errors. Load before collecting an OAuth credential, when a connection fails, or to add a provider.
---

# OAuth providers registry

The **rows** live in the sibling data file
[`oauth-providers.json`](oauth-providers.json). **This file is the prose that
goes with them**, and deliberately restates no row.

The engine reads the JSON (`core/oauth_registry.rs`) to prefill the Connect form
on **Settings → Accounts** and to fill in any endpoint you did not pass to
`connect_oauth_account`. It still hardcodes no provider: adding one is a data
edit. You read the same rows, and you keep them up to date.

## How it's used

When a service needs OAuth client credentials:

1. Call **`connect_oauth_account`** with the provider name and the scopes. For a
   provider in [`oauth-providers.json`](oauth-providers.json) that is all it
   needs: **the engine fills the endpoints in from the row itself.** That one
   tool covers the whole flow: with no client credentials yet it opens the
   credential modal (prefilled, so the user enters only `client_id`, plus
   `client_secret` for a confidential/web client), and once the client is saved
   the same call runs the authorization.
   **Do not hand-roll a `request_credential(auth_type: "oauth_client")` call
   first.** It is a second modal for the same value, and that extra step is what
   produced a duplicate credential on 2026-08-05.
2. For a provider the registry does **not** know, pass the endpoints yourself:
   `auth_url`, `token_url`, `userinfo_url`, and `userinfo_method` /
   `authorize_params` / `redirect_uri` where the provider needs one. Anything you
   pass wins over the registry, so this is also how a *derived provider* name
   (see the alias rule below) runs on a known provider's endpoints.
3. The values are stored in the credential's JSON
   (`{client_id, client_secret?, auth_url, token_url, userinfo_url,
   userinfo_method?, authorize_params?, scopes, redirect_uri?}`), which is the
   **per-credential source of truth** for endpoints. Token refresh and
   re-authorization read them back from there, and the registry is not consulted
   again: a credential fully describes its own flow.
   `client_secret` and `redirect_uri` are optional, see the two sections below
   for what their absence means.

**Read the result, do not assume it means everything worked.** A provider can
grant part of what was asked for and still complete the authorization, so
`connect_oauth_account` reports the account as connected AND names any scope it
did not get. When it does, the connection is real but cannot do the thing it was
connected for: relay the missing scopes, have the user enable them in the
provider's console (the result carries that provider's own console link and
instruction), and then RECONNECT. Do not retry the same call first, and do not
refresh the token: neither picks up a newly enabled scope.

A provider that is in neither the registry nor your args leaves the user typing
URLs in by hand, so only do that for one you genuinely cannot find. **Better:
find it via `web_search` and add a row to the JSON** so the next connection, for
you and for the Connect button alike, is one step.

### The credential is named for the provider, and typed `oauth_client`

An OAuth client registration is identified by its provider name **plus** its
auth type. Pass `dropbox` to `request_credential` with
`auth_type: "oauth_client"` and it is stored as exactly `dropbox`. Same for the
Add Credential form in Settings > Accounts.

The type is what marks it, so the name needs no namespace of its own. That also
means the same provider can legitimately hold two credentials: a plain `dropbox`
API key and the `dropbox` app registration are different rows, distinguished by
the OAUTH CLIENT badge in the list, and neither shadows the other.

So when you tell the user which credential holds their client:

- Call it **the OAuth Client credential for `<provider>`**. In the list it shows
  the bare provider name with an OAUTH CLIENT badge and the note "App
  registration for the `<provider>` connected account".
- Never tell them to name it `oauth:<provider>`. That was the storage key until
  2026-08-05 and no longer exists; if you pass it anyway the engine strips the
  prefix, so you land on the right row but the user sees a name they did not type.
- If they have an old `oauth:<provider>` row still showing, its unprefixed name
  was already taken by another credential when the rename ran. Have them check
  which of the two is live, delete the dead one, and re-save the survivor.

### Redirect URI

The provider's OAuth app must whitelist the exact loopback redirect URI Lucidos
will send. The **default** is the loopback-IP form:

```
http://127.0.0.1:14981/oauth/callback
```

Some providers (e.g. Spotify) reject `localhost` but accept the `127.0.0.1`
loopback IP — that's why the IP is the default. Others do the opposite: **the
Microsoft Entra portal's Redirect URIs box refuses `http://` + `127.0.0.1`** and
accepts only `https://…` or `http://localhost…`.

So the URI is **overridable per credential**. Pass `redirect_uri` to
`request_credential` / `connect_oauth_account` (it pre-fills the modal, and the
user can edit it) when the provider needs a different host form. Only these
three values are accepted — the engine's listener owns the port and path, and
binds **both** loopback families so all three genuinely work:

| Redirect URI | Use it when |
|---|---|
| `http://127.0.0.1:14981/oauth/callback` | **Default.** Omit `redirect_uri` to get this. |
| `http://localhost:14981/oauth/callback` | The provider rejects the IP literal (Microsoft's Web platform). |
| `http://[::1]:14981/oauth/callback` | Only if a provider demands the IPv6 literal. |

Anything else — a different port, a different path, a trailing slash, `https` —
is rejected when the flow starts, with an error listing these three. Tell the
user to register the URI **exactly**, character for character.

### Confidential vs public client

Lucidos picks the OAuth client type from **one thing: whether the credential has
a `client_secret`.** There is no provider list for this.

| Credential | Lucidos sends | Register the app as |
|---|---|---|
| `client_secret` filled in | the secret, no PKCE | a **web / confidential** app |
| `client_secret` left blank | no secret, PKCE (`S256`) | a **desktop / native / public** app |

Both are correct; they must match how the app is registered, because providers
reject a secret from a public client *and* reject a secret-less redemption from
a confidential one. Lucidos runs on the user's own machine, so the desktop/public
shape (RFC 8252) is the more natural fit when the provider offers it — tell the
user they can leave the client secret blank in that case.

## Alias rule — dedicated connections

Some APIs reject an access token that *also* carries unrelated scopes. Google's
Health API, for example, 403s ("Request contains disallowed OAuth scope(s)") any
token that also holds calendar / drive / docs / fitness scopes. The fix is a
**dedicated connection under a distinct provider name** that requests only the
narrow scopes — but it still uses the **base provider's endpoints**.

A derived name is **not** in the registry and must not be added to it: aliases
are ad hoc, one per user need. So the engine cannot resolve one for you, and
deliberately does not try to guess a base provider from the spelling. Read the
base provider's row out of [`oauth-providers.json`](oauth-providers.json) and
pass its endpoints explicitly, under the distinct name, so the token is stored
separately:

```
connect_oauth_account(
  provider="<derived-name>",
  scopes="<the narrow scopes only>",
  auth_url="<base provider's auth_url>",
  token_url="<base provider's token_url>",
  userinfo_url="<base provider's userinfo_url>",
  base_url="<the narrow API's own base URL>")
```

Match a derived name to its base by asking what the connection is for, or by
asking the user outright. The Connect button on **Settings → Accounts** does the
same thing: type a name the registry does not know and it asks which known
provider the connection runs on, then prefills from that provider's row.

## Known providers

**The rows live in [`oauth-providers.json`](oauth-providers.json), beside this
file.** Read it for a provider's `auth_url`, `token_url`, `userinfo_url`,
`userinfo_method`, `authorize_params`, `base_url`, and its console fields
(`console_url`, `client_type`, `setup_hint`, `permissions_hint`).

They are a data file rather than a table here because the engine serves them: the
Connect form on **Settings → Accounts** reads the same rows to prefill a
provider's whole app registration, so a user connecting a known provider enters
only a Client ID. This file deliberately does **not** restate a single row, and
`the_knowhow_markdown_restates_no_registry_row` fails the build if one comes
back. Two copies of an endpoint are two things that can disagree.

Everything below is the prose that goes with those rows: what the optional
columns mean, and the per-provider quirks that are not expressible as a value.

### The `userinfo_method` field

`userinfo_url` is what makes a *connected account* show **whose** account it is.
Omit it and the account lists as "No email" and the connect tool reports it as
unnamed, which is exactly what happened to Dropbox before its endpoint was
recorded here.

Almost every provider serves userinfo over **GET**, which is the default: pass
`userinfo_method` only for the exceptions, and only when the provider's row says so.
Dropbox is one: `users/get_current_account` is POST-only (Lucidos sends POST
with no body and no `Content-Type`, the shape Dropbox accepts). Note also that
Dropbox nests the display name as `name.display_name` rather than a flat `name`;
Lucidos reads both shapes, so nothing extra is needed for that.

Getting the method wrong costs only the account's name and email. The
connection itself still works, because userinfo is fetched best-effort after the
token exchange has already succeeded.

### The `authorize_params` column

Every provider has its own spelling of *"issue a refresh token"*, and getting it
wrong is invisible until hours later. A token with no refresh token cannot be
renewed, so `refresh_oauth_if_needed` can only report *"OAuth token expired but
no refresh token available"*: everything works on the day it is connected and
nothing works the next morning. That is what happened to Dropbox backups until
2026-08-05.

The default, sent whenever this column says _(default)_ and whenever the field
is left blank, is Google's: `access_type=offline&prompt=consent`. Pass an
explicit value only where the provider's row gives one, and pass it **verbatim**:
an explicit value REPLACES the default rather than adding to it, so what the
table says is exactly what Lucidos sends.

- **Dropbox** needs `token_access_type=offline`. Google's two parameters do
  nothing for it, so a Dropbox connection made with the default gets a
  four-hour access token and no refresh token at all.
- The value is `key=value&key=value`. Percent-encode a value that itself
  contains `&` or `=`.
- The flow owns `client_id`, `redirect_uri`, `response_type`, `scope`, `state`,
  `code_challenge` and `code_challenge_method`. Setting one of those here is
  refused outright, so use the `redirect_uri` and `scopes` arguments for those.
  `state` is generated per authorization and required back on the callback (see
  "One authorization at a time" below), so a pinned value would break the
  callback rather than configure anything.
- Write `none` for a provider strict enough to reject a parameter it does not
  recognize. That sends neither of the defaults.
- A **Dropbox client connected before this column existed** was backfilled with
  `token_access_type=offline` by a migration, so a reconnect from
  Settings → Accounts renews correctly without the user editing anything. Any
  other provider that turns out to need a value has to be set by hand (or by
  you, on the credential) before reconnecting.

### One authorization at a time

The callback listener binds a **fixed** loopback port (14981), because the
redirect URI has to be registered with the provider ahead of time. So a
workspace engine can have **at most one authorization in flight**, whatever the
provider, and whether it was started by `connect_oauth_account` or by a Connect
/ Reconnect / *Grant access* button.

Two consequences worth knowing before you diagnose one of them:

- **Starting a new authorization cancels the previous one.** If the user
  abandoned a consent screen (or you started a flow they never finished), the
  next one supersedes it rather than failing. The abandoned flow reports
  *"This authorization was canceled, most likely because a newer one was
  started"*. That is expected, not an error to chase. Before 2026-08-06 the
  abandoned flow instead held the port for its full 120 second timeout and every
  retry inside that window died with *"Address already in use (os error 48)"*.
- **A port clash that survives that is another program**, almost always a second
  Lucidos workspace part-way through connecting an account: workspaces run
  concurrently, each with its own engine, and they share this one machine-wide
  port. The error says so. Finish or abandon the other one; there is nothing to
  fix on this credential.

Each authorization also carries its own random `state`, and the listener ignores
any callback that does not echo it back. So a stale redirect from a superseded
flow, or a request from anything else that can reach the loopback port, cannot be
redeemed. Nothing to configure; it is listed here because `state` is refused in
`authorize_params` for this reason.

### Notes on scopes

- **Google**: scopes are full URLs (`https://www.googleapis.com/auth/<api>`).
  `openid email profile` are also valid. `userinfo_url` may instead be
  `https://openidconnect.googleapis.com/v1/userinfo` if you request `openid`.
- **Microsoft**: scopes look like `https://graph.microsoft.com/Mail.Read` or
  short names like `offline_access User.Read`. Include `offline_access` to get a
  refresh token. See the Microsoft section below — its app registration needs
  more care than the others.
- **GitHub**: scopes are short names (`repo read:user`). GitHub tokens don't
  expire and have no refresh token — that's expected.
- **Spotify / Dropbox**: short scope names per their docs. Dropbox's account
  scope is `account_info.read`, which is what its POST userinfo endpoint needs.
  A connection without it still works, it just reports no email.
- **Dropbox for backups** needs four:
  `files.content.write files.content.read files.metadata.read account_info.read`.
  Write covers the folder create, the upload and the retention delete; read is
  restoring; metadata is the backup listing that drives pruning and the health
  card. Request all four, and see the App Console section below, because
  Dropbox will not grant a scope the app itself has not been permitted.

### Dropbox: the App Console decides what may be asked for

Dropbox is the one provider in this table where enabling the app is a separate
step from requesting the scope, and every way of getting it wrong looks
identical to the user, so walk them through all three:

1. **The Permissions tab of their app in the Dropbox App Console is the maximum
   AND the default set.** An authorization request can narrow that set, never
   widen it. Ask for a scope the app has not been permitted and the call that
   needs it fails with *"Your app … does not have the required scope"*.
2. **A ticked box is not a saved box until they press Submit.** The Permissions
   tab keeps the ticks as unsaved page state and its **Submit** button sits at
   the bottom, below the fold, so it is easy to tick four boxes, navigate away,
   and change nothing at all. Have them scroll down and press Submit, then
   reload the tab and confirm the ticks survived. This one step cost a user an
   hour on 2026-08-07: everything else had been done correctly.
3. **Ticking a box there changes nothing that already exists.** Neither an
   issued access token nor the user's existing grant picks up a newly enabled
   scope. After changing the permissions they MUST reconnect the account from
   Settings → Accounts (the Backup page's *Grant access* button does the same
   thing for a backup provider). A refresh does not help: refreshing renews the
   scopes the token already has.

So the order is: enable the permissions in the App Console, press Submit, then
connect. If they connected first, the fix is to enable, Submit, and then
reconnect, and saying "tick the box" alone leaves them looking at the same
error.

## Microsoft (Entra) — redirect URI platform buckets

Entra does not store one flat list of redirect URIs. Each one lives in a
**platform bucket**, and the bucket decides which client type may redeem a code
with it:

| Platform in the portal | Client type | Redemption |
|---|---|---|
| **Web** | confidential | must send `client_secret` |
| **Mobile and desktop applications** | public | must send **no** secret; PKCE instead |
| Single-page application | public + CORS | not what Lucidos is |

`/authorize` accepts a URI from **any** bucket, so a wrong bucket authorizes
fine and only fails at the token exchange. Two symptoms, one cause:

- **`AADSTS90023` — `invalid_request`, *"The provided value for the input
  parameter 'redirect_uri' is not valid"*, after the browser already said
  "Authorization successful!"** — the URI is registered in a bucket that doesn't
  match the client type being used. Classically: the URI sits under *Mobile and
  desktop applications* while Lucidos is sending a `client_secret`.
- **`AADSTS50011` / redirect-URI mismatch at the consent screen** — the string
  doesn't match anything registered. Usually the `127.0.0.1` ↔ `localhost`
  difference, because the portal won't let the IP form into the Web bucket.

Pick one of the two coherent setups and make both halves agree:

**Desktop / public (recommended — Lucidos runs on the user's machine):**
1. Portal → *Authentication* → add a **Mobile and desktop applications**
   platform with `http://127.0.0.1:14981/oauth/callback`.
2. Set *Allow public client flows* to **Yes**.
3. In Lucidos, leave **Client Secret blank**, and omit `redirect_uri` (the
   default IP form is what's registered).

**Web / confidential:**
1. Portal → *Authentication* → add a **Web** platform with
   `http://localhost:14981/oauth/callback` (the box rejects the `127.0.0.1`
   form — that's a portal limitation, not a protocol one).
2. Create a client secret under *Certificates & secrets*.
3. In Lucidos, enter the **Client Secret** and pass
   `redirect_uri="http://localhost:14981/oauth/callback"`.

Do **not** register the same callback in both buckets — Entra picks one
arbitrarily when URIs differ only by bucket, which makes the failure
intermittent.

## Adding a new provider

Adding a known provider is a **knowhow edit, not an engine change**:

1. `web_search` for the provider's OAuth 2.0 authorization + token endpoints
   (and userinfo endpoint if it has one).
2. Add a row to `oauth-providers.json` with its `base_url` and, so the Connect
   button can help too, its `console_url`, `client_type` and `setup_hint`.
3. Note any scope quirks under "Notes on scopes".

That's it — the next `request_credential` / `connect_oauth_account` for that
provider pre-fills from your new row.
