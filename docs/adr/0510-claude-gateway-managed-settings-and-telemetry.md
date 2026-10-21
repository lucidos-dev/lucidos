# 0510: The Claude gateway's managed settings and usage telemetry live in the engine, and follow the gateway's protocol over its reference plugin

- **Status**: Accepted
- **Date**: 2026-10-17

## Context

ADR 0508 made the *Claude gateway* a provider of its own. The gateway does more
than serve the Messages API. Its client contract, served at `GET {base}/protocol`,
also covers these:

- *gateway managed settings* at `GET {base}/managed/settings`;
- an `availableModels` allowlist inside them;
- a `surface` parameter that names the client at sign-in;
- OTLP log export to `POST {base}/v1/logs`.

An organisation turns on usage telemetry through those managed settings, with
Claude Code's own switches. A reference plugin for another client implements
the same contract, and differs from the protocol document in places.

Plan: `docs/plans/2026-10-17-claude-gateway-provider.md`, phase 7.

## Decision

- **All of it lives in the engine, not in an adapter outside Lucidos.** The
  managed settings, the allowlist and the exporter sit under
  `llm/claude_gateway/`.
- **Where the reference plugin and the protocol document differ, the protocol
  wins.** Lucidos tolerates the reference's shapes where that costs nothing.
- **The managed settings are kept on the sign-in's row**, refreshed hourly
  with `If-None-Match`, and dropped with the sign-in.
- **A non-empty `availableModels` replaces the gateway's model listing** as
  the served set. An empty or absent one leaves the listing in charge.
- **`enforceAvailableModels` governs only gateway routes.** No policy hides
  the user's other providers.
- **Telemetry is one OTLP log record per finished gateway model call**, built
  from the persisted `ContextCaptured` row by a bus consumer. It carries counts,
  cost and timing, the thread id, and the account's email and id when the token
  is a JWT. It never carries content.
- **The exporter's cursor moves past captures it will never send**: made
  before the current sign-in, or while telemetry was off. They are skipped,
  never sent later.

## Rationale

**Nothing here is company-specific.** The Claude apps gateway is Anthropic's
product, which any organisation can deploy. The telemetry switches and the
`claude_code.api_request` event are Claude Code's public monitoring contract,
and OTLP is an open standard. What does vary arrives at run time from the
gateway: its URL, its allowlist, and whether and where telemetry goes. No
organisation's values are written into Lucidos.

**An adapter would break ADR 0508.** Fetching the policy and exporting to the
gateway both need the bearer, and the bearer never leaves the engine. The
allowlist must also gate routing inside the engine, before any request.

**The protocol is the contract; the reference is one client of it.** Three
places differ:

- The protocol locks OTLP export to `{base}/v1/logs` with the bearer. Only the
  managed settings can name a collector on another host, which never gets the
  bearer. The reference posts to `OTEL_EXPORTER_OTLP_ENDPOINT` as given.
- The protocol says the body is the policy itself. The reference reads a
  `settings` wrapper. Lucidos reads the wrapper when present, else the body.
- The protocol tells `404` (no policy) from `200 {}` (an empty one). Lucidos
  does too.

**The exporter reads up to the committed event horizon, not a settle lag.**
A sequence can commit out of order. The plan proposed reading only rows older
than a fixed lag. ADR 0364's committed horizon already answers the question
exactly, so a slow commit is never skipped and a fast one never waits.

**A skipped capture stays skipped.** The policy decides telemetry at the time
of the call. Sending a backlog once a policy turns telemetry on would report
calls made while the organisation had asked for nothing. One sign-in's records
also never reach a later sign-in's gateway, which may belong to a different
organisation.

**The first policy is awaited, not assumed.** Right after a sign-in the policy
fetch is still in flight. The exporter waits for it rather than skipping
captures it cannot yet judge.

**An empty allowlist is no allowlist.** Read literally, an empty list serves
nothing, so one stray `[]` in a policy would stop every gateway call. The
gateway's own listing still bounds what is served.

**`enforceAvailableModels` stops at the gateway.** The workspace is the
user's, and ADR 0508 put no organisation policy gate in Lucidos. A policy may
shape what the organisation's gateway serves, never which other providers the
user may reach.

## Consequences

- `claude_gateway_accounts` gains `managed_settings`, `managed_settings_etag`,
  `managed_settings_fetched_at` and `managed_telemetry_since`. The last records
  when the policy turned telemetry on, so a pass that runs after the change
  still skips what was made before it. A change announces
  `ClaudeGatewayManagedSettingsChanged`.
- `ContextCaptured` gains `provider`, the backend that served the call. The
  exporter selects `claude-gateway` rows with a usage block, and
  `claude-gateway` `ModelCallFailed` rows (see below).
- `claude_gateway_telemetry_cursor` holds one sequence. A crash between the
  collector's `2xx` and the cursor write resends that batch once.
  `session.id` with `event.sequence` lets a collector drop the repeat. The
  sequence alone repeats across users, since each workspace has its own
  database.
- The engine's own environment can opt out with
  `CLAUDE_CODE_ENABLE_TELEMETRY=0` or `DISABLE_TELEMETRY=1`, as Claude Code
  can. An engine started from a shell that sets either sends nothing.
- An opted-out engine flags the cursor row (`opted_out`) at startup, before
  any model call, and on each pass. The next pass without the opt-out skips
  everything up to the horizon and clears the flag in the same `UPDATE`. So
  a capture made after the last opted-out pass, before a stop or crash, is
  never sent by a later run. That pass also skips what the new run made
  before it, which errs on the side of not sending.
- Settings and `lucidos claude-gateway status` say when the gateway collects
  usage telemetry.
- A record also carries what Claude Code's own record does when the call
  holds it: `identity.source=gateway-oidc` and `user.groups` from a JWT, the
  call's `effort`, `speed=normal`, the 5-minute and 1-hour cache-write split,
  and `os.type` and `host.arch` on the resource.
- `request_id`, `ttft_ms` and `prompt.id` follow the section below.
- Protobuf is hand-encoded over the closed OTLP logs subset, so no dependency
  was added.
- No other managed setting is applied. Coding-agent threads already get them
  from Claude Code.

## Failed calls, `request_id`, `ttft_ms` and `prompt.id`

Claude Code reports a failed call as `claude_code.api_error`. Lucidos now does
too, so the gateway sees the calls that failed as well as the ones that cost.

- **A failed call is a `ModelCallFailed` event, never a `ContextCaptured`.** A
  capture binds to a transcript step, and every usage rollup sums it. A failed
  call has no context to show and no spend. The event holds facts only: the
  model, provider, timing, attempts, a failure class, the status and the
  `request-id`. It holds no error text, because a provider's error body can
  echo request content.
- **`error` is Claude Code's class slug, never the message.** The exporter maps
  the stored facts:

  | Stored | `error` |
  |---|---|
  | status 429 | `rate_limit` |
  | status 529 | `server_overload` |
  | status 401 or 403 | `auth_error` |
  | status 404 | `model_not_found` |
  | status 413 | `request_too_large` |
  | dropped connection or broken stream | `connection_error` |
  | missed deadline | `api_timeout` |
  | anything else | `unknown` |

  Every slug matches `^[a-z0-9_]+$`, which Claude Code's class mode requires.
  `status_code` carries the detail.
- **`prompt.id` is the turn's origin event id.** A turn's row carries the
  `request_event_id` it was written under, and that wins even when a follow-up
  message queued behind the turn is newer. An auxiliary row has none, so the
  exporter reads the thread's newest `MessageReceived`, `TriggerStarted` or
  `ChildThreadCompleted` before the row. That matches Claude Code's "until the
  next prompt". A row with neither sends no `prompt.id`.
- **Absent stays absent.** No `request-id` header, no streamed token, no status
  or no turn means no attribute: never an empty string or a zero.
- **Dedupe is unchanged.** Each row has its own event sequence, so `session.id`
  with `event.sequence` and `event.name` names one record. The cursor, the
  opt-out skip and the sign-in gates treat both row types alike.

`client_request_id`, `api_retries_exhausted` and `api_refusal` are not sent.
A call the caller cancels records nothing.

## Alternatives considered

- **An adapter process outside the engine.** Lost: it would need the bearer,
  which ADR 0508 keeps inside, and it could not gate routing.
- **Following the reference plugin exactly.** Lost: it sends the bearer to
  whatever endpoint the environment names. The protocol keeps the bearer on
  the gateway origin.
- **A fixed settle lag before reading captures.** Lost: any lag is either too
  short for a slow commit or a needless wait for a fast one. The committed
  horizon is exact.
- **Sending captures from the provider call itself.** Lost: a slow or failed
  collector would then delay or fail a turn, and a restart would drop what was
  in flight.
- **Sending the backlog when telemetry turns on.** Lost: it reports calls made
  before the organisation asked, possibly to a different organisation.
- **Hiding other providers under `enforceAvailableModels`.** Lost: the
  workspace is the user's, not the organisation's.
- **An OpenTelemetry SDK crate.** Lost: one log record shape does not justify
  the dependency tree.
