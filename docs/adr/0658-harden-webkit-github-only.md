# 0658: Hardening sends touched WebKit specs to GitHub on every host

- **Status**: Accepted
- **Date**: 2026-11-11
- **Supersedes**: [0616](0616-harden-webkit-local-and-carried-verdicts.md), decision A only

## Context

A local WebKit hardening run exhausted host memory. ADR 0616 moved macOS
WebKit execution locally to avoid GitHub queue times, but that exposes the
contributor's host to the browser and engine memory load.

## Decision

Dispatch hardening's touched WebKit specs to GitHub on every host, pinned to
the run's start commit, alongside the local non-WebKit suite. Preserve ADR
0616's independent passing-verdict carry and driver retry behavior.

## Rationale

GitHub isolates WebKit's memory load from the contributor's host. Removing
host routing and the local sequencing wait gives every host the same policy.

## Consequences

- This hardening leg requires configured GitHub access and pays its queue and build cost.
- Missing GitHub prerequisites fail explicitly; there is no local fallback.
- Local non-WebKit execution, carry records and manual `--webkit` behavior remain unchanged.

## Alternatives considered

- **Keep local macOS WebKit execution.** Rejected because it exhausted host memory.
- **Fallback locally when GitHub is unavailable.** Rejected because it restores the same memory risk.
- **Remove passing-verdict carry.** Rejected because valid results still avoid duplicate runs.
