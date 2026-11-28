---
name: Building an Auth Handshake, Builtin provider sign-in
description: Part of Building an Auth Handshake. Its index lists the sections it covers.
---

One part of `system-knowhow/building-an-auth-handshake`, which lists every part.

## Builtin provider sign-in

Before authoring a handshake, check the builtin catalog in
`system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy`.
The `claude-gateway` builtin uses the engine's issuer-bound sign-in and managed model policy.
It accepts only `POST /messages`; apps never export or supply its bearer.
An explicit `apis.json` entry still overrides a builtin.

## Decision tree

1. **Single header or query param that doesn't rotate** (Bearer token, API key, basic auth) → `static_credential`. Done.
2. **Login script returns a session token** (Comfort Cloud, OAuth password grant, anything POSTing username+password to `/login`) → `script_handshake`.
3. **HMAC signature appended to the query string** in the simple Binance shape (sha256 over `<existing>&timestamp=<ms>`, append `signature=<hex>`) → built-in `hmac_signed`. No code to write.
4. **Anything else per-request** → `wasm_signer`: another signed payload, HMAC over headers + body, ECDSA / EdDSA, AWS SigV4, a per-call JWT, a binary auth blob. You write a `no_std` Rust module under `signers/<name>/`.
5. **Multi-step auth** (e.g. session token from a login script AND a per-request signature) → compose: `[script_handshake, wasm_signer]`. Earlier layers' outputs flow into the signer via `prior_layer_outputs`.

## On-disk schema

`data/config/apis.json`:

```json
{
  "<provider-name>": {
    "base_url": "https://api.example.com",
    "insecure_transport": false,   // optional, default false (see below)
    "timeout_secs": 300,           // optional, 1-600 (see below)
    "auth": {
      "pipeline": [
        // one or more layer entries, in execution order
      ],
      "granted_capabilities": []   // e.g. ["replace_body"], see WASM section
    }
  }
}
```

### `insecure_transport`: off by default, and say why you turned it on

The engine validates every upstream's certificate. It also refuses to put a
credential on plain `http://` to a non-loopback host, because anyone on the
path reads it.

`"insecure_transport": true` turns both off for that one provider: you accept
that this upstream never proves who it is. Use it in exactly two cases.

- **A self-signed dev backend.** An `https://` URL whose certificate nothing
  signed.
- **A device on your LAN or tailnet with a key**, reachable only over plain
  `http://`.

Two things do NOT need it. A **loopback** address is exempt, so
`{"sonos": {"base_url": "http://localhost:5005"}}` and a keyed local model
server keep working. An **uncredentialed** plain-`http://` entry has no secret
to leak.

Without the flag, an affected call answers 502 naming what to change. With it,
the engine logs the provider at startup and posts one notification listing every
entry it will not vouch for.

### `timeout_secs`: how long this entry waits

Seconds the engine waits on one upstream request for this entry, from 1 to 600.
It wins over the workspace's `proxy_timeout_secs` preference (default 30). Set
it on a slow backend, so one long call can finish without raising the limit for
every entry. An out-of-range value rejects the entry by name, and its calls
answer 502 with the reason. See `system-knowhow/lucidos-cli/subcommands-13.md` § Timeouts.

Layer shapes (all live in `proxy_pipeline_config::LayerConfig`):

```jsonc
// Static credential: kind picks the variant. `header` is optional for
// api_key (default "Authorization"); `param_name` is required for query_param.
{"type": "static_credential", "kind": "bearer",      "credential": "openai-key"}
{"type": "static_credential", "kind": "api_key",     "credential": "k", "header": "X-API-Key"}
{"type": "static_credential", "kind": "basic",       "credential": "u-and-p"}     // value must be "user:password"
{"type": "static_credential", "kind": "query_param", "credential": "k", "param_name": "api-key"}

// Script handshake: `script` resolves relative to `data/` (no `..`, no leading
// `/`): `"scripts/auth/comfort-cloud.py"` is the file at
// `data/scripts/auth/comfort-cloud.py` (git-tracked). `credential` is OPTIONAL:
// omit it when the script sources its secret elsewhere (OS keychain, OAuth-only
// exchange). `oauth_providers` is optional too (see Script contract).
{"type": "script_handshake", "credential": "comfort-cloud", "script": "scripts/auth/comfort-cloud.py"}
{"type": "script_handshake", "script": "scripts/auth/keychain-login.py"}   // no credential: the script sources its secret
{"type": "script_handshake", "credential": "firebase-web-api-key", "script": "scripts/auth/firebase-google-exchange.py", "oauth_providers": ["google"]}

// Built-in Binance-shape HMAC signs the query string and appends a signature.
// Setting `timestamp_param` prepends that timestamp to the signed query.
// `key_header` defaults to `X-API-KEY`; Binance uses `X-MBX-APIKEY` below.
{
  "type": "hmac_signed",
  "key_credential": "binance-key",
  "secret_credential": "binance-secret",
  "key_header": "X-MBX-APIKEY",
  "algorithm": "sha256",
  "signed_payload": "query_string",
  "signature_param": "signature",
  "timestamp_param": "timestamp"
}

// WASM signer: `module` is the basename under data/auth-modules/.
// `credential_handles` map logical names (declared in the module's manifest)
// to credential-store entries. The module never sees the raw bytes: it gets
// an opaque integer handle and asks the host to HMAC with it.
{
  "type": "wasm_signer",
  "module": "binance-hmac",
  "credential_handles": [
    {"name": "api_secret", "credential": "binance-secret"}
  ]
}
```

Old single-variant configs (`{"auth": {"type": "bearer", ...}}`) auto-upgrade to the pipeline shape on engine startup, with an `apis.json.bak.<unix>` backup beside the live file (`proxy_migration.rs`). The upgrade runs **per entry**, so a legacy entry beside migrated ones still upgrades. `credential_bundle` is permanently removed: an entry using it is rejected, not upgraded.

### A bad entry is rejected, never fatal (ADR 0135)

Nothing in `apis.json` can stop the workspace starting. Each entry parses on its own:

- **Good entries load and work**, including the ones beside a bad entry.
- **A rejected entry is named** in the startup log, and announced as a notification plus a `ProxyConfigRejected` event. The reason is the upgrade error if any (`credential_bundle`, an unknown `auth.type`, a missing required field), else the parse error.
- **A request to a rejected name answers 502** with that reason, never 404. Only a 404 falls through to the builtin provider of the same name, which would silently change the backend.
- **An unreadable or unparseable file** rejects every name, builtins included. Nothing can tell which entry it overrode, so the engine serves none until it parses again.

Fix a rejected entry by editing `data/config/apis.json` and restarting the workspace.

## Layer 1: `static_credential`

Use the LLM `request_credential` tool (or the credentials UI) to register the credential, then add a one-line entry to `apis.json`. No code. See `system-knowhow/lucidos-cli.md` for the call syntax.

## Layer 2: `script_handshake`

For login dances. The engine shares one in-memory cache across the workspace's proxy pipelines. Each handshake entry has its own expiry, capped at 365 days. Concurrent requests for the same uncached entry share one `python3` run. An upstream 401 on a cache hit invalidates that entry and retries once. Restarting the workspace clears the cache.

### Script contract

- **Where the file lives.** `script` resolves under `data/scripts/` and nowhere else: `data/scripts/auth/foo.py` is `"script": "scripts/auth/foo.py"`. A path outside `scripts/` is refused when `apis.json` loads, naming the provider and the value. This matches the approve route, and stops `"script": ".env"` naming the gitignored config.
- **Two spellings, one file.** Write `"scripts/auth/foo.py"`. An older config's `"data/scripts/auth/foo.py"` still resolves, because the redundant `data/` comes off once. Only once: `"data/data/scripts/x.py"` is refused, and `"data/.env"` reduces to `".env"` and is refused too.
- **A script runs only if Lucidos recorded who wrote it** (ADR 0144).
  `data/scripts/` is writable over the API, and an app UI reaches that API with
  your authority. So the engine records a script only when its OWN file tools
  write it. That happens when you ask the Lucidos Agent to write or edit it. A
  save from the Files panel, an editor, or a plugin install leaves the script
  unapproved.

  An unapproved script answers 502 with
  `auth handshake script '<path>' is not approved, so it will not run`. Two ways
  back: ask the agent to make the change, or run
  `lucidos handshake approve scripts/auth/<name>.py`. `lucidos handshake list`
  shows the state of every script `apis.json` names. The Files panel shows the
  same warning when you open one.

  Approval is per content hash, not per path. Overwriting an approved script
  does not inherit its standing.
- **A script's token goes to one host, and `apis.json` cannot move it.** No
  stored credential's scope covers a minted token. So the record carries a
  `base_url` column beside the hash (ADR 0144 decision 4). It fills the first
  time a proxy call would use the script, from the entry's `base_url` then, and
  is enforced from then on.

  Point the entry elsewhere afterwards and the call answers 502 naming both
  hosts. This stops a rewritten `base_url` in `data/config/apis.json` (writable
  over the API) delivering the token to whoever wrote it. To move a script on
  purpose, or to let a second provider share one, edit the column in
  `<workspace>/.lucidos/approved-handshake-scripts`.
- **A script is handed one set of secrets, and `apis.json` cannot swap it.**
  `credential` and `oauth_providers` go into the script's environment, never to
  the entry's `base_url`. So the credential's own scope says nothing about them:
  an OAuth client is scoped to its token endpoint, where the SCRIPT presents it.

  The record carries an `injects` column instead, beside `base_url` and filled
  the same way. It lists `c:<credential>` and `o:<provider>` members. Change
  either field afterwards and the call answers 502, naming what the script is
  approved to receive. To change it on purpose, edit that column.
- **The path must be relative, with no `..` anywhere in it.** Any `..` substring is refused, not just a `..` segment. A rejected value takes out that one entry, naming the provider and the value, and a call to it answers 502. See "A bad entry is rejected, never fatal" above.
- **`credential` is optional.** When set, the engine injects it as env vars (table below) before the script runs. When omitted, this layer injects no `CRED_*` var, and the script sources its secret itself (OS keychain, OAuth-only exchange, etc.).
- Env var shape depends on the credential's type. It is the same convention `run_python` / `run_bash` use, so a script works under either. A handshake gets its configured credential without naming it. `run_python` / `run_bash` set one only when the text names it (`system-knowhow/best-practices/workspace-layout.md` § Credentials in subprocesses):

  | Credential type | Env vars injected |
  |---|---|
  | `password` | `CRED_<NAME>_USERNAME` + `CRED_<NAME>_PASSWORD` (split out of the stored JSON) |
  | `api_key`  | `CRED_<NAME>` (the raw key) |
  | `bearer`   | `CRED_<NAME>` (the raw token) |
  | `basic`    | `CRED_<NAME>` (the raw `user:password` string; split it yourself if you need the parts) |
  | `secret`   | `CRED_<NAME>` (the raw shared secret, the type for a value signed with rather than sent) |

  Transform for `<NAME>`: uppercase the credential's `service_name`, then replace every character outside `A-Z 0-9 _` with `_`. So `comfort-cloud` (password) → `CRED_COMFORT_CLOUD_USERNAME` + `CRED_COMFORT_CLOUD_PASSWORD`; `firebase-web-api-key` (api_key) → `CRED_FIREBASE_WEB_API_KEY`; `email:work` → `CRED_EMAIL_WORK`. The result is always a legal shell identifier. Any credential type works: pick the one that honestly describes the secret.
- **Optional OAuth env vars.** For each provider in `oauth_providers: ["<name>", ...]`, the engine looks up the connected OAuth account and refreshes an expired access token. It injects `OAUTH_<UPPER>_ACCESS_TOKEN` (always) and `OAUTH_<UPPER>_EMAIL` (when known), with the same name transform. So `["google"]` exposes `OAUTH_GOOGLE_ACCESS_TOKEN`. If the provider is not connected, the layer fails with a 502 naming it, the script never runs, and the message points at `connect_oauth_account`.
- **The script's whole environment is those credentials plus a fixed runtime allowlist.** It inherits nothing else, so a handshake cannot read the database password or another provider's key. The allowlist is `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `LC_CTYPE`, `SSL_CERT_FILE`, `SSL_CERT_DIR` and `LUCIDOS_WORKSPACE` (`RUNTIME_ENV_ALLOWLIST` in `api/proxy_script_runner.rs`). So there is no other `LUCIDOS_*` setting, no `DATABASE_URL`, and no provider key. A script needing another value takes it as a credential.
- **A handshake script gets only `CRED_*` and `OAUTH_*` names, never a credential's custom env var name.** A custom name such as `GITHUB_TOKEN` reaches a `run_bash` or `run_python` call that names it, but the engine drops it here and logs the drop. It passes only `CRED_` or `OAUTH_` followed by `A-Z`, `0-9` and `_`. So no name can set a loader hook (`PYTHONPATH`), a proxy (`HTTPS_PROXY`) or a runtime name (`PATH`). Read the canonical `CRED_<NAME>`, which always arrives.
- **The script is self-contained.** It may import the standard library and
  anything installed for `python3`, but not a module beside it. Its directory is
  writable over the API, so a module there is unapproved code. A file named
  after a stdlib module would shadow the real one. Put a helper inline, or
  install it as a package.
- Prints exactly **one** JSON object on stdout:
  ```json
  {
    "headers": {
      "Authorization": "Bearer <token>",
      "X-Client-Id": "<client>"
    },
    "expires_in": 3600
  }
  ```
- `expires_in` is mandatory, with a 60-second floor so a buggy `expires_in: 1` cannot loop the script.
- On error: exit non-zero. The engine puts stderr in its 502 response and logs.
- Be **idempotent**: the engine re-runs on every cache miss and on 401-retry. No file writes, no event emits, no side effects.
- Use the Python stdlib (`hashlib`, `hmac`, `base64`, `secrets`) for crypto. **Don't** `subprocess.run(["sha256sum"/"shasum"/"openssl", ...])`: `sha256sum` isn't on macOS by default, `shasum` isn't on Linux, and Lucidos installs guarantee neither.
- 30-second timeout.

### Worked example (Comfort Cloud, `password` credential)

The canonical login dance: the script POSTs a username and password to `/login` and gets back a session token. So `credential` is a `password`-typed entry, and the script reads `CRED_<NAME>_USERNAME` + `CRED_<NAME>_PASSWORD`.

```jsonc
// data/config/apis.json
{
  "comfort-cloud": {
    "base_url": "https://accsmart.panasonic.com",
    "auth": {
      "pipeline": [
        {"type": "script_handshake",
         "credential": "comfort-cloud",
         "script": "scripts/auth/comfort-cloud.py"}
      ]
    }
  }
}
```

```python
# data/scripts/auth/comfort-cloud.py
import os, json, sys
import pcomfortcloud   # pip install pcomfortcloud

try:
    s = pcomfortcloud.Session(
        os.environ["CRED_COMFORT_CLOUD_USERNAME"],
        os.environ["CRED_COMFORT_CLOUD_PASSWORD"],
    )
    s.login()
    print(json.dumps({
        "headers": {"X-Access-Token": s.access_token, "X-Client-Id": s.client_id},
        "expires_in": 1800,   # half of Panasonic's hour, safe under clock skew
    }))
except Exception as e:
    print(f"Comfort Cloud login failed: {e}", file=sys.stderr)
    sys.exit(1)
```

### Worked example (Firebase via Google OAuth)

A Firebase-backed app needs Firebase ID tokens for Firestore / Storage, but the user identity is the Google account already connected via `connect_oauth_account`. Use `oauth_providers: ["google"]` to forward the Google access token. The script exchanges it for a Firebase ID token at `identitytoolkit.googleapis.com:signInWithIdp`.

```jsonc
// data/config/apis.json
{
  "firestore-snake-work": {
    "base_url": "https://firestore.googleapis.com",
    "auth": {
      "pipeline": [
        {"type": "script_handshake",
         "credential": "firebase-snake-work-web-api-key",
         "script": "scripts/auth/firebase-google-exchange.py",
         "oauth_providers": ["google"]}
      ]
    }
  }
}
```

```python
# data/scripts/auth/firebase-google-exchange.py
import os, json, sys, urllib.request

token = os.environ["OAUTH_GOOGLE_ACCESS_TOKEN"]
api_key = os.environ["CRED_FIREBASE_SNAKE_WORK_WEB_API_KEY"]
req = urllib.request.Request(
    f"https://identitytoolkit.googleapis.com/v1/accounts:signInWithIdp?key={api_key}",
    data=json.dumps({
        "postBody": f"access_token={token}&providerId=google.com",
        "requestUri": "http://localhost",
        "returnIdpCredential": True,
        "returnSecureToken": True,
    }).encode(),
    headers={"Content-Type": "application/json"},
)
try:
    resp = json.load(urllib.request.urlopen(req, timeout=15))
except Exception as e:
    print(f"Firebase exchange failed: {e}", file=sys.stderr)
    sys.exit(1)
print(json.dumps({
    "headers": {"Authorization": f"Bearer {resp['idToken']}"},
    "expires_in": int(resp.get("expiresIn", 3600)) - 60,   # one-minute safety margin
}))
```

The web API key is a non-secret Firebase project identifier, registered as an `api_key` credential (`request_credential` with `service_name = firebase-snake-work-web-api-key`, `auth_type = "api_key"`). The script reads it from `CRED_FIREBASE_SNAKE_WORK_WEB_API_KEY`. A `password`-typed entry with the key in `password` also works (as `CRED_<NAME>_PASSWORD`), but `api_key` is the honest type and needs no dummy `username`. The engine refreshes the Google access token before invoking the script, so the script ignores expiry.

If the user hasn't connected Google, the proxy request fails fast with `502 ... script_handshake requires OAuth provider 'google' but no account is connected; user must connect it first via connect_oauth_account`.

## Layer 3: `hmac_signed` (built-in)

For services that sign the query string, `hmac_signed` appends
`<signature_param>=<hex(hmac_sha256_or_512(secret, canonical_query))>`.
Setting `timestamp_param` also appends `<timestamp_param>=<unix-ms>` before signing.
The default adds no timestamp. The API key goes in `key_header`, which defaults
to `X-API-KEY`. Binance requires the explicit `X-MBX-APIKEY` override shown above.

If the service signs anything other than the query string (body, full path, headers, custom canonical form), go to Layer 4.

