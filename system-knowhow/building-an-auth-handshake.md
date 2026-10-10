---
name: Building an Auth Handshake
description: Authenticating an external API when a static header is not enough: the apis.json proxy auth pipeline and authoring a no_std WASM signer. Load for "can't log into X", "needs a session token", "OAuth password grant", "HMAC signing", "sign every request", "wasm signer", or "wasmtime incompatible import type".
---

# Building an auth handshake

The proxy auth pipeline runs on every outbound `/api/v1/proxy/<api>/...` call. A provider's `auth` block in `data/config/apis.json` is a list of **layers** that run in declared order. Each layer can attach headers, append query params, or (with explicit consent) replace the body. Then the engine forwards the request. On a 401 from upstream, layers that opted into `InvalidateAndRetry` lose their caches and the pipeline runs once more.

| Layer | Use for | Per-request work |
|---|---|---|
| `static_credential` (kind: `bearer` / `api_key` / `basic` / `query_param`) | A header or query param that doesn't rotate. | None: value baked in. |
| `script_handshake` | Login dance that mints a short-lived session token, cached until expiry. Python script you write. | Cache hit: free. Miss: spawn `python3`, parse JSON. |
| `hmac_signed` | Built-in Binance-shape HMAC over the query string. | HMAC compute, no cache. |
| `wasm_signer` | Anything more elaborate. Sandboxed `no_std` Rust → wasm32. | Compile-once, instantiate-per-call. |

For a single static header, use `static_credential` (see `system-knowhow/lucidos-cli.md` § `lucidos proxy`). The other three layers cost more.

**Don't know the API's shape yet?** If the user is logged into a site with no
public API, derive one: see `system-knowhow/deriving-an-api-from-a-site.md`.
Then come back here to pick the layer.

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
answer 502 with the reason. See `system-knowhow/lucidos-cli.md` § Timeouts.

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

For login dances. The engine caches the script's output until `expires_in` elapses. It singleflights concurrent first-time requests so only one `python3` runs. On a 401 from upstream it invalidates the cache and retries once.

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
- Env var shape depends on the credential's type. It is the same convention `run_python` / `run_bash` use, so a script works under either:

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
- **A handshake script gets only `CRED_*` and `OAUTH_*` names, never a credential's custom env var name.** A custom name such as `GITHUB_TOKEN` reaches `run_bash` and `run_python`, but the engine drops it here and logs the drop. It passes only `CRED_` or `OAUTH_` followed by `A-Z`, `0-9` and `_`. So no name can set a loader hook (`PYTHONPATH`), a proxy (`HTTPS_PROXY`) or a runtime name (`PATH`). Read the canonical `CRED_<NAME>`, which always arrives.
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

For services that match the Binance shape exactly: append `<timestamp_param>=<unix-ms>` and `<signature_param>=<hex(hmac_sha256_or_512(secret, canonical_query))>` to the query string, and send the key in `<key_header>`. No code: two credentials (`key_credential` and `secret_credential`) and the JSON layer block above.

If the service signs anything other than the query string (body, full path, headers, custom canonical form), go to Layer 4.

## Layer 4: `wasm_signer`

For everything else. You write a small `no_std` Rust crate that compiles to `wasm32-unknown-unknown`. The engine sandboxes it via wasmtime, and it produces the per-request mutations.

### How the engine loads modules

- Drop `<name>.wasm` (and optional `<name>.manifest.json` sidecar) under `<workspace>/data/auth-modules/`. Or ship them in a plugin's `auth-modules/` directory (see `system-knowhow/plugins.md`); install auto-reloads.
- Modules under the old `data/artifacts/auth-modules/` get a one-shot rename on next startup if the new path is empty. If both exist, the engine leaves both alone for the operator to pick.
- Engine scans the directory at startup (`proxy_wasm_signer::load_wasm_modules`).
- The LLM tool `reload_proxy_modules` (or `POST /api/v1/proxy-modules/reload`) re-scans and atomically swaps the compiled-module map. In-flight requests finish on the old module; new ones see the new map. No engine restart needed.
- A pipeline `wasm_signer` layer references a module by its basename. If the file is missing the engine returns 502 with an actionable message.

### Manifest sidecar

The sidecar carries WASM-host metadata only: secret expectations, body-mode preference, capability requests. It never carries provider config: `data/config/apis.json` is the single source of truth for `wasm_signer` entries, and the engine ignores unknown fields here. Ship example `apis.json` snippets in the plugin's `setup` field (see `system-knowhow/plugins.md`) so the install-time LLM walks the user through wiring them.

```json
{
  "secret_handles": ["api_secret"],
  "body_mode": "either",
  "capabilities": []
}
```

- `secret_handles`: logical names the module expects in `SignInput.secret_handles`. The provider config's `credential_handles` map these names → credential-store entries. Each handle is valid for one invocation only.
- `body_mode`:
  - `"raw"`: module gets the full body bytes, but requests over 1MB are rejected with 413.
  - `"hash"`: host always passes a SHA-256 + length, never the raw bytes (signer opted out for privacy).
  - `"either"` (default): host passes raw under 1MB, hash above.
- `capabilities`: only `"replace_body"` is recognized. It needs both the manifest declaring it AND the provider config's `granted_capabilities` listing it. Either missing → 403.

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

The `sha1`/`sha256`/`sha512` and `hex_encode`/`base64_encode` imports are a convenience. Prefer inlining a `no_std` Rust implementation in the signer. SHA-256 is ~150 lines of pure arithmetic, hex ~10, base64 ~30. Reviewed `no_std` crates exist: `sha2`, `hex` and `base64`, each with `default-features = false`. It costs ~5 KB of wasm. The signer becomes self-contained and portable across engine versions, with no ABI to keep in lockstep with the host.

Reserve host imports for things wasm cannot do alone:

- `current_time_ns` / `current_time_secs`: wasm has no clock.
- `random_bytes`: wasm has no entropy source.
- `hmac_sha1` / `hmac_sha256` / `hmac_sha512`: these take an opaque `secret_id`, so the raw key bytes never enter wasm memory. **Always use the host import for HMAC.** Deriving it inside the signer needs the secret in wasm, which defeats the handle.
- `log`: for engine-side observability. Each invocation gets a budget. A line prints at most 4 KiB, and the host cuts the rest with a `[truncated, N more bytes]` marker. An invocation prints at most 32 lines or 16 KiB. Past that, one `log budget exhausted` line prints and later calls print nothing.

The same holds outside the signer: never shell out to `sha256sum` / `shasum` / `openssl dgst` from a build script, test, or handshake script. `sha256sum` is GNU coreutils, `shasum` is BSD/Perl, and Lucidos installs guarantee neither.

### Authoring a signer

Standalone crate with its own `[workspace]`, so it stays out of the engine's lockfile and default-target builds:

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
    0   // 0 packs (ptr=0, len=0): host parses as empty / fails to deserialize.
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

# Tell the engine to pick it up, no restart:
#   LLM:  call the `reload_proxy_modules` tool
#   HTTP: POST /api/v1/proxy-modules/reload
```

Or distribute the signer inside a plugin, so other workspaces install it as a unit. Ship `<name>.wasm` + `<name>.manifest.json` under the plugin's `auth-modules/` directory. `install_plugin` lands them at `data/auth-modules/...` AND auto-reloads the WASM signer map, with no `cp` or manual reload. See `system-knowhow/plugins.md`. Pair the signer with a `setup` field in `manifest.toml` that walks the user through wiring `apis.json` and registering credentials. Those are workspace state and don't ship in plugins.

The build script targets `wasm32-unknown-unknown` in release, then copies the artifact (cargo flattens hyphens to underscores in its name) to `signers/<name>/<name>.wasm`.

### Worked example: Binance HMAC

Already in the tree at `signers/binance-hmac/`. Algorithm:

1. Find the existing query string in `SignInput.url` (after `?`).
2. Build canonical = `<existing>&timestamp=<ms>` (current time / 1e6).
3. `hmac_sha256(SECRET_ID_API_SECRET, canonical, ...)`: the secret stays in the host's per-call table.
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

The static layer attaches the API key header; the WASM layer adds the timestamp + signature query params. For Binance the built-in `hmac_signed` layer works too. The WASM version is the reference for porting other exchanges.

## Composing layers

The pipeline runs layers in declared order. Each layer sees `prior_layer_outputs[<earlier-layer-namespace>]`, a JSON map keyed by layer kind (`script_handshake`, `wasm_signer`, etc.). Use it when a per-request signer needs a value the login script just minted:

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

Inside `comfort-cloud-hmac` the signer reads `prior_layer_outputs["script_handshake"]["headers"]["x-cfc-auth-token"]`. The script_handshake layer publishes its emitted headers as JSON for downstream layers (`proxy_script_layer.rs`).

## 401 retry

After every forwarded request, the pipeline asks each layer two things: did your `apply` come from a cache, and do you want a retry on 401? If upstream returns 401 AND any cache-hit layer opted into `InvalidateAndRetry`, the engine invalidates those caches and runs the pipeline once more. `script_handshake` opts in (cached headers might have rotated). `wasm_signer`, `hmac_signed`, and `static_credential` are stateless: a fresh signature that still fails is a real auth failure, surfaced as-is (`proxy_pipeline.rs`).

## Pitfalls

- **Reaching for a heavier layer than needed.** A Bearer token that doesn't rotate is `static_credential`, not a script. A "Binance-shape" service (sign the query string, append timestamp + signature) is `hmac_signed`, not a signer.
- **Forgetting the manifest sidecar.** Without it the loader applies defaults: no `secret_handles`, `body_mode = "either"`, no `capabilities`. A signer that calls `hmac_sha256(SECRET_ID_API_SECRET, ...)` then sees an out-of-bounds handle and gets `-1` back. Symptom: 502 with no signature.
- **`replace_body` without both the manifest cap and the provider grant.** Layer returns 403 (see § Manifest sidecar).
- **Pinning a hard-coded `secret_id`.** The order of `credential_handles` in the provider config sets the indices. Read `SignInput.secret_handles[<your-name>]` instead of hardcoding `0`.
- **Not exporting `memory`.** The host looks up the standard name `memory` and 502s otherwise. `cdylib` builds emit one by default, but declaring it (`(memory (export "memory") 1)`) makes the contract obvious.
- **`alloc` returning the same pointer twice.** Bump-allocate, or SignOutput overwrites SignInput before the host reads it.
- **`expires_in` too long in a script_handshake.** Refresh early rather than serve an expired token. The 401 retry helps, but each one is a user-visible blip.
- **Editing a handshake script outside Lucidos and expecting it to run.** It
  stops running until approved (see § Script contract).
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
- **Pointing an entry at a Lucidos engine or gateway.** Every proxied request
  carries the `x-lucidos-proxied` header, and every engine answers it with
  403. An app reaches the engine through the SDK, never through the proxy.
- **Logging credentials inside a script_handshake.** The engine captures and surfaces stderr. Don't `print(username)` or write secrets to a file.
- **WASM module too big or too slow.** wasmtime instantiates per request. Keep the heap small (the binance-hmac signer uses 256KB) and avoid heavy crates: `no_std` + `panic = "abort"` keeps it tight.
- **Cryptic `incompatible import type` from wasmtime when the imports look right.** The running engine binary almost always pre-dates the host import you call. Verify with `strings <engine-binary> | grep <import-name>`. Two fixes: rebuild + restart the engine, OR inline a pure-Rust replacement in the signer (see § Prefer pure-Rust crypto/encoding). The second is more durable: the signer no longer depends on the engine's host-import revision.

## Testing locally

- `./scripts/test-engine.sh -- -- proxy_` covers the pipeline runner, layer impls, config parsing, migration, and the WASM host imports. The script provisions the Postgres some of them need.
- The signers live outside the workspace, so their `.wasm` must exist before a test can load it. `./scripts/e2e-wasm.sh` runs `./signers/build-all.sh`, then the real-artifact tests in `crates/lucidos-e2e/tests/wasm_signers.rs`. No running workspace needed.
  ```bash
  ./scripts/e2e-wasm.sh                                                # every signer, both tests
  ./scripts/e2e-wasm.sh -- wasm_signer_layer_runs_binance_hmac_signer  # filter to one test
  ```
- For a fresh signer template, copy `signers/test-echo/`, the smallest working module. Change the response, build, drop it into `data/auth-modules/`, and call `reload_proxy_modules`.
