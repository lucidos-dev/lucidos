# 0784: Inbound webhook senders are described as data so the engine needs no per provider code

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0032](0032-a-state-write-owns-its-announcement.md), [0097](0097-webhooks-get-their-own-socket.md), [0122](0122-a-webhook-claims-a-delivery-before-it-emits.md)

## Context

Inbound webhooks come from senders that sign deliveries in different ways.
GitHub, Stripe and Slack each use their own header, signed string and digest
encoding.

`core/webhooks.rs` holds the webhook configuration and the checks a delivery
must pass. This record keeps the design choices that shaped it.

This record restates the design comments of
`crates/lucidos-core/src/core/webhooks.rs`, written before the comment strip
removes them. Nobody checked it line by line against the code.

## Decision

- Signature checking is data. `HmacConfig` names the header that carries the
  signature, how to extract it, what string is signed, and how the digest is
  encoded.
- Deduping is data in the same way. The sender names the header that carries
  its delivery id.
- The secret is never in the config. `credential` names a row in the
  `credentials` table, which stays the only home for the value.
- The config of a webhook, apart from its identity, is grouped into one struct.
- An update has every field optional, and `None` keeps the stored value.
  `dedupe` is switched off by a config with `window_secs: 0`. `hmac` takes a
  named three-state.
- A new webhook token comes from `lucidos-local-token`, together with `ct_eq`.
- The webhook registry emits the create, update and delete events itself, and
  is registered in `core::announced_surfaces`.
- A refusal reason is an enum arm. `verify` produces five arms, and
  `api::webhooks::deliver` produces the other two.
- Database rows are read into a named struct before the JSON columns are
  decoded.

## Rationale

- GitHub, Stripe and Slack are all expressible as config, so none of the three
  needs engine code. A fourth sender is a config change.
- The grouped config keeps `create` under an argument count that anyone can
  read at a call site. The grouped fields travel together and distinguish one
  hook from another.
- `window_secs: 0` means no clear variant is needed. This avoids the
  `Option<Option<_>>` an optional JSONB column otherwise wants. `hmac` has no
  off value, so a named three-state is used. A shape nobody has to decode beats
  a nested option.
- Four hand-copies of a secret's minting and comparison drifted. A stale copy
  is a caller that silently cannot authenticate.
- Emitting from the registry means every mutation path announces by
  construction, and no caller can skip the event.
- Every refusal answers 401, so the sender learns nothing. The reason is
  recorded in the log, the row and the refusal run instead.
- Making the two `deliver` reasons arms rather than hand-written strings lets
  `refusal_run_reasons` key on a closed set. It also lets
  `examined_the_delivery` classify every reason.
- A named row struct means a column added in the middle of `WEBHOOK_COLUMNS`
  cannot silently shift every field after it.

## Consequences

- Adding a sender needs a config entry and no engine change.
- The `credentials` table is the single place a webhook secret lives.
- Every refusal reason must be an arm of the enum. A new reason needs a new
  arm, and it must be classified by `examined_the_delivery`.
- Webhook mutations cannot bypass the announcement, because the emit lives in
  the registry.
- Token minting and comparison have one implementation, in
  `lucidos-local-token`.

## Alternatives considered

- Engine code for each sender such as GitHub, Stripe or Slack. Rejected in
  favour of config fields.
- A nested `Option<Option<_>>` for clearing `dedupe`. Rejected because
  `window_secs: 0` already means off.
- Passing the identity and every setting as separate arguments to `create`.
  Rejected because the call site becomes unreadable.
- Hand-written copies of token minting and comparison. Rejected because four
  copies drifted.
- Emitting events from the call sites of create, update and delete. Rejected
  because a caller could skip one.
- Hand-written strings for the two refusal reasons that `deliver` produces.
  Rejected for a closed set of arms.
- A tuple for a row as read. Rejected because a column added in the middle
  could shift every field after it.
