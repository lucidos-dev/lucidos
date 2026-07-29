---
paths:
  - "crates/lucidos-app/e2e/**"
  - "crates/lucidos-app/src/**/*.test.ts"
  - "crates/lucidos-app/src/**/*.spec.ts"
  - "crates/lucidos-engine/tests/**"
  - "crates/lucidos-e2e/tests/**"
  - "crates/lucidos-app/src/generated/**"
---

# E2E Testing

Six test suites. The API + browser suites run against the e2e workspace
(`~/workspaces/e2e-test`); the wasm + embedder suites are pure Rust integration
tests that only need external setup (built WASM artifacts; downloaded ML model);
the packaged-build smoke test boots the macOS `.app` itself. Contract tests run
inline as part of `cargo test`.

**The e2e engine builds + runs in RELEASE by default** (`scripts/lib/e2e.sh` sets
`RELEASE=1`; `docs/plans/2026-06-28-e2e-always-release-build.md`). The debug engine's
CPU cost was the dominant driver of the mobile-webkit WebContent cold-start
contention wedge — release eliminates that flake class and matches the
packaged/prod engine. For fast local single-spec iteration on the debug build, set
`LUCIDOS_E2E_DEBUG=1` (the opt-out is authoritative; an explicit `RELEASE=` is
otherwise honored). The release compile caps `CARGO_BUILD_JOBS` at half the cores
to avoid a host OOM during codegen. Test-only seams that production must not expose
(e.g. `POST /api/v1/internal/seed-change-for-test`) are gated on
`cfg!(any(debug_assertions, feature = "e2e-test-hooks"))` so they survive the
release e2e build — which passes `--features e2e-test-hooks` — while a plain
`cargo build --release` / `cargo tauri build` (no feature) still 404s them.

**The e2e workspace database is rebuilt from zero on every run.**
`reset_e2e_database` (`scripts/lib/e2e.sh`) drops and recreates it instead of
truncating, so the engine's next boot runs the whole sqlx migration chain against
an empty database — **migration seeds included**, which a truncate that spared
`_sqlx_migrations` silently skipped (that left `models` permanently empty in e2e).
So e2e tests may assert on seeded data, e.g. the builtin model registry. Two rules
follow:

- **The reset owns the engine lifecycle** — it stops the engine, recreates the
  database, and starts it again, because migrations / `EventStore::init_schema()`
  / the pgvector setup run only at boot. Call `reset_e2e_database` **instead of**
  `ensure_workspace_running`, never before it.
- **Registry-style seeded rows are shared state within a run.** A test that
  mutates one (a builtin's `context_window`, say) must restore it; the database is
  recreated per run, not per test.

`--no-reset` skips the reset entirely and reuses the running workspace. Rationale
and the rejected template-database alternative: `docs/e2e-test-decisions.md`
§ "The e2e database is rebuilt from zero, never truncated".

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

### ThreadEvent union coverage (TS-side drift guard)

`thread_lifecycle.rs` generates the `EVENT_CLASSIFICATION` map (event name → class)
into `src/generated/thread-lifecycle.ts`, but the **payload** shapes live in a
hand-maintained discriminated union (`ThreadEvent` in
`src/store/thread-events/thread-event-types.ts`) — that union is NOT generated
(its legacy-tolerant optional fields and frontend-only doc comments deliberately
diverge from the strict Rust types, so a serde→TS codegen would regress them).
Two guards keep the union from silently drifting behind the generated map:

- **Compile-time:** `THREAD_EVENT_TYPE_FLAGS` (`satisfies Record<ThreadEvent['type'], true>`)
  forces the runtime `THREAD_EVENT_TYPE_NAMES` set to match the union exactly —
  `tsc` fails if a variant is added/removed without updating the set.
- **Runtime contract test:** `src/generated/thread-event-union.test.ts` asserts
  every key in the generated `EVENT_CLASSIFICATION` has a matching union member.

**When you add a Rust `ThreadEvent` variant:** after regenerating
`thread-lifecycle.ts`, add the matching payload member to the `ThreadEvent` union
AND its key to `THREAD_EVENT_TYPE_FLAGS` (both in `thread-event-types.ts`), or the
contract test / `tsc` fails. The guard is one-way (`EVENT_CLASSIFICATION ⊆
union`); the union may legitimately carry extra members (retired legacy events,
the `CommandCheckpoint*` pair) that the classification map omits.

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
names and passes them as substring filters, so it runs **5** tests instead of
the whole lib suite. `GATED_TESTS` at the top of the script is the source of
truth for that number — don't restate the count elsewhere, read it from there.
When you add a new `#[cfg(feature = "real-embedder-tests")]` test, add its name
to `GATED_TESTS`; the script's own drift check (it re-extracts the gated test
names from the source and diffs them against the list) fails loudly if you
forget, so the count cannot silently go stale again.

**Network resilience (warm cache + graceful skip).** These tests must never red
the suite on a transient huggingface.co outage (a real failure mode — a
`tokenizer.json` fetch once timed out and failed the nightly e2e). Two layers:

- **Warm cache (fast/deterministic path).** `e2e-embedder.sh` pins
  `FASTEMBED_CACHE_DIR` to a stable, machine-persistent dir
  (`${XDG_CACHE_HOME:-$HOME/.cache}/lucidos/fastembed`) so the ~465 MB seed
  survives `cargo clean` / worktree churn and is shared across worktrees + the
  nightly checkout. On a cache hit, `hf-hub`'s `ApiRepo::get` short-circuits
  before any network call — seeded runs are fully offline. (The model is far too
  large to commit, so `.fastembed_cache/` stays gitignored; the cache is *seeded*
  once, not checked in.)
- **Graceful skip (resilience guarantee).** When the cache is cold *and*
  huggingface.co is unreachable, `test_util::shared_embedder()` returns `None`
  (logging a `SKIP` line) instead of panicking on the model-fetch `.unwrap()`.
  Each gated test does `let Some(provider) = shared_embedder() else { return };`,
  so an HF outage degrades to *skipped*, never *failed*. Only a model-fetch /
  network error skips (matched by `is_model_fetch_failure`) — assertion failures
  and non-network init errors (corrupt model, bad config) still fail loudly.

To prove the offline path locally: seed once (`./scripts/e2e-embedder.sh`), then
re-run with `HF_ENDPOINT=http://127.0.0.1:1` — the tests pass from the warm cache
with zero network. To prove the skip: point `FASTEMBED_CACHE_DIR` at an empty dir
with the same unreachable endpoint — the tests skip rather than fail.

For unit/wiring tests of code that *uses* the embedder (e.g. memory rebuild,
`recall_memory`), use the `KeywordEmbedder` mock from `crate::test_util` —
deterministic, network-free, cosine reflects keyword overlap. Default
`cargo test` stays offline; only add `#[cfg(feature = "real-embedder-tests")]`
when the test genuinely depends on the model's semantic behavior.

## Packaged Build Smoke Test (macOS)

`scripts/e2e-packaged.sh` — boots the **packaged** macOS build end-to-end and
asserts the chain that dev e2e never touches: staged Resources, the bundled
gateway + engine binaries, relocatable **embedded** Postgres provisioning, a
per-workspace database, the engine spawn, and static serving through the gateway
proxy.

```bash
./scripts/e2e-packaged.sh            # reuse an existing .app, else build it
./scripts/e2e-packaged.sh --rebuild  # force a fresh build-dmg.sh build first
./scripts/e2e.sh --packaged          # run it as a final phase of the full suite
```

It runs the bundle's **headless service role** (`Lucidos --service`) under an
isolated temp `HOME` + a free port + a seeded fastembed cache, then asserts over
HTTP + on disk: gateway health (`/~/api/v1/health`) → picker → create a workspace
→ poll it to `healthy` → engine health through the gateway (`/<slug>/api/v1/health`)
→ app shell base href → embedded Postgres on disk. Graceful SIGTERM teardown
verifies a clean stop (port freed, no `postmaster.pid`) and removes the temp `HOME`.

It does **not** drive the WKWebView UI: Apple's WKWebView exposes no WebDriver and
`tauri-driver` supports only Linux/Windows, so the packaged window can't be
automated on macOS (ADR 0016). The Tauri layer's non-UI logic is unit-tested in
`crates/lucidos-app` (`lib.rs` / `notifications.rs` / `desktop.rs`,
`cargo test -p lucidos-app` — needs a built `crates/lucidos-app/dist/` for
`generate_context!` to compile, so run a frontend build first).

**macOS-only** (skips gracefully elsewhere) and **heavy** (full release + DMG
build + a Postgres download) — so it is standalone and NOT in the default
`e2e.sh` run; the nightly opts in via `--packaged` / `LUCIDOS_E2E_PACKAGED=1`.
See `docs/e2e-test-decisions.md` for the rationale.

**When to write:** changes to the packaged boot chain — `build-dmg.sh` resource
staging, `crates/lucidos-app/src/desktop.rs` service/gateway wiring, the embedded
Postgres provisioning, or the gateway's boot/control surface.

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
| Packaged macOS build boots | Packaged build smoke test |
| Native Tauri (non-UI) logic | `cargo test -p lucidos-app` |

`./scripts/e2e.sh` runs API → browser → wasm → embedder back-to-back. The
nightly trigger calls it; nothing else runs the wasm + embedder suites
automatically, so don't bypass `./scripts/e2e.sh` in trigger pipelines. The
packaged smoke test is opt-in (`./scripts/e2e.sh --packaged` /
`LUCIDOS_E2E_PACKAGED=1`) because the full build is too heavy for every run.
