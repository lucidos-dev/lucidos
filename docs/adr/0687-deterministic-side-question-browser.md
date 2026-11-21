# 0687: Use deterministic side-question browser coverage for card integration

- **Status**: Accepted. Amends ADR 0388 for side-question card coverage.
- **Date**: 2026-11-14

## Context

ADR 0388 kept the side-question browser spec local because its answer required
session recall. The spec remains selected by removed refusal strings in the
Codex side-question change. Its real-provider tag prevents the required GitHub
WebKit run (ADR 0658).

## Decision

Request a distinct explicit answer through the existing fake CLI in the
side-question card scenario. Keep the real endpoint, subprocess and event path,
and remove the real-provider tag from that scenario.

Tag tests that require provider recall or tool behaviour as before. A side-question
card integration test that needs only an explicit answer may run against the fake.

## Rationale

The existing fake already answers `Say exactly: "X"`. No new simulation is needed
to verify card rendering, folding, persistence and conversation isolation.
Those assertions cover browser and engine integration on all supported browser
runners without credentials. Provider session isolation requires separate runtime
coverage and real CLI probes.

## Consequences

- The routing fixture expects three tagged files locally and nine against the fake.
- Desktop, mobile Chromium and mobile WebKit can run the card scenario remotely.
- The deterministic answer does not certify provider memory or reasoning.
- ADR 0388's historical four-file split and rejection of all fake side questions
  no longer describe this card scenario. Other real-provider scenarios remain tagged.

## Alternatives considered

- **Permit local real-provider WebKit.** The user chose deterministic integration
  coverage instead, preserving remote WebKit and avoiding credential requirements.
- **Simulate session recall in the fake.** That would test our model of provider
  behaviour. The explicit answer avoids that simulation.
- **Drop the selected spec from retry evidence.** It remains selected and must pass.
