# 0730: ChatGPT plan search uses its own Responses wire as the last backend

- **Status**: Accepted
- **Date**: 2026-11-17
- **Amends**: [0636](0636-chatgpt-plan-provider.md)

## Context

Plan-only workspaces have no web search backend, so Lucidos Agent searches fail.
The Phase 0 probe in [the provider plan](../plans/2026-11-09-chatgpt-plan-provider.md) completed a hosted `web_search` request.
ADR 0636 left search out pending that evidence.

## Decision

Add `chatgpt-plan-responses` after Vertex, Anthropic and OpenAI key search.
It wraps only a plan `OpenAiProvider`, using its own Responses endpoint and authentication.
The plan request streams with storage disabled and omits platform-only fields.
Its terminal response supplies citations through the existing OpenAI search parser.

## Rationale

The existing wire owns served-model checks, token refresh, typed plan errors and the shared limit pause.
Reusing those boundaries keeps plan tokens away from platform search and app proxies.
Putting the plan last preserves existing backend choices.

## Consequences

Every signed-in, enabled plan workspace can search, subject to OpenAI's account policy.
Signing out or disabling the provider removes its search backend.
Plan search records `chatgpt-plan` billing and never falls through to another provider after a plan failure.
Claude Code and Codex retain their own search tools.

## Alternatives considered

- Reusing platform search would mix credentials and violate ADR 0636's endpoint boundary.
- Leaving search disabled would ignore the successful probe and leave plan-only workspaces unable to search.
- Putting the plan first would change established backend choices for users with other search providers.
