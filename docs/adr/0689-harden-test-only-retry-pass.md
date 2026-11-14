# 0689: Passing harden retries finish when only standalone tests changed

- **Status**: Accepted
- **Date**: 2026-11-14
- **Amends**: [0536](0536-fast-test-loop.md)

## Context

ADR 0536 required a full round after successful retries even when there were
no new commits and failures came from disk pressure or host load. On 14 November
the maintainer approved avoiding that extra round when only tests changed.

## Decision

Passing retries complete hardening if every commit since the last full round
started changes only standalone test/spec files. No commits also qualifies.
Inspect each commit, including merge parents, rather than the net diff.
Persist the original full-round baseline through repeated retries and print
the judged range and classification in the verdict evidence.

Use a narrow filename allowlist with exclusions for fixture/helper directories
and tracked consumers referencing a test filename or its extensionless name.
Fixtures, shared imported helpers, product source, build/tooling configuration,
unknown paths and unverifiable history require RERUN followed by the full set.
Inline tests in product files remain product changes.

## Rationale

The earlier full round already covered unchanged product behavior. Retrying
the failures supplies the missing passing evidence after test-only fixes or
environment recovery. A source change later reverted must still count, so
checking only the final tree would be insufficient.

## Consequences

The baseline outlives individual retry rounds. Old retry records without a
baseline conservatively owe a full round. Range and classification evidence
remains in the round state as well as the printed verdict.
The classifier deliberately rejects uncertain cases, including some standalone
tests mentioned by other tracked files. This costs an extra run rather than
accepting a helper as a test.

Carried WebKit verdicts (ADR 0616), generated-drift and branch-path selection
checks, fast tier and isolated Codex driver tests keep their existing behavior.

## Alternatives considered

- Always run the full set after retries: wastes coverage already obtained when
  only test code or the host environment changed.
- Classify directories named tests as test-only: accepts fixtures and helpers
  consumed by suites that the retry did not run.
- Inspect the net diff only: hides intervening product edits and reverts.
