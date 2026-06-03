---
globs:
  - "crates/lucidos-app/e2e/**"
  - "crates/lucidos-app/src/**/*.test.ts"
  - "crates/lucidos-app/src/**/*.spec.ts"
  - "crates/lucidos-engine/tests/**"
  - "crates/lucidos-e2e/tests/**"
  - "crates/lucidos-app/src/generated/**"
---

# E2E Testing

Five test suites. The API + browser suites run against the e2e workspace
(`~/workspaces/e2e-test`); the wasm + embedder suites are pure Rust integration
tests that only need external setup (built WASM artifacts; downloaded ML model).
Contract tests run inline as part of `cargo test`.

## Browser E2E (Playwright)

Tests in `crates/lucidos-app/e2e/`. Chat, streaming, cancellation, CC sessions, changes UI, threads, reload resilience.

```bash
./scripts/e2e-browser.sh                        # Run all (chromium + mobile + iOS webkit)
./scripts/e2e-browser.sh -h -f chat.spec.ts     # Headed, single file
./scripts/e2e-browser.sh -- --grep "cancel"     # Filter by name
./scripts/e2e-browser.sh --ios                  # Launch iOS Simulator with Safari (requires Xcode)
```

Three Playwright projects run by default:

- `chromium` (1280x800) — desktop browser
- `mobile` (375x812) — mobile Chromium emulation
- `mobile-webkit` (390x844) — iOS Safari emulation (WebKit engine, iPhone UA, 3x scale)

Serial, 2min timeout, traces on failure.
Helpers: `e2e/helpers.ts` — `sendMessage()`, `waitForResponse()`, `switchToClaudeMode()`, etc.
DB helpers: `e2e/db-helpers.ts` — `psql()`, `git()`.

**When to write:** UI bugs, interaction flows, state transitions, streaming, layout behavior.

## API E2E (Rust)

Tests in `crates/lucidos-e2e/tests/api_support/` (workspace member crate `lucidos-e2e`). HTTP contracts, SSE, errors.

```bash
./scripts/e2e-api.sh                    # Run all
./scripts/e2e-api.sh -f health          # Filter
```

**When to write:** New endpoints, changed responses, error handling, SSE.

## Contract Tests (Rust ↔ TypeScript)

Source of truth: `thread_lifecycle.rs`. TS is generated — never hand-edit.

Regenerate: `cargo test -p lucidos-engine generate_typescript_file -- --ignored && cargo test -p lucidos-engine generate_cross_validation_fixture_file -- --ignored`

Staleness checks run as part of `cargo test`.

**When to update:** Changes to `available_thread_actions()`, `display_section()`, or their types.

## WASM Signer E2E (Rust)

Tests in `crates/lucidos-e2e/tests/wasm_signers.rs`. Exercise real `.wasm`
artifacts produced by `./signers/build-all.sh` (compiled from
`signers/binance-hmac/` and `signers/test-echo/` to `wasm32-unknown-unknown`)
through `WasmSignerLayer::apply`. The artifacts are gitignored; the script
builds them before running the tests.

```bash
./scripts/e2e-wasm.sh                   # Build signers + run all
```

**When to write:** New signer, change to the WASM signer layer host imports
(`__wasm_test_internals`), or any change that could break the
manifest → load → sign pipeline against a real artifact.

The wat-based tests of the same layer (`wasm_signer_layer_runs_echo_signer_end_to_end`,
the body-mode and capability tests) stay in
`crates/lucidos-engine/tests/proxy_wasm_engine.rs` — they construct WASM inline
via `wat::parse_str` and need no external setup.

## Real-Embedder Tests (Cargo Feature Gate)

Tests that exercise properties of the real fastembed model (MultilingualE5Small)
— cross-lingual similarity, Norwegian synonyms, semantic ranking, etc. — live
behind the `real-embedder-tests` Cargo feature in `lucidos-engine`. They
download ~465 MB from huggingface.co on first run and would otherwise flake on
slow networks or fail offline.

```bash
./scripts/e2e-embedder.sh               # Run only the gated tests
cargo test -p lucidos-engine --features real-embedder-tests   # All lib tests + gated
```

The `e2e-embedder.sh` script keeps a hand-maintained list of the gated test
names and passes them as substring filters so it runs ~8 tests instead of
~1933. When you add a new `#[cfg(feature = "real-embedder-tests")]` test, add
its name to `GATED_TESTS` at the top of the script.

For unit/wiring tests of code that *uses* the embedder (e.g. `discover_knowhow`,
`stamp_knowhow`), use the `KeywordEmbedder` mock from `crate::test_util` —
deterministic, network-free, cosine reflects keyword overlap. Default
`cargo test` stays offline; only add `#[cfg(feature = "real-embedder-tests")]`
when the test genuinely depends on the model's semantic behavior.

## Test Level Selection

| Scenario | Test type |
|----------|-----------|
| UI bug / interaction flow | Browser e2e |
| API response / SSE | API e2e |
| WASM signer artifact behavior | WASM signer e2e |
| Embedder semantic behavior | Real-embedder gated test |
| Shared Rust logic changed | Contract test |
| Store/signal behavior | Vitest |
| Rust engine logic | `cargo test` |

`./scripts/e2e.sh` runs API → browser → wasm → embedder back-to-back. The
nightly trigger calls it; nothing else runs the wasm + embedder suites
automatically, so don't bypass `./scripts/e2e.sh` in trigger pipelines.
