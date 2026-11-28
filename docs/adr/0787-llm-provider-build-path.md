# 0787: One build path in `provider_build.rs` constructs the active provider and its search backends

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0023](0023-web-search-is-provider-native.md), [0508](0508-claude-gateway-provider.md)

## Context

The engine installs an active `Arc<dyn LlmProvider>` at startup and again when
credentials or preferences change at runtime.

The choice of which provider to install lives in `crate::llm::select_provider`,
which has a unit-tested matrix. `provider_build.rs` resolves that function's
boolean inputs from the database and env. It then maps the chosen
`ProviderSelection` onto a concrete provider.

A degraded boot has no database pool. Stored credentials and the
`local_base_url` preference cannot be read, but env fallbacks still apply.

This record restates the design comments of
`crates/lucidos-core/src/llm/provider_build.rs`, written before the comment
strip removes them. Nobody checked it line by line against the code.

## Decision

- One construction path builds the active provider. Startup in `main.rs` and
  `spawn_provider_config_subscriber` both use it.
- The build inputs are fixed at boot and reused verbatim by the runtime
  subscriber.
- The direct providers (OpenAI-wire and Anthropic) are resolved by one
  function. `pool == None` marks a degraded boot, and the same code serves both
  cases.
- Web-search backends are built in the same step as the credentials they need.
  They are returned beside the providers.
- A provider the user has switched off is vetoed by `switches`. Its search
  backend goes down with it.
- The Anthropic and OpenAI builders take already-resolved auth or keys. They do
  not resolve them.
- `build_openrouter_provider` and `build_xai_provider` share one body. It
  always sets `force_chat_completions`.
- `build_local_provider` does not use that shared body.
- The search chain puts Anthropic before OpenAI.
- The Claude gateway has no search backend.

## Rationale

- A hot-swap yields a provider byte-identical to a fresh boot, because both run
  the same code on the same inputs.
- `vertex_location`, `vertex_token_cache` and `model_registry` are shared
  handles. A rebuilt provider tracks live region and registry updates and
  reuses warm Vertex tokens.
- Built providers keep their auth private. Getters to hand a key back out would
  widen the surface a secret travels across for no benefit.
- Resolving auth twice could make the provider and its search backend disagree
  about which source won. Passing one resolved value avoids that.
- A provider that is off must not keep answering `web_search` on the key that
  is off with it.
- Neither OpenRouter nor xAI implements OpenAI's Responses API. A
  `gpt-5`-shaped id served by one must still take the Chat Completions path.
- Anthropic's server tool has no per-call fee. OpenAI's Responses web search
  bills per call on top of the tokens the results consume. The chain order is
  therefore a cost decision.
- The Claude gateway promises no server tools, so it serves no web search.
- The local provider resolves its own base URL, tolerates an empty key and logs
  that base. The shared hosted-backend body does not fit it.

## Consequences

- A reorder of the search chain cannot reach only one copy of it.
- On a degraded boot, every field degrades to `None` or an omitted backend on a
  build error. The engine still comes up on its other providers.
- A stored credential that cannot be read falls through to the env fallback
  with a log line. See `CredentialUnreadable`.
- The named OpenRouter and xAI wrappers stay. Each carries what is
  provider-specific: its env fallback, and why its headers are present or
  absent.
- Logs name the source of a credential, never the secret.
- A reqwest client that cannot be built degrades that provider to `None`
  instead of aborting.

## Alternatives considered

- Rebuild search backends from the built providers. Rejected because it would
  need getters that expose keys.
- Keep a separate degraded-boot copy of the seven builders, the search chain
  and the result literal. This was the earlier shape. A chain reorder could
  reach one copy only.
- Let each builder resolve its own auth or key. Rejected because the provider
  and its search backend could disagree about which source won.
- Route the local provider through the shared hosted-backend body. Rejected on
  purpose.
