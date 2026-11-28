# 0748: The main check runs frontend typecheck and Vitest on Linux after landing

- **Status**: Accepted
- **Date**: 2026-11-20
- **Amends**: [0694](0694-harden-round-in-minutes.md), [0464](0464-main-check-on-github.md)

## Context

Five window peers tests failed when Vitest ran two files together. Node's
BroadcastChannel delivered messages across workers that used the same name.
The main check ran browser specs and Rust and script tests, so it missed the
Vitest failure.

## Decision

The main check adds a Linux frontend job to the existing suites matrix. It runs
`npm run typecheck` and `npm test` from `crates/lucidos-app`. It runs both checks
even if one fails, and reports the shard log and its failing exit code.

## Rationale

The check sees interactions between changes after they land. One platform
suffices for the frontend's TypeScript and Vitest checks. The browser specs
already exercise the built frontend.

## Consequences

Ordinary e2e requests keep their existing suites. Covered hardening rounds
still run the selected TypeScript, Vitest and Vite checks before Apply.
The new job adds runner time to the main check.

## Alternatives considered

Moving the frontend checks out of covered hardening would delay feedback on
individual changes. Running the same checks on macOS would add a runner
without testing any additional platform-specific frontend code.
