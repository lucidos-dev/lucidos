# 0763: Webhook delivery verifies raw bytes before parsing and stays separate from webhook CRUD by path

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0097](0097-webhooks-get-their-own-socket.md)

## Context

The webhooks module in `crates/lucidos-agents-api/src/api/webhooks.rs` serves
two audiences under one prefix.

The user manages hooks through CRUD routes. A sender calls one route, `POST
/webhooks/:id/deliver`, which the hook socket forwards.

The delivery route is reachable by anything on the public internet that the
user pointed at it. The CRUD routes are not.

This record restates the design comments of
`crates/lucidos-agents-api/src/api/webhooks.rs`, written before the comment
strip removes them. Nobody checked it line by line against the code.

## Decision

- Keep the two surfaces apart by path, not only by method. Nobody has to notice
  a verb to see which is which.
- Delivery reads the body as raw bytes and verifies before it parses anything.
- A request that brings a credential says where its value comes from. Absent
  keeps the older shape: the credential already exists and `hmac.credential`
  names it.
- `Generate` has the engine invent the shared secret. `Provided` takes a secret
  the sender issued.
- Both arms write one credential and then name it on the hook. They differ only
  in where the value came from.
- A secret a request brings is checked first as a `PendingSecret`, and written
  later through `PendingSecret::write`.
- A header allow-list that would carry a secret is refused. The rest of the
  list is normalised.

## Rationale

- A public endpoint must not run a parser for a caller it has not accepted. The
  signature is over those exact raw bytes anyway.
- GitHub takes whatever secret the receiver puts in its webhook form, so
  `Generate` is right there. It is the safer default, because a hand-typed
  secret is the realistic weak link.
- Slack and Stripe issue their own secret and show it in their console. Those
  senders take `Provided` and nothing else can work.
- Splitting the check from the write keeps a failed mutation from changing a
  live hook.
- The events table is append-only, so a secret copied into a payload stays
  there permanently.
- An allow-list lets the refusal reach whoever configured the hook.

## Consequences

- A reader can tell the public route from the user's routes by path alone.
- Delivery runs no parser before the signature check passes.
- The credential value source depends on the sender, so a request for Slack or
  Stripe must use `Provided`.
- A rejected secret or header list leaves the live hook unchanged.
- A hook cannot be configured to record a header that carries a secret.

## Alternatives considered

- Telling the two surfaces apart by HTTP verb alone is rejected. The path
  carries the difference instead.
- Filtering headers at write time is rejected. It would silently drop a header
  the configurer expected to see, where an allow-list refuses visibly.
