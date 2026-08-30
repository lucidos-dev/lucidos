---
name: Building an Auth Handshake
description: Authenticating an external API when a static header is not enough: the apis.json proxy auth pipeline and authoring a no_std WASM signer. Load for "can't log into X", "needs a session token", "OAuth password grant", "HMAC signing", "sign every request", "wasm signer", or "wasmtime incompatible import type".
---

# Building an auth handshake

The proxy auth pipeline runs on every outbound `/api/v1/proxy/<api>/...` call. A provider's `auth` block in `data/config/apis.json` is a list of **layers** that execute in declared order — each layer can attach headers, append query params, or (with explicit consent) replace the body. The engine forwards the request once layers have finished. On a 401 from upstream, layers that opted into `InvalidateAndRetry` get their caches blown and the pipeline runs once more.

Four layer types are supported:

| Layer | Use for | Per-request work |
|---|---|---|
| `static_credential` (kind: `bearer` / `api_key` / `basic` / `query_param`) | A header or query param that doesn't rotate. | None — value baked in. |
| `script_handshake` | Login dance that mints a short-lived session token (POST username+password to `/login`, get back headers, cache until expiry). Python script you write. | Cache hit: free. Miss: spawn `python3`, parse JSON. |
| `hmac_signed` | Built-in Binance-shape HMAC signing over the canonical query string (`<existing>&timestamp=<ms>` → SHA-256, append `signature=<hex>`). | HMAC compute, no cache. |
| `wasm_signer` | Anything more elaborate: bespoke signature schemes, ECDSA / EdDSA, AWS SigV4, header signing, request body signing, anything HMAC-but-different. Sandboxed `no_std` Rust → wasm32. | Compile-once, instantiate-per-call. |

For a single static header (Bearer token, API key, basic auth) reach for `static_credential` — see `system-knowhow/lucidos-cli.md` § `lucidos proxy`. The other three layers cost more (a Python interpreter, a per-request crypto compute, or a WASM instance), so don't pick them when a static header would do.

**Don't know the API's shape yet?** If it is a site the user is logged into and
there is no public API to read, derive one: drive it once in the browser, watch
what the page calls, and turn that into an entry plus an endpoint catalog. See
`system-knowhow/deriving-an-api-from-a-site.md`, then come back here to pick the
layer.

## Decision tree

1. **Single header that doesn't rotate** → `static_credential` (bearer / api_key / basic / query_param). Done.
2. **Login script returns a session token** (Comfort Cloud, OAuth password grant, anything POSTing username+password to `/login`) → `script_handshake`. The engine caches the headers until your script's `expires_in` elapses, refreshes on cache miss, retries once on 401.
3. **HMAC signature appended to the query string** in the simple Binance shape (timestamp + sha256 over `<existing>&timestamp=<ms>`) → built-in `hmac_signed`. No code to write.
4. **Anything else per-request** (different signed payload, HMAC-but-different, HMAC over headers + body, ECDSA, SigV4, JWT minted per call, a binary-protocol auth blob) → `wasm_signer`. You write a `no_std` Rust module under `signers/<name>/`.
5. **Multi-step auth** (e.g. session token from a login script AND a per-request signature) → compose: `[script_handshake, wasm_signer]`. Earlier layers' outputs flow into the signer via `prior_layer_outputs`.

## On-disk schema

`data/config/apis.json`:

```json
{
  "<provider-name>": {
    "base_url": "https://api.example.com",
    "insecure_transport": false,   // optional, default false (see below)
    "auth": {
      "pipeline": [
        // one or more layer entries, in execution order
      ],
      "granted_capabilities": []   // e.g. ["replace_body"] — see WASM section
    }
  }
}
```

### `insecure_transport`: off by default, and say why you turned it on

The engine validates the upstream's certificate, for every provider. It also
refuses to put a credential on plain `http://` when the host is not loopback,
because anyone on the path reads it.

`"insecure_transport": true` on an entry turns both off, for that one provider.
One flag, one meaning: you accept that this upstream never proves who it is.
Reach for it in exactly two cases.

- **A self-signed dev backend.** An `https://` URL whose certificate nothing
  signed.
- **A device on your LAN or tailnet with a key**, reachable only over plain
  `http://`.

Two things do NOT need it. A **loopback** address is exempt, so
`{"sonos": {"base_url": "http://localhost:5005"}}` and a keyed local model
server both keep working untouched. An **uncredentialed** plain-`http://` entry
has no secret to leak, so it is fine as it stands.

Without the flag, an affected call answers 502 naming what to change. With it,
the engine logs the provider at startup and posts one notification listing every
entry it will not vouch for.

Layer shapes (all live in `proxy_pipeline_config::LayerConfig`):

```jsonc
// Static credential — kind picks the variant. `header` is optional for
// api_key (default "Authorization"); `param_name` is required for query_param.
{"type": "static_credential", "kind": "bearer",      "credential": "openai-key"}
{"type": "static_credential", "kind": "api_key",     "credential": "k", "header": "X-API-Key"}
{"type": "static_credential", "kind": "basic",       "credential": "u-and-p"}     // value must be "user:password"
{"type": "static_credential", "kind": "query_param", "credential": "k", "param_name": "api-key"}

// Script handshake — `script` resolves relative to `data/` (no `..`, no leading
// `/`): `"scripts/auth/comfort-cloud.py"` is the file at
// `data/scripts/auth/comfort-cloud.py` (git-tracked). `credential` is OPTIONAL —
// omit it when the script sources its secret elsewhere (OS keychain, OAuth-only
// exchange); then no `CRED_*` env var is injected from this layer. `oauth_providers`
// is optional; when present, each listed provider's connected OAuth access token is
// injected as `OAUTH_<UPPER>_ACCESS_TOKEN` in the script's env.
{"type": "script_handshake", "credential": "comfort-cloud", "script": "scripts/auth/comfort-cloud.py"}
{"type": "script_handshake", "script": "scripts/auth/keychain-login.py"}   // no credential — secret sourced by the script
{"type": "script_handshake", "credential": "firebase-web-api-key", "script": "scripts/auth/firebase-google-exchange.py", "oauth_providers": ["google"]}

// Built-in Binance-shape HMAC. Sign-only the query string; appends
// `timestamp=<ms>` first, then `signature=<hex>`. Key is sent in `X-MBX-APIKEY`
// (or whatever `key_header` overrides to).
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

// WASM signer — `module` is the basename under data/auth-modules/.
// `credential_handles` map logical names (declared in the module's manifest)
// to credential-store entries. The module never sees the raw bytes — it gets
// an opaque integer handle and asks the host to HMAC with it.
{
  "type": "wasm_signer",
  "module": "binance-hmac",
  "credential_handles": [
    {"name": "api_secret", "credential": "binance-secret"}
  ]
}
```

Old single-variant configs (`{"auth": {"type": "bearer", ...}}`) are auto-upgraded to the pipeline shape on engine startup, with a `apis.json.bak.<unix>` backup written next to the live file (`proxy_migration.rs`). The upgrade is decided **per entry**, so a legacy entry appended beside already-migrated ones is still upgraded. `credential_bundle` is permanently removed, and an entry using it is rejected rather than upgraded.

### A bad entry is rejected, never fatal (ADR 0135)

Nothing in `apis.json` can stop the workspace starting. Each entry is parsed on its own:

- **Good entries load and work**, including the ones beside a bad entry.
- **A rejected entry is named** in the startup log, and announced to the workspace as a notification plus a `ProxyConfigRejected` event. The reason is the upgrade error where there is one (`credential_bundle`, an unknown `auth.type`, a missing required field), otherwise the parse error.
- **A request to a rejected name answers 502**, carrying that reason. It is deliberately not a 404. Only a 404 falls through to the builtin provider of the same name, so answering one here would silently change which backend the request reaches.
- **An unreadable or unparseable file** rejects every name, builtins included. Nothing can tell which entry the file was overriding, so the safe answer is to serve none of them until it parses again.

Fix a rejected entry by editing `data/config/apis.json` and restarting the workspace.

## Layer 1: `static_credential`

Use the LLM `request_credential` tool (or the credentials UI) to register the credential, then add a one-line entry to `apis.json`. No code. See `system-knowhow/lucidos-cli.md` for the call syntax.

## Layer 2: `script_handshake`

For login dances. The engine caches the script's output until `expires_in` elapses, singleflights concurrent first-time requests so only one `python3` runs, and on a 401 from upstream invalidates the cache and retries once.

### Script contract

- **Where the file lives.** `script` is resolved under `data/scripts/`, and nowhere else: the file at `data/scripts/auth/foo.py` is referenced as `"script": "scripts/auth/foo.py"`. A path outside `scripts/` is refused when `apis.json` loads, naming the provider and the value, so the config never reaches a request. That guard matches the approve route, which has always required `data/scripts/`. Without it, `"script": ".env"` needed no `..` to name the gitignored config beside the typed subdirectories.
- **Two spellings, one file.** `"scripts/auth/foo.py"` is the form to write. A config from before resolution moved under `data/` says `"data/scripts/auth/foo.py"`, and that still resolves: the redundant `data/` comes off once. It comes off once and no more, so `"data/data/scripts/x.py"` is refused rather than laundered, and `"data/.env"` reduces to `".env"` and is refused with it.
- **A script runs only if Lucidos recorded who wrote it.** `data/scripts/` is
  writable over the API, and an app UI reaches that API with your authority. So
  the engine will not run a handshake script just because the file is there
  (ADR 0144). It records the content when its OWN file tools write it. That is
  what happens when you ask the Lucidos Agent to write or edit the script.
  Nothing else records: a save from the Files panel, an editor, or a plugin
  install leaves the script unapproved.

  An unapproved script answers 502 with
  `auth handshake script '<path>' is not approved, so it will not run`. Two ways
  back: ask the agent to make the change, or run
  `lucidos handshake approve scripts/auth/<name>.py`. `lucidos handshake list`
  shows the state of every script `apis.json` names. Opening one in the Files
  panel shows the same warning before you hit the 502.

  Editing the file changes its hash, so approval is per content, not per path.
  Overwriting an approved script does not inherit its standing.
- **A script's token goes to one host, and `apis.json` cannot move it.** The
  script mints a live token, so no stored credential's scope covers it. The
  record therefore carries a `base_url` column beside the hash, and the token
  goes nowhere else (ADR 0144 decision 4). It is filled the first time a proxy
  call would use the script, from whatever `base_url` the entry then names, and
  enforced from then on.

  Point the entry somewhere else afterwards and the call answers 502 naming
  both hosts. That is the whole point: `data/config/apis.json` is writable over
  the API, and without the column a rewritten `base_url` delivered the minted
  token to whoever wrote it. To move a script on purpose, or to let a second
  provider share one, edit the column in
  `<workspace>/.lucidos/approved-handshake-scripts`.
- **A script is handed one set of secrets, and `apis.json` cannot swap it.**
  `credential` and `oauth_providers` both name something the engine puts in the
  script's environment. Neither is sent to the entry's `base_url`, so the
  credential's own scope has nothing true to say about them: an OAuth client is
  scoped to its provider's token endpoint, which is where the SCRIPT presents
  it, not where the proxy goes.

  The record carries an `injects` column instead, beside the `base_url` one and
  filled the same way. It lists `c:<credential>` and `o:<provider>` members.
  Change either field afterwards and the call answers 502, naming what the
  script is approved to receive. To change it on purpose, edit that column.
- **The path must be relative, with no `..` anywhere in it.** Not just no `..` segment: any `..` substring is refused, because this is a filesystem path. A rejected value takes out that one entry, naming the provider and the value, and a call to it answers 502. See "A bad entry is rejected, never fatal" above.
- **`credential` is optional.** When the layer config sets `credential`, that credential is injected as env vars (shape below) before the script runs. When it's omitted, no `CRED_*` env var is injected from this layer — the script must source its secret by other means (read a rotating token from the OS keychain, do an OAuth-only exchange, etc.). The env-var table below applies only when a credential is configured.
- Reads the named credential from env vars. The shape depends on the credential's type — same convention `run_python` / `run_bash` already inject for their subprocesses, so a script you wrote for one works for the other:

  | Credential type | Env vars injected |
  |---|---|
  | `password` | `CRED_<NAME>_USERNAME` + `CRED_<NAME>_PASSWORD` (split out of the stored JSON) |
  | `api_key`  | `CRED_<NAME>` (the raw key) |
  | `bearer`   | `CRED_<NAME>` (the raw token) |
  | `basic`    | `CRED_<NAME>` (the raw `user:password` string — split it yourself if you need the parts) |
  | `secret`   | `CRED_<NAME>` (the raw shared secret, the type for a value signed with rather than sent) |

  Transform for `<NAME>`: uppercase the credential's `service_name`, then replace every character outside `A-Z 0-9 _` with `_`. So `comfort-cloud` (password) → `CRED_COMFORT_CLOUD_USERNAME` + `CRED_COMFORT_CLOUD_PASSWORD`; `firebase-web-api-key` (api_key) → `CRED_FIREBASE_WEB_API_KEY`; a namespaced name like `email:work` → `CRED_EMAIL_WORK`. The injected variable is therefore always a legal shell identifier. There is no type restriction: pick whichever credential type honestly describes the secret, and the script reads the matching env var.
- **Optional OAuth env vars.** When the layer config lists `oauth_providers: ["<name>", ...]`, the engine looks up each provider's connected OAuth account (auto-refreshing the access token if expired) and injects `OAUTH_<UPPER>_ACCESS_TOKEN` (always) and `OAUTH_<UPPER>_EMAIL` (when known). Same name transform as `CRED_*`. So `oauth_providers: ["google"]` against a connected Google account exposes `OAUTH_GOOGLE_ACCESS_TOKEN` (and `OAUTH_GOOGLE_EMAIL` when the user's email is on the account). If the user hasn't connected the requested provider, the layer fails with a 502 naming the missing provider — the script never runs and the user gets a clear hint to invoke `connect_oauth_account` first.
- **The script's whole environment is those credentials plus a fixed runtime allowlist.** It inherits nothing else from the engine, which is what stops a handshake reading the database password or another provider's key. The allowlist is `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `LC_CTYPE`, `SSL_CERT_FILE`, `SSL_CERT_DIR` and `LUCIDOS_WORKSPACE` (`RUNTIME_ENV_ALLOWLIST` in `api/proxy_script_runner.rs`). So `os.environ` holds no `LUCIDOS_*` setting beyond the workspace path, no `DATABASE_URL`, and no provider key. A script needing another value takes it as a credential.
- **The script is self-contained.** It may import the standard library and
  anything installed for `python3`, but not a module sitting beside it. Its own
  directory is writable over the API. So a module there is unapproved code, and
  a file named after a stdlib module would shadow the real one. Both would run
  with this layer's credentials, so neither is importable. Put a helper inline,
  or install it as a package.
- Performs the login dance — any HTTP, any third-party library importable by system `python3`.
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
- `expires_in` is mandatory and clamped to a 60-second floor (so a buggy `expires_in: 1` doesn't DOS-loop the script).
- On error: exit non-zero. Stderr is captured into the engine's 502 response and logs.
- Be **idempotent** — the engine re-runs on every cache miss and on 401-retry. No file writes, no event emits, no side effects.
- Use the Python stdlib (`hashlib`, `hmac`, `base64`, `secrets`) for crypto. **Don't** `subprocess.run(["sha256sum"/"shasum"/"openssl", ...])` — `sha256sum` isn't on macOS by default, `shasum` isn't on Linux, and the bundled installer paths Lucidos ships to don't guarantee either. Pure Python runs in the same interpreter the engine already spawned, with no PATH surprises.
- 30-second timeout.

### Worked example (Comfort Cloud — `password` credential)

This is the canonical login dance: the user genuinely has a username and a password, the script POSTs both to `/login`, and gets back a session token. So `credential` here is a `password`-typed entry holding both fields, and the script reads `CRED_<NAME>_USERNAME` + `CRED_<NAME>_PASSWORD`.

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

A Firebase-backed app stored in a Lucidos workspace needs Firebase ID tokens for Firestore / Storage requests, but the *user identity* is the same Google account they already connected via `connect_oauth_account`. Use `oauth_providers: ["google"]` to forward the Google access token into the script, and have the script exchange it for a Firebase ID token at `identitytoolkit.googleapis.com:signInWithIdp`.

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

The web API key is a non-secret Firebase project identifier registered as an `api_key` credential (`request_credential` with `service_name = firebase-snake-work-web-api-key`, `auth_type = "api_key"`); the script reads it from `CRED_FIREBASE_SNAKE_WORK_WEB_API_KEY`. (A `password`-typed entry with the key in the `password` field still works — the env var changes to `CRED_<NAME>_PASSWORD` — but `api_key` is the honest type and avoids inventing a dummy `username`.) The Google OAuth account is whatever the user already connected — the engine refreshes its access token automatically before invoking the script, so the script doesn't have to think about expiry.

If the user hasn't connected Google, the proxy request fails fast with `502 ... script_handshake requires OAuth provider 'google' but no account is connected; user must connect it first via connect_oauth_account` — the operator sees exactly which `connect_oauth_account` call is missing.

## Layer 3: `hmac_signed` (built-in)

For services that match the Binance shape exactly: append `<timestamp_param>=<unix-ms>` and `<signature_param>=<hex(hmac_sha256_or_512(secret, canonical_query))>` to the query string, send the key in `<key_header>`. No code. Just two credentials (`key_credential` and `secret_credential`) and the JSON layer block above.

If the service signs anything other than the query string (body, full path, headers, custom canonical form) — go to Layer 4.

## Layer 4: `wasm_signer`

For everything else. You write a small `no_std` Rust crate that compiles to `wasm32-unknown-unknown`, the engine sandboxes it via wasmtime, and it produces the per-request mutations.

### How the engine loads modules

- Drop `<name>.wasm` (and optional `<name>.manifest.json` sidecar) under `<workspace>/data/auth-modules/` (or ship them inside a plugin's `auth-modules/` directory, see `system-knowhow/plugins.md`; install auto-reloads).
- Engines that previously stored modules under `data/artifacts/auth-modules/` get a one-shot rename on next startup if the new path is empty; both-exist leaves both alone (operator pick).
- Engine scans the directory at startup (`proxy_wasm_signer::load_wasm_modules`).
- The LLM tool `reload_proxy_modules` (or `POST /api/v1/proxy-modules/reload`) re-scans and atomically swaps the compiled-module map. In-flight requests finish on the old module; new ones see the new map. No engine restart needed.
- A pipeline `wasm_signer` layer references a module by its basename. If the file is missing the engine returns 502 with an actionable message.

### Manifest sidecar

The sidecar carries WASM-host metadata only: secret expectations, body-mode preference, capability requests. It never carries provider config: `data/config/apis.json` is the single source of truth for `wasm_signer` entries, and the engine ignores any unknown fields here. Ship example `apis.json` snippets in the plugin's `setup` field (see `system-knowhow/plugins.md`) so the install-time LLM walks the user through wiring them up.

```json
{
  "secret_handles": ["api_secret"],
  "body_mode": "either",
  "capabilities": []
}
```

- `secret_handles` — logical names the module expects to find in `SignInput.secret_handles`. The provider config's `credential_handles` map these names → credential-store entries; the host vouches that each handle is valid for one invocation only.
- `body_mode`:
  - `"raw"` — module gets the full body bytes, but requests over 1MB are rejected with 413.
  - `"hash"` — host always passes a SHA-256 + length, never the raw bytes (signer opted out for privacy).
  - `"either"` (default) — host passes raw under 1MB, hash above.
- `capabilities` — currently only `"replace_body"` is recognized. Activating it requires both the manifest declaring the capability AND the provider config's `granted_capabilities` listing it. Either missing → 403.

### WASM ABI

Two required exports plus optional `alloc`:

```rust
// Required: module's own linear memory under the standard name.
(memory (export "memory") 1)

// Optional: host calls this to reserve space before writing SignInput JSON.
// If not exported, the host writes at fixed offset 8192.
#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32;

// Required: host calls this with (in_ptr, in_len) of the SignInput JSON.
// Return value packs (out_ptr << 32) | out_len pointing at SignOutput JSON.
#[no_mangle]
pub extern "C" fn sign(in_ptr: i32, in_len: i32) -> i64;
```

`SignInput` (host writes this into your memory):

```jsonc
{
  "method": "GET",
  "url": "https://api.binance.com/api/v3/account?recvWindow=5000",
  "headers": [["x-existing", "value"], ...],
  "body": {"type": "raw", "bytes": [...]}        // OR
                                                 // {"type": "hash_only", "sha256_hex": "...", "length": N}
  "prior_layer_outputs": { "<earlier-layer-namespace>": { ... } },
  "secret_handles": {"api_secret": 0, "api_key": 1},   // logical name → opaque u32 index
  "current_time_ns": 1700000000000000000
}
```

`SignOutput` (your `sign` returns a slice pointing at this):

```jsonc
{
  "add_headers": [["x-mbx-apikey", "..."], ["x-signature", "..."]],   // optional
  "add_query":   [["timestamp", "1700000000000"], ["signature", "..."]], // optional
  "replace_body": [104, 105, 33]                                       // optional, capability-gated
}
```

### Host imports

Available to every signer (in `extern "C"`-callable form):

```rust
extern "C" {
    fn current_time_ns() -> i64;
    fn current_time_secs() -> i64;
    fn random_bytes(out_ptr: i32, out_len: i32) -> i32;            // → bytes written, -1 on error

    fn sha1   (data_ptr: i32, data_len: i32, out_ptr: i32) -> i32; // → digest length
    fn sha256 (data_ptr: i32, data_len: i32, out_ptr: i32) -> i32;
    fn sha512 (data_ptr: i32, data_len: i32, out_ptr: i32) -> i32;

    // Secret is referenced by its opaque handle (the i32 from SignInput.secret_handles).
    // The signer never sees raw secret bytes.
    fn hmac_sha1  (secret_id: i32, data_ptr: i32, data_len: i32, out_ptr: i32) -> i32;
    fn hmac_sha256(secret_id: i32, data_ptr: i32, data_len: i32, out_ptr: i32) -> i32;
    fn hmac_sha512(secret_id: i32, data_ptr: i32, data_len: i32, out_ptr: i32) -> i32;

    fn hex_encode   (in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
    fn base64_encode(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;

    fn log(ptr: i32, len: i32);   // host-side log line, prefixed `[wasm-signer:<name>]`
}
```

Source: `crates/lucidos-engine/src/api/proxy_wasm_host.rs`. All primitives are sync (CPU-bound, microseconds).

### Prefer pure-Rust crypto/encoding inside the signer

The host imports for `sha1`/`sha256`/`sha512` and `hex_encode`/`base64_encode` exist for convenience, but reach for them only after weighing the alternative: inline a `no_std` Rust implementation in the signer itself. SHA-256 is ~150 lines of pure arithmetic, hex is ~10, base64 is ~30 — all freely copy-pastable, all reviewed implementations exist on crates.io as no_std crates (`sha2` with `default-features = false`, `hex` with `default-features = false`, `base64` with `default-features = false`). Costs ~5 KB of wasm; the signer becomes self-contained, portable across engine versions, and has no ABI surface to keep in lockstep with the host.

Reserve host imports for things wasm genuinely cannot do alone:

- `current_time_ns` / `current_time_secs` — wasm has no clock.
- `random_bytes` — wasm has no entropy source.
- `hmac_sha1` / `hmac_sha256` / `hmac_sha512` — these take an opaque `secret_id` so the raw key bytes never enter wasm memory. **Always use the host import for HMAC**, never derive HMAC inside the signer (you'd need the secret in wasm, which defeats the handle indirection).
- `log` — for engine-side observability.

Same principle outside the signer: never shell out to `sha256sum` / `shasum` / `openssl dgst` from a build script, test, or handshake script. Pure-language implementations are reproducible; system commands depend on which Unix you're on (`sha256sum` is GNU coreutils, `shasum` is BSD/Perl, neither is guaranteed on the bundled-installer machines Lucidos ships to).

### Authoring a signer

Standalone crate with its own `[workspace]` (so it doesn't pollute the engine's lockfile or default-target builds):

```toml
# signers/<name>/Cargo.toml
[workspace]   # <- yes, an empty `[workspace]` table. Standalone.

[package]
name = "<name>"
version = "0.1.0"
edition = "2021"
publish = false

[lib]
crate-type = ["cdylib"]

[profile.release]
opt-level = "s"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

```rust
// signers/<name>/src/lib.rs
#![no_std]
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { core::arch::wasm32::unreachable() }

// Bump allocator over a static heap. WASM is single-threaded inside an
// instance, so unsynchronized access is sound.
const HEAP_SIZE: usize = 256 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_OFFSET: usize = 0;

#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    if size < 0 { return 0; }
    let need = size as usize;
    unsafe {
        let off = core::ptr::addr_of!(HEAP_OFFSET).read();
        if off + need > HEAP_SIZE { return 0; }
        let ptr = core::ptr::addr_of!(HEAP) as usize + off;
        core::ptr::addr_of_mut!(HEAP_OFFSET).write(off + need);
        ptr as i32
    }
}

extern "C" {
    fn current_time_ns() -> i64;
    fn hmac_sha256(secret_id: i32, data_ptr: i32, data_len: i32, out_ptr: i32) -> i32;
    fn hex_encode(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
}

#[no_mangle]
pub extern "C" fn sign(in_ptr: i32, in_len: i32) -> i64 {
    // 1. Read SignInput from (in_ptr, in_len).
    // 2. Compute whatever your signature needs.
    // 3. Write SignOutput JSON into a fresh alloc'd region.
    // 4. Return ((out_ptr as i64) << 32) | (out_len as i64).
    0   // 0 packs (ptr=0, len=0) — host parses as empty / fails to deserialize.
}
```

### Build + install

```bash
# From repo root:
./signers/build-all.sh <name>
# → produces signers/<name>/<name>.wasm

# Then deploy into the workspace:
cp signers/<name>/<name>.wasm   <workspace>/data/auth-modules/<name>.wasm
cp signers/<name>/manifest.json <workspace>/data/auth-modules/<name>.manifest.json

# Tell the engine to pick it up — no restart:
#   LLM:  call the `reload_proxy_modules` tool
#   HTTP: POST /api/v1/proxy-modules/reload
```

Or distribute the signer inside a plugin so other workspaces can install it as a unit -- ship `<name>.wasm` + `<name>.manifest.json` under the plugin's `auth-modules/` directory, and `install_plugin` lands them at `data/auth-modules/...` AND auto-reloads the WASM signer map. No `cp` + manual reload needed. See `system-knowhow/plugins.md` for plugin authoring; pair the signer with a `setup` field in `manifest.toml` that walks the user through wiring `apis.json` and registering credentials, since those are workspace state and don't ship in plugins.

The build script targets `wasm32-unknown-unknown` in release, then copies the artifact (cargo flattens hyphens to underscores in the artifact name) to `signers/<name>/<name>.wasm`.

### Worked example: Binance HMAC

Already in the tree at `signers/binance-hmac/`. Algorithm:

1. Find the existing query string in `SignInput.url` (after `?`).
2. Build canonical = `<existing>&timestamp=<ms>` (current time / 1e6).
3. `hmac_sha256(SECRET_ID_API_SECRET, canonical, ...)` — secret stays in the host's per-call table.
4. `hex_encode(...)` → 64 ASCII bytes.
5. Emit `SignOutput { add_query: [["timestamp", ms], ["signature", hex]] }`.

`signers/binance-hmac/manifest.json`:

```json
{ "secret_handles": ["api_secret"], "body_mode": "either", "capabilities": [] }
```

Provider config:

```json
{
  "binance": {
    "base_url": "https://api.binance.com",
    "auth": {
      "pipeline": [
        {"type": "static_credential", "kind": "api_key",
         "credential": "binance-key", "header": "X-MBX-APIKEY"},
        {"type": "wasm_signer", "module": "binance-hmac",
         "credential_handles": [{"name": "api_secret", "credential": "binance-secret"}]}
      ]
    }
  }
}
```

The static layer attaches the API key header; the WASM layer adds the timestamp + signature query params. Note this overlaps with the built-in `hmac_signed` layer — for Binance specifically you can use either. The WASM version is the reference example for porting other exchanges.

## Composing layers

The pipeline runs layers in declared order. Each layer sees `prior_layer_outputs[<earlier-layer-namespace>]` (a JSON map keyed by layer kind: `script_handshake`, `wasm_signer`, etc.). Use this when a per-request signer needs a value the login script just minted:

```json
{
  "auth": {
    "pipeline": [
      {"type": "script_handshake", "credential": "comfort-pw",
       "script": "scripts/auth/comfort-login.py"},
      {"type": "wasm_signer", "module": "comfort-cloud-hmac",
       "credential_handles": [{"name": "api_secret", "credential": "comfort-secret"}]}
    ]
  }
}
```

Inside `comfort-cloud-hmac` the signer reads `prior_layer_outputs["script_handshake"]["headers"]["x-cfc-auth-token"]` — the script_handshake layer publishes its emitted headers as JSON for downstream consumers (`proxy_script_layer.rs:166-180`).

## 401 retry

After every forwarded request, the pipeline asks each layer "did your `apply` come from a cache?" and "would you like a retry on 401?". If upstream returns 401 AND any cache-hit layer opted into `InvalidateAndRetry`, the engine invalidates those caches and runs the pipeline once more. `script_handshake` opts in (cached headers might have rotated). `wasm_signer`, `hmac_signed`, and `static_credential` are stateless — a fresh signature still failing is a real auth failure, surfaced as-is. (`proxy_pipeline.rs:118-154`.)

## Pitfalls

- **Reaching for `wasm_signer` when `hmac_signed` would do.** If your service is "Binance-shape" (sign the query string, append timestamp + signature), use the built-in. The WASM example exists to show the pattern, not because Binance needs custom code.
- **Reaching for `script_handshake` when a static header would do.** A Bearer token that doesn't rotate is a `static_credential` block, not a script.
- **Forgetting the manifest sidecar.** Without it the loader applies defaults: no `secret_handles`, `body_mode = "either"`, no `capabilities`. A signer that calls `hmac_sha256(SECRET_ID_API_SECRET, ...)` then sees an out-of-bounds handle and gets `-1` back. Symptom: 502 with no signature.
- **`replace_body` without both the manifest cap and the provider grant.** Layer returns 403. Both halves must be set.
- **Pinning a hard-coded `secret_id`.** The order of `credential_handles` in the provider config determines indices. Read `SignInput.secret_handles[<your-name>]` from the JSON instead of hardcoding `0`.
- **Not exporting `memory`.** The host looks up the standard name `memory` and 502s otherwise. `cdylib` builds emit one by default — but explicitly declaring it (`(memory (export "memory") 1)`) makes the contract obvious.
- **`alloc` returning the same pointer twice.** Bump-allocate or you'll write SignOutput on top of SignInput before the host reads it.
- **`expires_in` too long in a script_handshake.** Better to refresh more often than serve requests with a token that expired five minutes ago. The 401 retry helps but each one is a user-visible blip.
- **Editing a handshake script outside Lucidos and expecting it to run.** The
  save works and the script stops running, because nothing recorded the new
  content. Ask the agent to make the edit, or approve it afterwards.
- **A credential pointed at the wrong provider.** A credential is only sent to
  a base URL it declares, so an entry pointing elsewhere answers 502 naming the
  whole declared set. Fix the credential's base URLs in Settings, or the entry's
  `base_url`.
- **One key serving two hostnames of one provider.** Binance signs spot calls at
  `api.binance.com` and futures calls at `fapi.binance.com` with the same HMAC
  pair, so the credential must declare BOTH. It is a set, and every member is
  exact: there is no wildcard and nothing is inferred from a host's spelling.
  Add the second host in Settings, or run:

  ```bash
  lucidos credentials set-base-urls --name binance-key \
    --url https://api.binance.com --url https://fapi.binance.com
  ```

  `set-base-urls` REPLACES the set, so pass every host. `lucidos credentials
  list` prints what each credential covers today.
- **Logging credentials inside a script_handshake.** Stderr is captured by the engine and surfaced. Don't `print(username)` or write secrets to a file.
- **WASM module too big or too slow.** wasmtime instantiates per request. Keep the heap small (the binance-hmac signer uses 256KB) and avoid pulling heavy crates — `no_std` + `panic = "abort"` keeps it tight.
- **Cryptic `incompatible import type` from wasmtime when the imports look right.** Almost always means the running engine binary pre-dates the host import you're calling — the source repo has the new function but the installed engine was built before the commit that added it (verify with `strings <engine-binary> | grep <import-name>`). Two fixes: rebuild + restart the engine, OR drop the host import and inline a pure-Rust replacement in the signer (see § Prefer pure-Rust crypto/encoding). The second is the more durable fix — the signer no longer depends on the engine's host-import revision.

## Testing locally

- `./scripts/test-engine.sh -- -- proxy_` covers the pipeline runner, layer impls, config parsing, migration, and the WASM host imports. Several of those tests need Postgres, which the script provisions.
- The Rust-source signers live outside the workspace, so their `.wasm` must exist before a test can load it. `./scripts/e2e-wasm.sh` does both halves: it runs `./signers/build-all.sh`, then the real-artifact tests in `crates/lucidos-e2e/tests/wasm_signers.rs`. No running workspace needed.
  ```bash
  ./scripts/e2e-wasm.sh                                                # every signer, both tests
  ./scripts/e2e-wasm.sh -- wasm_signer_layer_runs_binance_hmac_signer  # filter to one test
  ```
- For a fresh signer template, `signers/test-echo/` is the smallest possible working module — copy it, change the response, build, drop into `data/auth-modules/`, call `reload_proxy_modules`.
