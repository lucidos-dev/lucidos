# 0508: The Claude gateway is its own provider kind, with its own token table and refresh path

- **Status**: Accepted
- **Date**: 2026-10-17

## Context

An organisation can run a *Claude gateway*: Anthropic's Messages API behind its
own sign-in, an OAuth device-code flow (RFC 8628) open to a public client.
Claude Code signs in to one when its managed settings force
`forceLoginMethod: gateway`, so coding-agent threads already used it. The
Lucidos Agent could not. Its Anthropic provider sent to the fixed
`ANTHROPIC_API_BASE_URL` with an API key or a subscription token.

Plan: `docs/plans/2026-10-17-claude-gateway-provider.md`.

## Decision

- **The gateway is its own provider, `ProviderKind::ClaudeGateway`** (wire
  `claude-gateway`), not a base-URL override on `Anthropic`. Both share one
  Anthropic wire, which takes a host and a gateway access.
- **Its tokens live in their own table, `claude_gateway_accounts`**, one row
  that records its `issuer`. They never enter `oauth_accounts`.
- **It refreshes on its own path** (`llm/claude_gateway/tokens.rs`), not
  through `refresh_oauth_if_needed`.
- **Endpoints come from the gateway's discovery document**, and the engine
  refuses one whose `issuer` differs from the configured URL.
- **The served models come from the gateway's `/v1/models`.** A model it does
  not list is "not served" before any request.

## Rationale

**Each credential stays on its own host by construction.** Picture one provider
slot with a switchable base URL. A crossed setting would then send a direct
Anthropic key to the gateway, or a gateway bearer to `api.anthropic.com`. Two
kinds make that a type error instead. It mirrors how Vertex already serves
Claude models as a kind of its own.

**A separate table keeps the token away from every reader of
`oauth_accounts`.** Each of those hands a token on: script `OAUTH_*` variables,
the `http_request` tool, the proxy script layer, backups, the access-token
endpoint and Settings → Accounts. A filter at each would have to be remembered
by every future reader. A table no such reader touches needs no filter. Raw
SQL through `LUCIDOS_DATABASE_URL` still reads it, as it reads every stored
secret (ADR 0481). The table also gives the issuer a column, which the token
source checks before it sends.

**The shared refresh does not fit.** `refresh_oauth_if_needed` needs a stored
client with a `client_id` and a fixed `token_url`. The gateway is a public
client whose token endpoint comes from discovery, so it has neither.

**Lucidos signs in as itself.** The gateway's sessions are per machine and per
application, and every request names Lucidos in its `User-Agent`. The gateway
can therefore tell Lucidos from Claude Code.

## Consequences

- A sign-in the gateway rejects (`invalid_grant`) forgets the row and announces
  `ClaudeGatewaySignedOut` with reason `rejected`. Settings and the failed turn
  both say to sign in again. Nothing falls back to another provider.
- Changing `claude_gateway_url` signs out of the old gateway, whoever wrote it.
- A device sign-in in progress is in memory only. An engine restart means
  starting again.

What we deliberately do not do:

- **No reuse of Claude Code's gateway token.** Lucidos never reads another
  application's keychain entry.
- **No web search through the gateway.** The `anthropic-server-tool` backend
  stays with direct Anthropic, since the gateway does not promise server tools.
- **No app proxy access.** `api/proxy_builtin.rs` has no gateway entry, so an
  app cannot spend the gateway credential.
- **No environment variable.** The sign-in needs a person, so an env-only setup
  could never finish.
- **No change to coding-agent threads.** They follow Claude Code's managed
  login already.
- **No policy gate.** Whether an organisation allows Lucidos on its gateway is
  for its owners to decide, not a check in Lucidos.

## Alternatives considered

- **A base-URL override on the Anthropic provider.** The smallest change, and
  rejected for the credential crossing above.
- **Storing the tokens in `oauth_accounts` under provider `claude-gateway`.**
  The first plan. Rejected in phase 2, once the list of readers that hand a
  token on was counted.
- **Reusing Claude Code's sign-in.** It would skip a second sign-in, but it
  means reading another application's secret store, and the gateway's sessions
  are per application anyway.
