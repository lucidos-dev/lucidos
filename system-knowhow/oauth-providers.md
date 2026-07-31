---
name: OAuth Providers Registry
description: The registry of OAuth 2.0 provider endpoints (auth_url / token_url / userinfo_url, default base URL, typical scopes) the agent consults when collecting OAuth client credentials. Load this BEFORE calling request_credential (auth_type oauth_client) or connect_oauth_account so you can pass the endpoints and the credential modal pre-fills them instead of forcing the user to type Google's / Microsoft's own URLs by hand. Covers the alias rule for dedicated connections (e.g. a health-only Google connection named "ghealth" reuses Google's endpoints), the loopback redirect URI and when to override its host form, the confidential-vs-public client rule (a blank client secret means PKCE), Microsoft Entra's redirect-URI platform buckets and the AADSTS90023 / AADSTS50011 symptoms they cause, and how to add a new provider (edit this file — no engine change). Maintain this file: when you discover a provider's endpoints via web_search, add a row here.
---

# OAuth providers registry

This file is the **single registry** of known OAuth 2.0 provider endpoints. The
engine no longer hardcodes any provider — it reads nothing from this file
directly. **You** (the agent) read it, and you keep it up to date.

## How it's used

When a service needs OAuth client credentials:

1. **Load this file** and find the provider's endpoints.
2. Call `request_credential` with `auth_type: "oauth_client"` (or
   `connect_oauth_account`) and **pass `auth_url`, `token_url`, `userinfo_url`
   and optional `scopes` / `redirect_uri`** from the row below. The credential
   modal then pre-fills those fields and the user only enters `client_id` (plus
   `client_secret`, if the app is registered as a confidential/web client).
3. The values are stored in the credential's JSON
   (`{client_id, client_secret?, auth_url, token_url, userinfo_url, scopes,
   redirect_uri?}`), which is the **per-credential source of truth** for
   endpoints. Token refresh and re-authorization read them back from there.
   `client_secret` and `redirect_uri` are optional — see the two sections below
   for what their absence means.

If you omit the endpoint args, the modal treats it as a custom provider and
makes the user type the URLs in by hand — only do that for a provider you
genuinely can't find. **Better: find it via `web_search` and add a row here** so
the next connection is one step.

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

Resolve a derived name to its base provider's endpoints:

- `ghealth`, `google-health`, `google-*` → use the **google** row.
- `ms-*`, `microsoft-*`, `outlook`, `azure` → use the **microsoft** row.
- Otherwise match the longest provider key that the name starts with / contains;
  fall back to asking the user which base provider it is.

Connect it under the distinct name so its token is stored separately:

```
connect_oauth_account(
  provider="ghealth",
  scopes="https://www.googleapis.com/auth/cloud-healthcare",
  auth_url="https://accounts.google.com/o/oauth2/v2/auth",
  token_url="https://oauth2.googleapis.com/token",
  userinfo_url="https://www.googleapis.com/oauth2/v2/userinfo",
  base_url="https://healthcare.googleapis.com")
```

## Known providers

| Provider | auth_url | token_url | userinfo_url | base_url |
|---|---|---|---|---|
| `google` | `https://accounts.google.com/o/oauth2/v2/auth` | `https://oauth2.googleapis.com/token` | `https://www.googleapis.com/oauth2/v2/userinfo` | `https://www.googleapis.com` |
| `microsoft` | `https://login.microsoftonline.com/common/oauth2/v2.0/authorize` | `https://login.microsoftonline.com/common/oauth2/v2.0/token` | `https://graph.microsoft.com/v1.0/me` | `https://graph.microsoft.com` |
| `github` | `https://github.com/login/oauth/authorize` | `https://github.com/login/oauth/access_token` | `https://api.github.com/user` | `https://api.github.com` |
| `dropbox` | `https://www.dropbox.com/oauth2/authorize` | `https://api.dropboxapi.com/oauth2/token` | _(none)_ | `https://api.dropboxapi.com` |
| `spotify` | `https://accounts.spotify.com/authorize` | `https://accounts.spotify.com/api/token` | `https://api.spotify.com/v1/me` | `https://api.spotify.com` |

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
- **Spotify / Dropbox**: short scope names per their docs.

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
2. Add a row to the table above with its `base_url`.
3. Note any scope quirks under "Notes on scopes".

That's it — the next `request_credential` / `connect_oauth_account` for that
provider pre-fills from your new row.
