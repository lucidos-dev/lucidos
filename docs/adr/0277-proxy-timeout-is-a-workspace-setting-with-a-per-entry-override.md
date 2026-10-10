# 0277: The proxy timeout is a workspace setting, and an apis.json entry may override it

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

The engine proxy cut every upstream request at a fixed 30 seconds. The limit
was reqwest's total timeout on the proxy client, so it covered the whole body.
The proxy buffers a streamed reply before it answers, so a streamed call was cut
the same way. `lucidos proxy` added a second 30-second cut of its own, from the
blocking reqwest client's implicit default.

A long model call through the proxy never came back. At about 117 output
tokens a second, anything over roughly 3,000 tokens ran past 30 seconds. The
routes those calls take are the builtin model providers (`vertex`, `openai`,
and the rest), which have no `apis.json` entry.

## Decision

The wait is configurable in two layers, each accepting 1 to 600 seconds:

1. `timeout_secs` on an `apis.json` entry, for that entry only.
2. `proxy_timeout_secs`, a workspace preference in the preference catalog.

An entry's value wins, then the workspace value, then 30 seconds. The engine
applies the result per upstream request, and caps one whole proxied call at 600
seconds. The CLI and the SDK wait 660 seconds, so no client deadline cuts a
call before the engine does.

## Rationale

The workspace preference has to exist because of the builtins. A per-entry field
reaches a builtin route only through an `apis.json` entry that overrides it. That
entry replaces the builtin, and with it the engine-held credential the builtin
exists to supply.

The per-entry field exists so one slow backend can wait longer without raising
the limit for every other one. A hung local device should still fail at 30
seconds while a model route waits five minutes.

Each layer validates the way its store already does. The preference catalog
refuses an out-of-range write with a message naming the key and the range. An
`apis.json` entry with a bad field is rejected by name at load, like any
malformed entry. The rest of the file keeps working.

The maximum matches the ten-minute default request timeout of the Anthropic and
OpenAI SDKs. It covers about 70,000 output tokens at the measured rate.

## Consequences

- A proxied call can hold an engine task and an upstream socket for up to ten
  minutes. That was already true of a slow upstream within 30 seconds, only
  shorter.
- A redirect hop and the one 401 retry each get the whole wait, as they did
  with the fixed 30 seconds. So one call can make up to ten upstream requests,
  and without a cap it could outlast any fixed client deadline. The 600-second
  cap on the whole call closes that. At the default it never binds: ten
  requests at 30 seconds is 300.
- The two client deadlines are literals that must track the engine maximum. A
  test reads both sources and fails when either drifts.
- The proxy still buffers a streamed body. Raising the limit lets a long
  streamed call finish; it does not make the proxy stream.

## Alternatives considered

- **The per-entry field alone.** It misses the builtin routes, which is where
  long model calls go. Rejected.
- **The workspace preference alone.** The first plan. It covers every route, but
  raising it for a model call also raises it for every small backend. The user
  chose the two-layer variant.
- **One budget for the whole call, with no per-request wait.** Simpler, but it
  changes the default: a redirect chain that fits in 30 seconds per hop today
  would be cut at 30 seconds in total. The cap keeps per-request semantics and
  binds only above the default.
- **No maximum.** A typo such as `30000` would park a request for eight hours.
  Rejected.
- **Streaming passthrough instead of a longer timeout.** It would take the body
  out of the total timeout. It is a larger change to the forward path, and it
  does not help an unstreamed call. Out of scope.
