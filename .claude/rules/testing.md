---
globs:
  - "crates/cognos-app/e2e/**"
  - "crates/cognos-app/src/**/*.test.ts"
  - "crates/cognos-app/src/**/*.spec.ts"
  - "crates/cognos-engine/tests/**"
  - "crates/cognos-app/src/generated/**"
---

# E2E Testing

Three test suites. All run against the e2e workspace (`~/workspaces/e2e-test`).

## Browser E2E (Playwright)

Tests in `crates/cognos-app/e2e/`. Chat, streaming, cancellation, CC sessions, changes UI, threads, reload resilience.

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

Tests in `crates/cognos-engine/tests/api_e2e_support/`. HTTP contracts, SSE, errors.

```bash
./scripts/e2e-api.sh                    # Run all
./scripts/e2e-api.sh -f health          # Filter
```

**When to write:** New endpoints, changed responses, error handling, SSE.

## Contract Tests (Rust ↔ TypeScript)

Source of truth: `thread_lifecycle.rs`. TS is generated — never hand-edit.

Regenerate: `cargo test -p cognos-engine generate_typescript_file -- --ignored && cargo test -p cognos-engine generate_cross_validation_fixture_file -- --ignored`

Staleness checks run as part of `cargo test`.

**When to update:** Changes to `resolve_actions()`, `display_section()`, or their types.

## Test Level Selection

| Scenario | Test type |
|----------|-----------|
| UI bug / interaction flow | Browser e2e |
| API response / SSE | API e2e |
| Shared Rust logic changed | Contract test |
| Store/signal behavior | Vitest |
| Rust engine logic | `cargo test` |
