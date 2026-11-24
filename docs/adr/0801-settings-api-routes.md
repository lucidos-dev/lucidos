# 0801: The settings API gives credential scope edits their own replacing verb

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0161](0161-a-credential-scope-is-a-set-of-base-urls.md), [0247](0247-one-model-row-many-routes.md)

## Context

`crates/lucidos-agents-api/src/api/settings.rs` serves the Settings routes for
credentials, models, response styles and Access.

The shape of each route records a separate decision. This record keeps them
together.

This record restates the design comments of
`crates/lucidos-agents-api/src/api/settings.rs`, written before the comment
strip removes them. Nobody checked it line by line against the code.

## Decision

- **Credential scope edits.** `lucidos credentials set-base-urls` has its own
  verb. It replaces one credential's set of base URLs and does not append.
- **Scope validation.** A scope the proxy gate could not parse is refused at
  the form. A body naming neither scope field is refused too. An empty list is
  a real answer and must be written down.
- **Model efforts.** `GET /api/v1/models` returns the full registry. Each route
  carries the reasoning efforts its backend supports. The server derives them
  per request from the route's provider and wire id, and does not store them.
- **Response styles.** `GET /api/v1/response-styles` is read-only and returns
  the merged style library. Shipped styles come first, then the user's own.
  Each row says whether it is shipped, overridden or the user's own.
- **Style writes.** The library is written only through `PUT /preferences` on
  `response_styles`, where the one write chokepoint bounds-checks it.
- **Builtin model edits.** A builtin keeps its identity, so `label` and
  `sort_order` stay engine-owned. It still accepts `on_chat_ladder`, `routes`,
  `preferred_provider` and `vision`. `provider` and `context_window` apply to
  the first route. User models update any provided field.
- **Tailnet status.** What the engine sees of the tailnet has its own route. It
  is not part of `NetworkConfigResponse`. The last answer sits in a
  process-wide in-memory slot.

## Rationale

- A whole-row credential edit names the auth type and defaults the auth header.
  A script widening a scope through it would clobber fields it never meant to
  touch.
- Replacing makes the command state the whole resulting set. A reader of the
  shell history then sees what the credential now covers.
- Refusing a bad scope at the form avoids a 502 at the proxy gate, far from the form. The user thought that call was configured.
- Efforts depend on the backend. The same Claude id offers six efforts on
  Vertex and four through OpenRouter. The picker must offer exactly what
  `RoutingProvider` will send.
- Efforts are a pure function of provider and wire id. A re-routed model is
  therefore right immediately. A user adding a local model never has to declare
  efforts they cannot know.
- Keeping the shipped style instructions in Rust gives them a single home.
  Mirrored into TypeScript, they would drift the first time either side was
  reworded.
- Which backends serve a model is a fact that changes. `preferred_provider` is
  the model picker's own write path. Neither is identity. A vendor can raise a
  context window, and a seeded value can be wrong.
- Both Access panes fetch `NetworkConfigResponse` separately and read disjoint
  fields. That is safe only while it stays the cheap local call `SettingsView`
  documents. Tailnet fields cost a Tailscale query and a network round trip,
  and the bind editor reads none of them.
- One engine serves one workspace, so the process-wide slot is per-workspace by
  construction. `CLAUDE.md` § Engine Statelessness allows it because the next
  miss rebuilds it and a restart loses nothing.

## Consequences

- Scope changes need the dedicated verb. A script cannot change auth settings
  by accident.
- The scope body always states a scope explicitly. A `secret` that goes nowhere
  is recorded as an empty list.
- The style library has one write path and one source for shipped text.
- The editor can choose Reset or Delete from each row's shipped, overridden or
  own status.
- Every seeded model can be re-routed or given a new preferred backend without
  losing its identity fields.
- Tailnet status can be slower than the rest of Settings without slowing the
  bind editor.

## Alternatives considered

- Widening a scope through the whole-row credential edit, rejected because it
  clobbers the auth type and header.
- Appending to a scope rather than replacing it, rejected because the command
  would no longer state the whole resulting set.
- Storing efforts on the model row, or per row rather than per route, rejected. The answer depends on the backend, and a stored value can go stale.
- Mirroring the shipped styles in TypeScript, rejected because the two copies
  would drift.
- Refusing `preferred_provider` on a builtin, rejected because it would pin
  every seeded model to the first backend the migration listed.
- Folding tailnet status into `NetworkConfigResponse`, rejected because that
  endpoint must stay cheap and local.
