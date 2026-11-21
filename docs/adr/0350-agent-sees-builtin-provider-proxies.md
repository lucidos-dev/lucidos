# 0350: The Lucidos Agent sees every builtin provider proxy, and request_credential refuses a key one already holds

- **Status**: Accepted
- **Date**: 2026-10-02

## Context

A user asked the agent to use an OpenAI model. The agent decided it had no
OpenAI access and opened a credential form for an OpenAI key. The engine already
served a working builtin `openai` proxy: `lucidos proxy openai /models` answered
200.

The agent could not have known. Its context listed stored credentials only, and
a builtin proxy is neither a credential row nor an `apis.json` entry. Worse, the
`proxy_request` tool read `apis.json` alone, so it could not reach a builtin even
when asked by name. Scripts and apps could; the agent could not.

## Decision

1. `proxy_request` resolves a name exactly as the HTTP proxy route does, through
   one shared resolver: `apis.json` first, the builtin only on its 404.
2. One catalog lists every builtin. The agent's context renders it on every
   turn, with each base URL, whether the base already has `/v1`, and whether the
   proxy is configured. "Configured" is the proxy's own resolver answering.
3. `request_credential` **refuses** when the service name maps to a configured
   builtin that injects a key. A keyless `local` server holds none, so a key
   for it may still be asked for. The match ignores case and separators, plus a small alias table
   (`gpt`, `claude`, `grok` and similar). Two overrides proceed: an auth type
   that is not a bare token, and a host outside the builtin's base.
4. Every model provider is a builtin or names its reason. `opencode-free` stays
   out, per ADR 0104.

## Rationale

**Refuse rather than warn.** A warning still opens the form, and the user then
types a key the engine already holds. That second copy is worse than useless:
it shadows the working one and is one more secret to rotate. The refusal costs
the agent one tool call and tells it the exact call to make instead.

**The overrides keep the refusal honest.** It only knows the builtin injects a
token at one host. An OAuth client registration, or a key for a different host,
is a different credential, so it proceeds.

**An unknown state never refuses.** When the resolver itself fails, nobody knows
whether the proxy works. Refusing then would block the only path the user has.

**One resolver, two callers.** The divergence was two code paths that each
decided where a name goes. Sharing the function makes the agent and
`lucidos proxy` agree by construction, and both still pass the scope gate of
ADR 0157.

## Consequences

- The block costs about 1,100 characters on every turn. It sits in the per-turn
  message, after the cached system block, so it does not disturb the prompt
  cache. Its order is fixed and it carries no timestamps.
- A new `ProviderKind` fails a test until it joins the catalog or the exclusion
  list.
- A user who really wants a second OpenAI key under the same name adds it in
  Settings, not through the agent.

## Alternatives considered

**Warn and still open the form.** Rejected for the reason above: the form is the
harm.

**List the builtins only when the turn classifier asks for credentials.** The
stored-credential list is gated that way. It was rejected for the builtins. The
failure happens exactly when the agent thinks no credential is involved, and
concludes it has no access.

**Write the builtins into `apis.json`.** It would reuse the existing listing.
Rejected: the builtin's key and base come from the engine's provider config, and
a copied entry would go stale the first time either changes. It would also
override the builtin forever, by the precedence rule.

**Give `opencode-free` a proxy too.** Offered and declined. ADR 0104's reason
still holds: nothing should build on an anonymous endpoint that can vanish
without notice.
