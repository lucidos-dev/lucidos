---
name: Building an Auth Handshake
description: Authenticating an external API when a static header is not enough: the apis.json proxy auth pipeline and authoring a no_std WASM signer. Load for "can't log into X", "needs a session token", "OAuth password grant", "HMAC signing", "sign every request", "wasm signer", or "wasmtime incompatible import type".
---

# Building an auth handshake

The proxy auth pipeline runs on every outbound `/api/v1/proxy/<api>/...` call. A provider's `auth` block in `data/config/apis.json` is a list of **layers** that run in declared order. Each layer can attach headers, append query params, or (with explicit consent) replace the body. Then the engine forwards the request. On a 401 from upstream, layers that opted into `InvalidateAndRetry` lose their caches and the pipeline runs once more.

Model route metadata reports public protocol defaults without resolving credentials
or refreshing tokens. SDK `modelRequest` builds a payload; sending it through
`lucidos.proxy` still uses the ordinary proxy authentication path.

| Layer | Use for | Per-request work |
|---|---|---|
| `static_credential` (kind: `bearer` / `api_key` / `basic` / `query_param`) | A header or query param that doesn't rotate. | None: value baked in. |
| `script_handshake` | Login dance that mints a short-lived session token, cached until expiry. Python script you write. | Cache hit: free. Miss: spawn `python3`, parse JSON. |
| `hmac_signed` | Built-in Binance-shape HMAC over the query string. | HMAC compute, no cache. |
| `wasm_signer` | Anything more elaborate. Sandboxed `no_std` Rust → wasm32. | Compile-once, instantiate-per-call. |

For a single static header, use `static_credential` (see `system-knowhow/lucidos-cli/subcommands-13.md` § `lucidos proxy`). The other three layers cost more.

**Don't know the API's shape yet?** If the user is logged into a site with no
public API, derive one: see `system-knowhow/deriving-an-api-from-a-site.md`.
Then come back here to pick the layer.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/building-an-auth-handshake/builtin-provider-sign-in` | Builtin provider sign-in; Decision tree; On-disk schema (`insecure_transport`: off by default, and say why you turned it on; `timeout_secs`: how long this entry waits; A bad entry is rejected, never fatal (ADR 0135)); Layer 1: `static_credential`; Layer 2: `script_handshake` (Script contract; Worked example (Comfort Cloud, `password` credential); Worked example (Firebase via Google OAuth)); Layer 3: `hmac_signed` (built-in) |
| `system-knowhow/building-an-auth-handshake/layer-4-wasm-signer` | Layer 4: `wasm_signer` (How the engine loads modules; Manifest sidecar; WASM ABI; Host imports; Prefer pure-Rust crypto/encoding inside the signer; Authoring a signer; Build + install; Worked example: Binance HMAC); Composing layers; 401 retry; Pitfalls; Testing locally; Diagnostic evidence |
