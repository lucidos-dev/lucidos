---
paths:
  - "crates/lucidos-app/e2e/**"
  - "crates/lucidos-app/src/**/*.rs"
  - "crates/lucidos-app/src/**/*.test.ts"
  - "crates/lucidos-engine/tests/**"
  - "crates/lucidos-e2e/tests/**"
  - "crates/lucidos-app/src/generated/**"
  - "scripts/e2e*.sh"
  - "scripts/lib/e2e*.sh"
---

# E2E Testing

Six test suites. The API and browser suites run against the e2e workspace (`~/workspaces/e2e-test`). The wasm and embedder suites are pure Rust integration tests that need external setup (built WASM artifacts, a downloaded ML model). The packaged-build smoke test boots the macOS `.app`. Contract tests run inline as part of `cargo test`. Rationale for the harness: `docs/e2e-test-decisions.md`.

**The e2e engine builds and runs in RELEASE by default** (`scripts/lib/e2e.sh` sets `RELEASE=1`; `docs/plans/2026-06-28-e2e-always-release-build.md`). A local *filtered run* takes the debug engine and skips mobile-webkit, unless `--webkit` or a Playwright `--project` asks for it (ADR 0536). `LUCIDOS_E2E_DEBUG=1` also picks the debug build. A test-only seam production must not expose (`POST /api/v1/internal/seed-change-for-test`) is gated on `cfg!(any(debug_assertions, feature = "e2e-test-hooks"))`. The release e2e build passes `--features e2e-test-hooks`. A plain `cargo build --release` does not.

**The e2e workspace database is rebuilt from zero on every run.** `reset_e2e_database` (`scripts/lib/e2e.sh`) drops and recreates it, so the engine's next boot runs the whole migration chain, seeds included. A test may assert on seeded data, such as the builtin model registry.

- **The reset owns the engine lifecycle.** Call `reset_e2e_database` **instead of** `ensure_workspace_running`, never before it. Migrations and pgvector setup run only at boot.
- **The reset also starts the workspace TREE committed.** `settle_e2e_workspace_tree` commits whatever an earlier run left behind, since every apply refuses a dirty tree. It never discards.
- **Seeded registry rows are shared state within a run.** A test that mutates one restores it.

`--no-reset` reuses the running workspace (`docs/e2e-test-decisions.md` § "The e2e database is rebuilt from zero, never truncated").

## A whole suite runs on GitHub, a filtered run here

Every e2e script sends a whole suite to GitHub's runners when it can (ADRs 0382, 0386). A *filtered run* stays here: `-f`, or after `--` a Playwright `--grep`, `-g`, `--grep-invert` or a positional argument (ADR 0536). The report and exit code match a local run. The driver is `scripts/e2e-github.sh`, the runner half `scripts/e2e-github-shard.sh`, the workflow `.github/workflows/e2e.yml`.

- **A run stays local** (`e2e_github_handoff` decides, once, for all five scripts) for:
  - A filter, unless `--github` is given.
  - `--local`, or a local-only flag (`--no-reset`, `--headed`, `--ios`, `--packaged`).
  - A local-only variable: `THREAD_SHOTS` and the other screenshot switches, or `LUCIDOS_E2E_DEBUG`.
  - An unmet precondition: a dirty tree, no release libraries, no `lucidos` remote, or no `gh`. The fallback prints its reason.
- **`--github` insists.** It sends a filtered run too, and fails instead of falling back.
- **A local run takes an *e2e slot*** (`acquire_e2e_slots`, `scripts/lib/e2e_lock.sh`). A Chromium-only run takes the first free of `e2e-test` and `e2e-test-2`. A WebKit run or the umbrella takes both.
- **A GitHub run reuses a built engine** whose *engine input manifest* (`scripts/lib/engine_manifest.sh`) matches the tree. A miss builds under the `e2e` cargo profile.
- **`LUCIDOS_E2E_LOCAL=1` keeps nested scripts local**, so a caller inside a run never pushes a second run.
- **What runs where.** Linux runs API, WASM, embedder, `chromium` and `mobile`. macOS runs `mobile-webkit`. A spec with a `@real-claude-code` test runs on this host as the local leg. Every other spec meets the *fake Claude Code*.
- **Tag a test `@real-claude-code` when it needs real model behaviour**: resume memory, interrupts, mid-turn messages, side questions, tool use. An untagged test gets only `Say exactly: "X"` answered and the fake's `no rule` reply otherwise (ADR 0388).
- **What gets pushed.** HEAD, or the commit `--commit <sha>` names, stripped and scanned like a release, as a parentless commit on an `e2e/<run-id>` branch of the mirror. A named commit runs no local leg.
- **The main check is the one engine-driven run.** `e2e-github.sh main-check --commit <sha>` adds the engine suite and has no local fallback. An unmet precondition is no verdict: **exit 76** (ADR 0464).
- **No secrets in the workflow, ever.** `./scripts/check-e2e-workflow.sh` enforces its limits.
- **Harness changes stay runner-safe.** A shard reaches Postgres only through `shared_pg_psql` and `LUCIDOS_EXTERNAL_PG_PORT`, since macOS runners have no Docker. Shell must work under GNU and BSD tools.

## Browser E2E (Playwright)

Tests in `crates/lucidos-app/e2e/`: chat, streaming, cancellation, CC sessions, changes UI, threads, reload resilience.

```bash
./scripts/e2e-browser.sh                        # Run all
./scripts/e2e-browser.sh -h -f chat.spec.ts     # Headed, single file
./scripts/e2e-browser.sh -- --grep "cancel"     # Filter by name
./scripts/e2e-browser.sh --ios                  # iOS Simulator with Safari (requires Xcode)
```

Three projects: `chromium` (1280x800), `mobile` (375x812, mobile Chromium) and `mobile-webkit` (390x844, iOS Safari emulation).

**One browser by default.** A spec with no suffix (or `-desktop`) runs on `chromium` only. `-mobile` runs on the two phone projects, `-all` on all three. Name a spec `-all` when it also asserts a phone or WebKit path (`e2e/project-rules.test.ts`, ADR 0531).

Serial, 2 min timeout, traces on failure. **Calm motion by default:** every project emulates reduced motion. A spec that tests motion opts back in with `test.use({ reducedMotion: 'no-preference' })` (`e2e/motion-stance.test.ts`). Helpers: `e2e/helpers.ts` (`sendMessage()`, `waitForResponse()`, `switchToClaudeMode()`), `e2e/db-helpers.ts` (`psql()`, `git()`).

**When to write:** UI bugs, interaction flows, state transitions, streaming, layout behavior.

## API E2E (Rust)

Tests in `crates/lucidos-e2e/tests/api_support/` (member crate `lucidos-e2e`): HTTP contracts, SSE, errors.

```bash
./scripts/e2e-api.sh                    # Run all
./scripts/e2e-api.sh -f health          # Filter
```

**When to write:** new endpoints, changed responses, error handling, SSE.

**A run must leave the workspace tree as clean as it found it.** `e2e-api.sh` fails a run that leaves an uncommitted change, even after a failed test run, and names the paths. A test that needs a fixture under `data/` uses `write_data_fixture` and `remove_data_fixtures` (`api_support/mod.rs`), which commit in the same step.

`e2e-api.sh` also runs the **gateway chain test** (`crates/lucidos-gateway/src/chain_tests.rs`), the only local test that puts a real gateway router in front of a real engine. It is `#[ignore]`d because `make test` has no engine, and the script runs it BY NAME.

The gateway BINARY refuses to boot from a coding-agent worktree, with no opt-out (ADR 0021). Do not work around that to write a gateway test.

**GitHub browser shards do run the binary** (`LUCIDOS_E2E_GATEWAY=own`, ADR 0385). In every mode the e2e engine's `LUCIDOS_GATEWAY_PORT` and `LUCIDOS_WORKSPACE_ID` come from a gateway that lists it at the engine's port, never from the caller.

## Contract Tests (Rust ↔ TypeScript)

Rust is the source of truth. TS is generated, so never hand-edit `src/generated/`.

`./scripts/codegen.sh` writes every generated file: the Rust-rendered TS, CSS and JSON (`lucidos_engine::codegen::render_all`), then the SDK bundles in `packages/lucidos-sdk/dist/`. `npm test`, `npm run typecheck`, `npm run build` and every Vite run call it first. A bare `npx tsc --noEmit` does not, so after a Rust change run `npm run typecheck`. `make test` renders every file in memory.

**Regenerate** after a change to `available_thread_actions()`, `display_section()`, the `ThreadEvent` enum or any payload type it reaches, `engine/title_match.rs`, or any other source a renderer reads.

### ThreadEvent payload types (generated, ADR 0166)

The `syn` reader in `crates/lucidos-engine/src/engine/thread_events/ts_codegen.rs` generates the payload shapes into `src/generated/thread-event-wire.ts`. `store/thread-events/thread-event-types.ts` re-exports them and keeps only what the wire does not decide (labels, summary helpers, fingerprints).

It emits the **wire** shape: `variant + EventMeta fields + API stamps - API strips`. `#[serde(default…)]` or `skip_serializing_if` makes a property optional. `Option<T>` is `T`, never `T | null`.

**When you add a `ThreadEvent` variant or field:** regenerate. Nothing to hand-write. Two things fail loudly:

- **A new supporting type** fails the generator until it is in `TYPE_SOURCES`.
- **A new persisted variant** fails `all_persisted_event_types_matches_the_enum` until it is in that list in `thread_lifecycle.rs`.

A Rust doc comment's FIRST PARAGRAPH is carried into the generated file as TSDoc. Anything the wire carries but Rust does not is a row in a declared table in the generator, never a hand edit. That covers a retired variant, a legacy field, and an API stamp or strip. `thread-event-union.test.ts` keeps the two generated files in agreement. `wire-types-have-one-spelling.test.ts` fails on a second declaration of a generated type: re-export, never re-declare.

## WASM Signer E2E (Rust)

Tests in `crates/lucidos-e2e/tests/wasm_signers.rs` run real `.wasm` artifacts from `./signers/build-all.sh` through `WasmSignerLayer::apply`. The artifacts are gitignored, and the script builds them first.

```bash
./scripts/e2e-wasm.sh                   # Build signers + run all
```

**When to write:** a new signer, a change to the signer layer's host imports (`__wasm_test_internals`), or anything that could break the manifest, load and sign pipeline. The wat-based tests stay in `crates/lucidos-engine/tests/proxy_wasm_engine.rs`.

## Real-Embedder Tests (Cargo Feature Gate)

Tests of the real fastembed model (MultilingualE5Small) live behind the `real-embedder-tests` feature of `lucidos-engine`. They download about 465 MB on first run.

```bash
./scripts/e2e-embedder.sh               # Run only the gated tests
cargo test -p lucidos-engine --features real-embedder-tests   # All lib tests + gated
```

- **`GATED_TESTS`** at the top of `e2e-embedder.sh` is the list of gated test names, and the source of truth for their count. Add a new `#[cfg(feature = "real-embedder-tests")]` test there. The script's drift check fails if you forget.
- **A transient huggingface.co outage skips, never fails.** `test_util::shared_embedder()` returns `None` on a model-fetch error (`is_model_fetch_failure`), and a gated test does `let Some(provider) = shared_embedder() else { return };`. Assertion failures and non-network init errors still fail.
- **The cache is warm and stable.** The script pins `FASTEMBED_CACHE_DIR` to `${XDG_CACHE_HOME:-$HOME/.cache}/lucidos/fastembed`. Proof of the offline path: re-run with `HF_ENDPOINT=http://127.0.0.1:1`.
- **Other tests use the `KeywordEmbedder` mock** from `crate::test_util`. Gate a test only when it depends on the model's semantic behavior.

## Packaged Build Smoke Test (macOS)

`scripts/e2e-packaged.sh` boots the **packaged** macOS build and asserts the chain dev e2e never touches: staged Resources, the bundled gateway and engine, embedded Postgres, a per-workspace database, the engine spawn and static serving through the gateway.

```bash
./scripts/e2e-packaged.sh            # reuse an existing .app, else build it
./scripts/e2e-packaged.sh --rebuild  # force a fresh build-dmg.sh build first
./scripts/e2e.sh --packaged          # run it as a final phase of the full suite
```

It runs the bundle's headless service role (`Lucidos --service`) under a temp `HOME`, and asserts over HTTP and on disk. It does **not** drive the WKWebView UI (ADR 0016). The Tauri layer's non-UI logic is unit-tested in `crates/lucidos-app` (`cargo test -p lucidos-app`, which needs a built `crates/lucidos-app/dist/`).

**macOS-only** and **heavy**: standalone and NOT in the default `e2e.sh` run. The nightly opts in with `--packaged` or `LUCIDOS_E2E_PACKAGED=1`.

**When to write:** changes to the packaged boot chain: `build-dmg.sh` resource staging, `desktop.rs` service and gateway wiring, embedded Postgres provisioning, or the gateway's boot and control surface.

## The Codex driver tests fail under load

`runtime::codex::driver_tests` and `runtime::codex_app_server::driver_tests` spawn a `/bin/sh` stub and carry 30 s and 60 s event timeouts. Under host load they fail, and the failures look like real breakage in code the branch never touched (ADR 0292).

- **`/harden` runs these two modules alone, after the Codex review is joined** (`scripts/harden-suites.sh`).
- **Never accept a driver-test failure as a finding** until you re-run the module alone on a quiet host: `./scripts/test-engine.sh -- -- runtime::codex::driver_tests runtime::codex_app_server::driver_tests`.

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

**That last row is the one nothing else runs for you.** `make test` never runs `lucidos-app`, and clippy only compiles its test targets. Touch a `.rs` file under `crates/lucidos-app/src/` and run `cargo test --locked -p lucidos-app --lib` yourself (seconds, no Postgres). `/harden` Phase 4.5 carries the same row.

`./scripts/e2e.sh` runs API, browser, wasm and embedder back to back. Nothing else runs the wasm and embedder suites automatically, so never bypass it in trigger pipelines. The packaged smoke test is opt-in (`--packaged` / `LUCIDOS_E2E_PACKAGED=1`).
