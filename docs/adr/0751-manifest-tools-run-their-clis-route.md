# 0751: A manifest tool runs the route its CLI calls, so one gate binds both

- **Status**: Accepted
- **Date**: 2026-11-20

## Context

The capability parity manifest generates each domain's LLM tool schema and
its `lucidos` CLI command from one declaration
([ADR 0018](0018-capability-parity-manifest.md)). The schemas matched, but
the behaviour did not. The CLI sends an HTTP request to the domain's route.
The grouped LLM tool ran a hand-written in-process handler that reached the
same state by another path.

So a gate on the route bound the CLI and missed the tool. ADR 0750 put every
intelligence ladder edit behind the owner's `edit-intelligence-ladder`
permission, on the routes. The chat agent's `manage_models` tool called
`runtime::chat_ladder` directly and skipped that permission.

## Decision

A manifest domain's grouped LLM tool runs the route its CLI calls. The
engine maps the action onto the request the generated CLI would send and
runs it through its own `/api/v1` router, in-process. The request carries
the origin token a `lucidos` command from the calling thread's bash tool
would carry. Such a tool is a *route-backed tool*, and its domain has no
hand-written handler.

Each `Domain` states its `llm_execution`, `Route` or `LegacyInProcess`, with
no default. Only the domains on a shrink-only legacy list may say
`LegacyInProcess`.

## Rationale

- **Drift is impossible by construction.** One code path serves the tool and
  the CLI, so a route's gate, validation and events bind both. Parity of
  schema without parity of behaviour was the gap.
- **One caller identity.** The route sees the same agent, from the same
  thread, whichever surface asked. Permission checks such as ADR 0750 need
  no second copy in a handler.
- **No network hop.** The in-process router passes every layer a live
  request passes, and works with the gateway down.
- **The guard is mechanical.** A test fails on an in-process domain off the
  list, and on a listed domain that is route-backed. So a new domain cannot
  skip its route, and the list only shrinks.

## Consequences

- `models` moved first. Its `enable` and `disable` actions became LLM-only
  operations on `PUT /api/v1/models`, sending a fixed body value
  (`ArgIn::FixedBody`) the caller cannot set.
- A route-backed tool returns the route's answer as JSON, not prose. A route
  refusal that prepared a permission card tells the agent how to ask with it.
- The other LLM domains stay in-process for now, on the legacy list
  (`docs/temporary-measures.md`, "Manifest tools that run in-process"). Each
  moves on its own change.
- A new manifest domain writes no handler: it declares `Route`.

## Alternatives considered

- **Repeat the route's gate inside each handler.** Rejected: two copies of
  every gate, and nothing stops the next one drifting. This is how
  `manage_models` missed the ladder permission.
- **Call the route over HTTP through the gateway.** Rejected: a network hop,
  a dependency on the gateway being up, and no gain over the in-process
  router, which runs the same layers.
- **Move every domain at once.** Rejected: fifteen handlers with their own
  quirks in one unreviewable change. The shrink-only list makes the
  migration safe to do one domain at a time.
- **Let the owner's own turn skip the gate.** Rejected in ADR 0750: a route
  cannot tell an owner's turn from an agent acting alone.
