---
name: OAuth Providers Registry
description: The registry of OAuth 2.0 provider endpoints (auth_url / token_url / userinfo_url, default base URL, typical scopes) the agent consults when collecting OAuth client credentials. Load this BEFORE calling request_credential (auth_type oauth_client) or connect_oauth_account so you can pass the endpoints and the credential modal pre-fills them instead of forcing the user to type Google's / Microsoft's own URLs by hand. Covers the alias rule for dedicated connections (e.g. a health-only Google connection named "ghealth" reuses Google's endpoints), the loopback redirect URI, and how to add a new provider (edit this file — no engine change). Maintain this file: when you discover a provider's endpoints via web_search, add a row here.
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
   and optional `scopes`** from the row below. The credential modal then
   pre-fills those fields and the user only enters `client_id` + `client_secret`.
3. The values are stored in the credential's JSON
   (`{client_id, client_secret, auth_url, token_url, userinfo_url, scopes}`),
   which is the **per-credential source of truth** for endpoints. Token refresh
   and re-authorization read the URLs back from there.

If you omit the endpoint args, the modal treats it as a custom provider and
makes the user type the URLs in by hand — only do that for a provider you
genuinely can't find. **Better: find it via `web_search` and add a row here** so
the next connection is one step.

### Redirect URI

Every provider's OAuth app must whitelist this exact loopback redirect URI:

```
http://127.0.0.1:14981/oauth/callback
```

Some providers (e.g. Spotify) reject `localhost` but accept the `127.0.0.1`
loopback IP. Tell the user to register exactly this URI.

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
  refresh token.
- **GitHub**: scopes are short names (`repo read:user`). GitHub tokens don't
  expire and have no refresh token — that's expected.
- **Spotify / Dropbox**: short scope names per their docs.

## Adding a new provider

Adding a known provider is a **knowhow edit, not an engine change**:

1. `web_search` for the provider's OAuth 2.0 authorization + token endpoints
   (and userinfo endpoint if it has one).
2. Add a row to the table above with its `base_url`.
3. Note any scope quirks under "Notes on scopes".

That's it — the next `request_credential` / `connect_oauth_account` for that
provider pre-fills from your new row.
