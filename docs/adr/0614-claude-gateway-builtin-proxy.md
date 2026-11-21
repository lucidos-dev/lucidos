# 0614: Claude gateway proxy shares issuer-bound sign-in and managed model policy

- **Status**: Accepted
- **Date**: 2026-11-02

## Context

Apps and scripts need the same Claude gateway access as chat.
ADR 0508 deliberately excluded app proxy access; this decision supersedes only that exclusion.

## Decision

Expose `claude-gateway` as a builtin proxy accepting Anthropic `POST /messages`.
Use the configured origin, issuer-bound account and chat's shared refresh lock.
Apply managed model policy and served-model resolution before forwarding.

## Rationale

The existing proxy dispatch already enforces credential scope, redirects, one
401 retry and response header forwarding for every caller. A per-dispatch
gateway layer keeps the rejected bearer local to its request.
The shared refresh lock prevents rotating-token races with chat.

## Consequences

Tokens stay inside the engine and never enter ordinary credentials or `apis.json`.
Requests identify Lucidos. A gateway model or quota refusal has no provider fallback.
Usage captures retain proxy ownership and carry gateway provider attribution for
managed telemetry. Explicit `apis.json` entries keep precedence.

This works across supported macOS and Linux installs with a configured gateway
and matching sign-in. No workspace app migration is included.

## Alternatives considered

- Exporting a bearer to apps would expose a reusable credential.
- Independent proxy refresh state would race chat when refresh tokens rotate.
- Forwarding arbitrary gateway paths would allow the sign-in to reach unrelated APIs.
- Falling back to direct Anthropic would cross provider policy and accounting boundaries.
