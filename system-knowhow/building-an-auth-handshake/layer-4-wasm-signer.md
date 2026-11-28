---
name: Building an Auth Handshake, Layer 4: wasm_signer
description: Part of Building an Auth Handshake. Its index lists the sections it covers.
---

One part of `system-knowhow/building-an-auth-handshake`, which lists every part.

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
- `capabilities`: supports `"replace_body"` and `"read_prior_headers"`.
  Each requires the manifest declaration and the provider's
  `auth.granted_capabilities` grant.
  - `replace_body` permits a signer to replace the request body. An attempted
    replacement without both entries returns 403.
  - `read_prior_headers` exposes earlier layers' header values to the signer.
    Without both entries, every value becomes
    `"[withheld: grant read_prior_headers]"`; header names remain visible.

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

Source: `crates/lucidos-engine/src/engine/proxy_wasm_host.rs`. All primitives are sync (CPU-bound, microseconds).

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

Or distribute the signer inside a plugin, so other workspaces install it as a unit. Ship `<name>.wasm` + `<name>.manifest.json` under the plugin's `auth-modules/` directory. `plugins(action="install")` lands them at `data/auth-modules/...` AND auto-reloads the WASM signer map, with no `cp` or manual reload. See `system-knowhow/plugins.md`. Pair the signer with a `setup` field in `manifest.toml` that walks the user through wiring `apis.json` and registering credentials. Those are workspace state and don't ship in plugins.

The build script targets `wasm32-unknown-unknown` in release, then copies the artifact (cargo flattens hyphens to underscores in its name) to `signers/<name>/<name>.wasm`.

### Worked example: Binance HMAC

Already in the tree at `signers/binance-hmac/`. Algorithm:

1. Find the existing query string in `SignInput.url` (after `?`).
2. Build canonical = `<existing>&timestamp=<ms>` (current time / 1e6).
3. `signers/binance-hmac/src/lib.rs::hmac_sha256` with the secret handle and canonical bytes: the secret stays in the host's per-call table.
4. `signers/binance-hmac/src/lib.rs::hex_encode` → 64 ASCII bytes.
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

The pipeline runs layers in declared order. Each layer sees
`prior_layer_outputs[<earlier-layer-namespace>]`.
The first layer's namespace is its kind; subsequent namespaces append
`_<zero-based-position>`, even when their kinds differ.
Static credential kinds use `bearer`, `api_key`, `basic` or `query_param`.
For example, a handshake at position 1 publishes under `script_handshake_1`.

A signer reading a login token needs `read_prior_headers` declared in its
sidecar and granted in the provider's `auth` configuration.
For `comfort-cloud-hmac.manifest.json`:

```json
{
  "secret_handles": ["api_secret"],
  "body_mode": "either",
  "capabilities": ["read_prior_headers"]
}
```

The matching provider configuration:

```json
{
  "auth": {
    "pipeline": [
      {"type": "script_handshake", "credential": "comfort-pw",
       "script": "scripts/auth/comfort-login.py"},
      {"type": "wasm_signer", "module": "comfort-cloud-hmac",
       "credential_handles": [{"name": "api_secret", "credential": "comfort-secret"}]}
    ],
    "granted_capabilities": ["read_prior_headers"]
  }
}
```

Inside `comfort-cloud-hmac`, the signer reads
`prior_layer_outputs["script_handshake"]["headers"]["x-cfc-auth-token"]`.
The handshake is at position 0 here, so its namespace has no suffix.
It publishes emitted headers as JSON for downstream layers (`proxy_script_layer.rs`).

A missing declaration or grant replaces the token with the withholding placeholder.
Signing that placeholder produces an invalid signature and an upstream 401.
Check both capability entries when diagnosing this failure.

## 401 retry

After every forwarded request, the pipeline asks each layer two things: did your `apply` come from a cache, and do you want a retry on 401? If upstream returns 401 AND any cache-hit layer opted into `InvalidateAndRetry`, the engine invalidates those caches and runs the pipeline once more. `script_handshake` opts in (cached headers might have rotated). `wasm_signer`, `hmac_signed`, and `static_credential` are stateless: a fresh signature that still fails is a real auth failure, surfaced as-is (`proxy_pipeline.rs`).

## Pitfalls

- **Reaching for a heavier layer than needed.** A Bearer token that doesn't rotate is `static_credential`, not a script. A "Binance-shape" service (sign the query string, append timestamp + signature) is `hmac_signed`, not a signer.
- **Forgetting the manifest sidecar.** Without it the loader applies defaults: no `secret_handles`, `body_mode = "either"`, no `capabilities`. A signer that calls `signers/binance-hmac/src/lib.rs::hmac_sha256` then sees an out-of-bounds handle and gets `-1` back. Symptom: 502 with no signature.
- **`replace_body` without both the manifest cap and the provider grant.** Layer returns 403 (see § Manifest sidecar).
- **Expecting an injected value back in the reply.** The engine treats every header and query value a layer adds as a credential. It masks each one as `[REDACTED]` in the response headers, body and error text, so an echoed `timestamp` comes back masked too. Read a timestamp from the reply's own fields, not from your request's echo. A credentialed call also asks for an uncompressed reply, and an upstream that compresses anyway gets a 502.
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

## Diagnostic evidence

Upstream HTTP 500 responses remain provider failures, not internal Lucidos faults.
Proxy diagnostic lines mask registered credentials before storage.

Follow [Agent Sandbox](../agent-sandbox.md) for bounded diagnostic access and the raw-host-log restriction.
