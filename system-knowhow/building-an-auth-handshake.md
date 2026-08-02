---
name: Building an Auth Handshake
description: Use when wiring an external API whose authentication is more than attaching a static header, with phrases like "can't log into X", "needs a session token", "Comfort Cloud", "OAuth password grant", "HMAC signing", "sign every request", "Binance / Coinbase / Kraken / exchange API", or any service that needs per-request signatures, login dances that mint short-lived tokens, or chained auth steps. Also use when authoring or debugging a WASM signer module, with phrases like "wasm signer", "sign request body", "build a no_std wasm module", "wasmtime incompatible import type", "host import sha256/hmac/hex", or when about to inline crypto/encoding inside a signer or shell out to `sha256sum`/`shasum`/`openssl dgst` from a build script or handshake. Covers the proxy auth pipeline (`apis.json` → `auth.pipeline: [...]`), the four layer types (`static_credential`, `script_handshake`, `hmac_signed`, `wasm_signer`), how to author a no_std WASM signer module under `signers/<name>/` and install it as `data/auth-modules/<name>.wasm` (or ship it inside a plugin under `auth-modules/` per `system-knowhow/plugins.md`), the WASM ABI (`alloc`/`sign`, packed `(out_ptr,out_len)` return, JSON `SignInput`/`SignOutput`), the host imports a signer can call and which ones to inline pure-Rust instead (sha/hex/base64) vs. which to keep as host calls (clock, randomness, hmac with opaque secret handles), manifest sidecars (`secret_handles`, `body_mode`, `capabilities`), the `replace_body` capability gate, the 1MB body threshold, reloading via the `reload_proxy_modules` LLM tool (auto-fired after `install_plugin` ships an `auth-modules/` file), composing layers (e.g. login script → per-request HMAC), and a worked example using the existing `signers/binance-hmac/` module. Also covers when the simpler `bearer` / `api_key` / `basic` / `query_param` modes are sufficient and you should NOT reach for a script or signer.
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
    "auth": {
      "pipeline": [
        // one or more layer entries, in execution order
      ],
      "granted_capabilities": []   // e.g. ["replace_body"] — see WASM section
    }
  }
}
```

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

Old single-variant configs (`{"auth": {"type": "bearer", ...}}`) are auto-upgraded to the pipeline shape on engine startup, with a `apis.json.bak.<unix>` backup written next to the live file (`proxy_migration.rs`). `credential_bundle` is permanently removed and refused at startup with an actionable error.

## Layer 1: `static_credential`

Use the LLM `request_credential` tool (or the credentials UI) to register the credential, then add a one-line entry to `apis.json`. No code. See `system-knowhow/lucidos-cli.md` for the call syntax.

## Layer 2: `script_handshake`

For login dances. The engine caches the script's output until `expires_in` elapses, singleflights concurrent first-time requests so only one `python3` runs, and on a 401 from upstream invalidates the cache and retries once.

### Script contract

- **Where the file lives.** `script` is resolved relative to `data/` — the file at `data/scripts/auth/foo.py` is referenced as `"script": "scripts/auth/foo.py"`. Keep handshake scripts under `data/` so they're git-tracked. (A legacy script placed at the workspace root still resolves as a back-compat fallback, but move it under `data/`.)
- **`credential` is optional.** When the layer config sets `credential`, that credential is injected as env vars (shape below) before the script runs. When it's omitted, no `CRED_*` env var is injected from this layer — the script must source its secret by other means (read a rotating token from the OS keychain, do an OAuth-only exchange, etc.). The env-var table below applies only when a credential is configured.
- Reads the named credential from env vars. The shape depends on the credential's type — same convention `run_python` / `run_bash` already inject for their subprocesses, so a script you wrote for one works for the other:

  | Credential type | Env vars injected |
  |---|---|
  | `password` | `CRED_<NAME>_USERNAME` + `CRED_<NAME>_PASSWORD` (split out of the stored JSON) |
  | `api_key`  | `CRED_<NAME>` (the raw key) |
  | `bearer`   | `CRED_<NAME>` (the raw token) |
  | `basic`    | `CRED_<NAME>` (the raw `user:password` string — split it yourself if you need the parts) |

  Transform for `<NAME>`: uppercase the credential's `service_name`, replace `-`/`.`/space with `_`. So `comfort-cloud` (password) → `CRED_COMFORT_CLOUD_USERNAME` + `CRED_COMFORT_CLOUD_PASSWORD`; `firebase-web-api-key` (api_key) → `CRED_FIREBASE_WEB_API_KEY`. There is no type restriction — pick whichever credential type honestly describes the secret and the script reads the matching env var.
- **Optional OAuth env vars.** When the layer config lists `oauth_providers: ["<name>", ...]`, the engine looks up each provider's connected OAuth account (auto-refreshing the access token if expired) and injects `OAUTH_<UPPER>_ACCESS_TOKEN` (always) and `OAUTH_<UPPER>_EMAIL` (when known). Same name transform as `CRED_*`. So `oauth_providers: ["google"]` against a connected Google account exposes `OAUTH_GOOGLE_ACCESS_TOKEN` (and `OAUTH_GOOGLE_EMAIL` when the user's email is on the account). If the user hasn't connected the requested provider, the layer fails with a 502 naming the missing provider — the script never runs and the user gets a clear hint to invoke `connect_oauth_account` first.
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
- **Logging credentials inside a script_handshake.** Stderr is captured by the engine and surfaced. Don't `print(username)` or write secrets to a file.
- **WASM module too big or too slow.** wasmtime instantiates per request. Keep the heap small (the binance-hmac signer uses 256KB) and avoid pulling heavy crates — `no_std` + `panic = "abort"` keeps it tight.
- **Cryptic `incompatible import type` from wasmtime when the imports look right.** Almost always means the running engine binary pre-dates the host import you're calling — the source repo has the new function but the installed engine was built before the commit that added it (verify with `strings <engine-binary> | grep <import-name>`). Two fixes: rebuild + restart the engine, OR drop the host import and inline a pure-Rust replacement in the signer (see § Prefer pure-Rust crypto/encoding). The second is the more durable fix — the signer no longer depends on the engine's host-import revision.

## Testing locally

- `cargo test -p lucidos-engine --lib proxy_` covers the pipeline runner, layer impls, config parsing, migration, and the WASM host imports.
- The Rust-source signers live outside the workspace. Build with `./signers/build-all.sh` (or `./signers/build-all.sh binance-hmac` for one), then run the `#[ignore]`-marked end-to-end tests:
  ```bash
  cargo test -p lucidos-engine --lib wasm_signer_layer_runs_binance_hmac_signer -- --ignored
  cargo test -p lucidos-engine --lib wasm_signer_layer_runs_test_echo_wasm     -- --ignored
  ```
- For a fresh signer template, `signers/test-echo/` is the smallest possible working module — copy it, change the response, build, drop into `data/auth-modules/`, call `reload_proxy_modules`.
