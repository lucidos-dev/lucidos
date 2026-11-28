# 0782: An OAuth credential carries its own endpoints so the engine holds no provider table

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0032](0032-a-state-write-owns-its-announcement.md), [0068](0068-oauth-callback-port-has-one-owner.md), [0128](0128-oauth-authorization-always-leaves-the-app.md)

## Context

`crates/lucidos-core/src/core/oauth.rs` runs OAuth client credential flows for
many providers. The engine must not need to know which provider it talks to.

This record covers where provider facts live, how the client proves itself, and
who opens the browser. The callback port owner and the announced account writes
are recorded in ADR 0068 and ADR 0032.

This record restates the design comments of
`crates/lucidos-core/src/core/oauth.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- The engine ships no hardcoded endpoint table. Known providers, endpoints and
  typical scopes live in `system-knowhow/oauth-providers.md`, which the agent
  reads and maintains.
- Endpoints the agent passes are persisted into the per-credential JSON.
  `prepare_oauth_flow` and `refresh_oauth_if_needed` both read them back from
  that JSON.
- An *OAuth provider registry* row only prefills a credential at write time, so
  the modal asks just for the Client ID. It never takes part in a flow.
- A registry prefill carries no scopes. Each connection names its own scopes
  when it starts.
- Client authentication at code redemption is derived solely from whether the
  credential carries a `client_secret`. With a secret the client is
  confidential. Without one it is public and uses PKCE.
- Extra authorization-URL parameters, such as the spelling of "ask for a
  refresh token", are credential data. They are documented per provider in
  `system-knowhow/oauth-providers.md`. The flow sends whatever the credential
  stores.
- The engine does not open browsers. The caller supplies an `open_auth_url`
  opener, and it runs only after `prepare_oauth_flow` is already waiting for
  the callback.
- The userinfo method is read from the credential's optional `userinfo_method`
  key. GET is the default, and POST is for providers that serve userinfo as
  POST only.
- Both scope sets from a token exchange are reported in a named struct and
  stored. Their difference is the shortfall a later Reconnect re-requests.

## Rationale

- A credential that fully describes its own authorization means one source of
  truth for both preparing and refreshing a flow.
- A secret-less desktop app is a public client under RFC 8252. Providers reject
  a secret from a public client and reject a secret-less redemption from a
  confidential one, so the two shapes must not be mixed. Deriving the shape
  from the secret uses OAuth's own distinction.
- A scope set belongs to what a connection is for, not to the provider. So the
  registry cannot hold scopes.
- A provider that never sees its own spelling of the refresh-token request
  issues no refresh token. So that spelling has to be stored as data per
  registration.
- The client knows the platform and the user's in-app-browser preference, so it
  decides where a URL is displayed. Opening only after the wait starts means
  the callback can never arrive before something is waiting.
- An explicit userinfo method keeps credentials without the key working. A
  silent method fallback would hide a genuinely broken endpoint behind a second
  request.
- A named struct is used for the exchange result. The two scope sets are the point of reporting an authorization, and position cannot tell them apart.

## Consequences

- Adding a provider means adding documentation and credential data, not engine
  code.
- A credential keeps fully describing its authorization after the registry row
  that seeded it changes.
- Callers of the full flow must pass an opener.
- The one place the module branches on a provider is `provider_for_url`. It is
  a deliberate exception and is not extended.
- CLAUDE.md forbids provider-specific instructions in code, and this module
  follows that rule.

## Alternatives considered

- A hardcoded endpoint table in the engine, rejected in favor of
  agent-maintained knowhow.
- Registry rows that take part in a flow or carry scopes, rejected.
- Choosing client authentication by provider name, rejected in favor of the
  presence of `client_secret`.
- Sending a secret from a public client, or a secret-less redemption from a
  confidential one, rejected because providers refuse both.
- Having the engine open the browser, rejected in favor of a caller-supplied
  opener.
- Sniffing the userinfo method from a 400 followed by a retry, rejected because
  it hides a broken endpoint.
