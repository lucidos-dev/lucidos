# 0786: System events name one aggregate per fact and carry the resulting state rather than a delta

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0143](0143-webhook-ingress-probed-per-address-family.md), [0113](0113-persisted-system-events-are-subscribable.md)

## Context

`SystemEvent` variants in `event_bus_system_event.rs` each describe one fact.
Each variant needs an aggregate to follow, a persistence rule and a payload
shape. The code comments recorded these choices one variant at a time. This
record gathers them.

This record restates the design comments of
`crates/lucidos-core/src/engine/event_bus_system_event.rs`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- An upsert is one variant. `PluginMarketplaceRegistered` covers both a new
  registration and a re-registration under a new name. There is no `Renamed`
  variant.
- The payload states what the thing is now. It does not state what changed.
  `PluginMarketplaceRegistered` carries the resulting `name` and `source`. The
  disabled-tools event carries the full `disabled_tools` set by wire name, not
  added and removed lists.
- Each event follows the aggregate a reader would follow, not the code that
  caused it. Marketplace events use their own aggregate, not plugin. A
  marketplace scan shares the marketplace aggregate. A standing apply follows
  the one thread it is armed for. A frontend preview follows the one thread
  whose worktree it shows.
- A change to the agent's tool surface gets its own variant. The disabled-tools
  change is separate from `McpServerUpdated`, which is an auto-approve change.
- Pure UI signals are transient. They are never persisted. The marketplace scan
  start, the frontend preview signal and the Thread Queue panel refresh are
  examples.
- A persisted event can be the stored state itself. The latest capacity policy
  event is the stored policy. It is rebuilt at boot, with no separate config
  table.
- The engine reports and never repairs. The webhook ingress events are
  edge-triggered and carry no actor, because nobody asked for them.

## Rationale

- Marketplace registration is an upsert. `add_marketplace` keys on a hash of
  the canonical source. Re-registering the same source rewrites the existing
  entry's `name` and its raw `source` string. The raw string can differ by a
  `.git` suffix or owner/repo casing while hashing the same. A `Renamed`
  variant would misdescribe the source-only case. `RepositoryAdded` and
  `McpServerRegistered` are upserts for the same reason.
- A marketplace is the source a plugin can be installed from, and registering
  one installs nothing. That is why it is not part of the plugin aggregate.
- A standing apply is armed for one thread, and a reader follows that thread.
  Cancelling a sweep drops many arms, and each arm reports on its own thread.
  So the sweep is a field, not the aggregate.
- A user picks a tool selection, so one event that states the full selection is
  easier to read back than a pair of lists. A disabled tool leaves every
  request, so it moves the agent's tool surface the way `McpServerRemoved` does
  for a whole server.
- The scan runs at startup and every five minutes. Persisting it would add a
  heartbeat row and say nothing an audit wants. What a marketplace is stays
  audited by the registered and removed events.
- The frontend preview carries a port, never a URL. The same workspace is
  reached at `localhost` from a laptop and at a Tailscale name from a phone.
  Only the page knows which one the user is on, so it composes the href from
  its own `location`.
- The Thread Queue refresh covers in-memory user occupants that are not
  persisted as `thread_queue` rows. Background changes already fire persisted
  `ThreadQueue*` events.
- Webhook degradation is emitted after two consecutive cycles see the same
  failure. A workspace trigger decides what to do. Recovery needs one clean
  cycle. Being wrong in that direction costs a second declaration, while
  waiting costs a workspace that thinks it is still blind.
- The webhook refusal event is edge-triggered off the hook's own refusal run,
  so a standing fault emits nothing per cycle. The ingress probe cannot see
  these refusals, because a 401 is the healthy answer there.
- The outage duration covers the whole run, not the part since it was declared.
  Nothing is declared before the 30-minute floor, so the declaring event's own
  age understates every outage.

## Consequences

- A reader of a marketplace event sees its current `name` and `source`. It
  cannot tell a rename from a source-only change.
- Scans leave no audit rows. The audit of marketplaces rests on the registered
  and removed events alone.
- The preview signal never reaches the event store. Each client builds its own
  link from the port.
- The capacity policy needs a replay at boot to be reconstructed.
- A standing fault in a webhook produces no repeated events. Only the edges are
  visible.
- Webhook events have no actor to attribute.
- The payload shape of the ingress degradation event is pinned by ADR 0143.

## Alternatives considered

- A `Renamed` variant for marketplaces. It would misdescribe the source-only
  re-registration.
- Filing marketplaces under the plugin aggregate. A marketplace is a source,
  and registering one installs nothing.
- Making the sweep the aggregate of a standing apply. A cancelled sweep drops
  many arms, each reporting on its own thread.
- A field on `McpServerUpdated` for disabled tools. That event is an
  auto-approve change.
- Added and removed lists for disabled tools. One event stating the full
  selection is easier to read back.
- Persisting the scan start. It would add a heartbeat row and tell an audit
  nothing.
- Putting a URL in the frontend preview event. Only the page knows which host
  the user is on.
- A separate config table for the capacity policy. The latest persisted event
  already is the policy.
