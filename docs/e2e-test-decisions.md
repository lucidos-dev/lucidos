# E2E Test Design Decisions

Decisions and tradeoffs made while building the end-to-end test suite.

## Architecture: Three Layers

1. **Browser E2E tests** (Playwright, `crates/lucidos-app/e2e/`) — drive a real browser against the running Lucidos UI. Three projects run by default: `chromium` (desktop), `mobile` (mobile Chromium), and `mobile-webkit` (iOS Safari emulation via WebKit).
2. **HTTP API tests** (Rust, `crates/lucidos-e2e/tests/api_support/`, workspace member crate `lucidos-e2e`) — hit the API directly without a browser

All layers require a running Lucidos workspace (`~/workspaces/e2e-test` by default).

## Key Decisions

### Dual layout handling
Lucidos renders desktop and mobile layouts simultaneously — every DOM element exists twice. All Playwright selectors use `.first()` and visibility checks via `getBoundingClientRect()` to target only the visible (desktop) element. The `openThreadDrawer()` helper uses `page.evaluate()` with rect checks rather than Playwright's `.isVisible()` to avoid false positives from the hidden mobile layout.

### Thread drawer is collapsed by default
At 1280x800 viewport, the thread drawer starts collapsed. Tests that need it must call `openThreadDrawer()` which clicks the toggle button if the drawer isn't already visible.

### Reload tests don't rely on auto-focus
After `page.reload()`, localStorage-based thread auto-focus is unreliable in headless Chromium (timing issues with SSE reconnection and thread data loading). Reload tests instead:
1. Record the thread ID before reload
2. After reload, open the thread drawer
3. Explicitly click the thread to re-focus it
4. Then verify messages/state persisted

This tests the important thing (data persistence) without depending on the auto-focus race condition.

### Self-signed TLS
Lucidos uses HTTPS even in dev (Vite TLS). Both Playwright (`ignoreHTTPSErrors: true`) and Rust tests (`danger_accept_invalid_certs`) accept self-signed certificates.

### Port discovery
Both test layers read the workspace ports from `<workspace>/.lucidos/ports`. The workspace path is configurable via `E2E_WORKSPACE` environment variable, defaulting to `~/workspaces/e2e-test`.

### Unknown API routes return SPA fallback
The engine proxies unknown `/api/v1/*` routes to Vite, which returns the SPA HTML fallback with status 200. The Rust error test verifies the response is not valid JSON (i.e., it's HTML) rather than asserting a specific HTTP status code.

### Unique message markers
Every test uses `uniqueMessage(prefix)` to generate collision-free messages with timestamps and random suffixes. This prevents test interference when running against a shared workspace with existing data.

### LLM-dependent tests
Several tests send messages to the LLM and assert on responses. These tests use generous timeouts (90s for response completion) and assert on structural properties (non-empty content, visible element) rather than exact text content, since LLM output is non-deterministic.

### SSE streaming test
The streaming test captures text at two points during response generation. It asserts both snapshots are truthy (content appeared) rather than asserting the second is longer than the first, which is flaky with fast models that complete before the 1.5s delay.

### Rust API test module structure
Rust's module system doesn't allow both `tests/api.rs` and `tests/api/v1/mod.rs`. The solution uses `#[path]` attributes in `tests/api.rs` (in the `lucidos-e2e` crate) to include submodules from a `tests/api_support/` directory.

### Separate `lucidos-e2e` crate
API tests live in their own workspace member crate, not in `lucidos-engine`'s `tests/`. This keeps `cargo test -p lucidos-engine` from compiling them (so it stays fast and infra-free) and removes the need for `#[ignore]` on tests that require a running workspace. Run via `./scripts/e2e-api.sh` or the umbrella `./scripts/e2e.sh`.

### Single-writer lock on the e2e workspace
Both `e2e-browser.sh` and `e2e-api.sh` acquire `~/workspaces/e2e-test/.lucidos/e2e.lock` (PID + `$LUCIDOS_THREAD_ID` + worktree path + start time) before starting the workspace. A second invocation while the lock is held exits 1 with a message naming the holder; stale locks (dead PID) are reclaimed automatically. The lock exists because two CC sessions running Playwright concurrently against the shared workspace race on browser processes — on 2026-04-19 a WebKit GPU child leaked to 28 GB and OOM-rebooted a 32 GB Mac. Lock logic in `scripts/lib/e2e_lock.sh`; covered by `tests/e2e_lock_test.sh` (run directly, no harness).

## Test Coverage

### Browser E2E (16 tests)
- **Chat** (3): send/receive, thread sidebar, response content
- **Threads** (2): create/switch, message loading
- **Pinning** (3): pin, persist after reload, unpin
- **Reload** (2): message persistence, input usability
- **Streaming** (2): progressive rendering, completion status
- **Empty states** (4): compose view, drawer, health, error handling

### HTTP API (16 tests)
- **Health** (2): status ok, field structure
- **Chat** (3): stream response, event ID, thread targeting
- **Threads** (4): list shape, creation, pin/unpin, messages
- **SSE** (2): connection, event delivery
- **Errors** (5): unknown route, malformed JSON, missing content-type, wrong method, nonexistent thread

## Running the Tests

```bash
# Start the e2e workspace
./scripts/web-dev.sh -w ~/workspaces/e2e-test -b

# Browser E2E tests
./scripts/e2e-browser.sh

# HTTP API tests (also boots the e2e workspace)
./scripts/e2e-api.sh

# Both back-to-back (what the nightly pipeline runs)
./scripts/e2e.sh

# With visible browser (debugging)
./scripts/e2e-browser.sh -h
```
