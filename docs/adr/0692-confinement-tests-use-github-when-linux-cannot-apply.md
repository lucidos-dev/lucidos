# 0692: Confinement tests use GitHub when the Linux sandbox cannot apply

- **Status**: Accepted
- **Date**: 2026-11-14
- Amends [ADR 0690](0690-confinement-tests-run-outside-the-agent-sandbox.md), decision 8 and its GitHub runner rejection.

## Context

ADR 0690 requires real confinement tests and fails when the sandbox cannot apply.
A Linux host without usable Landlock therefore cannot harden sandbox changes.
The sandbox tests already report this through their real backend probe.

## Decision

Only that unavailable Linux probe routes confinement tests to e2e GitHub mode.
The driver runs a separate `ubuntu-latest` job and reports its verdict.
`/harden` pins the run to its start commit with `--commit`.
Local test failures never route to GitHub.

An unavailable GitHub route or a missing verdict exits 75 with the reason.
Only passing real confinement tests return 0. There is no skip.
The runner uses `--here`, so an unavailable runner sandbox fails without recursion.
The macOS owner permission route stays unchanged.

## Rationale

The GitHub rejection in ADR 0690 conflated runner execution with a per-change gate.
`/harden` drives this run, waits for it and checks its verdict.
It remains the gate, as with the touched WebKit leg (ADR 0616).
The existing driver supplies the stripped, scanned, parentless mirror push.
No new credential or push path is introduced.

## Consequences

Linux hosts without usable Landlock can harden when e2e GitHub mode is available.
Hosts without that mode fail loudly with 75 and its unavailable reason.
This includes contributors without mirror access; no account fallback is added.
Hosts with usable Landlock still test locally.
This adds no cross-OS coverage and changes no macOS permission behavior.

## Alternatives considered

- **Keep failing locally.** Rejected: it leaves supported Linux hosts unable to harden sandbox changes.
- **Skip with owner approval.** Rejected: nothing would prove confinement.
- **Run both operating systems on every change.** Rejected: cross-OS coverage exceeds this missing-host route.
- **Push directly from the confinement script.** Rejected: e2e GitHub mode already owns sanitization, credentials and verdict reporting.
