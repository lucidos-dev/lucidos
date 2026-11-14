# 0646: Workspace order: one list per gateway, keyed by route segment

- **Status**: Accepted
- **Date**: 2026-11-10

## Context

The picker and the in-app switcher drew local workspaces in registry order and
remote workspaces in a separate block below them. Nothing could be reordered.
The user wanted one list, local and remote mixed, in an order they set by
dragging in the picker.

A remote workspace has no row in this gateway's registry. It is a listing
fetched from a paired *remote gateway*, so a registry column cannot hold its
position.

## Decision

The gateway stores one ordered list of keys in
`<app-data>/config/workspace-order.json`. The key is the route segment the
workspace opens at: `<slug>` for a local workspace, `<slug>@<gateway>` for a
remote one. `GET /~/api/v1/control/workspaces` serves it as `order`, and
`PUT /~/api/v1/control/workspace-order` replaces it.

A workspace with no key goes after the ordered ones: local in registry order,
then remote by gateway. So an empty order is the old list.

## Rationale

- **The route segment already names both kinds.** `workspaceRouteSegment`
  computes it on the client, and the gateway routes on it. No new identifier is
  needed, and local and remote keys can never collide.
- **One store on the gateway serves every device.** Picker and switcher, on any
  paired device, read the same field from the same listing.
- **A separate file keeps the registry's shape.** The registry stays a record of
  local engines. The order file writes atomically through the same
  temp-then-rename helper, so a crash never leaves it torn.
- **Stale keys are harmless.** The client merge drops keys with no listed
  workspace and appends listed workspaces with no key. Every row shows exactly
  once whatever the file holds.

## Consequences

- Renaming a remote gateway on this side changes its workspaces' keys. They
  fall to the end until placed again. Nothing is lost.
- Deleting a local workspace drops its key, so a later workspace with the same
  slug does not inherit the old position.
- Each gateway keeps its own order. A remote gateway's order never reaches this
  one.
- Only the picker writes the order. The switcher reads it.

## Alternatives considered

- **An `order` column per registry row.** It covers local workspaces only. Remote
  rows would still need a second store, and two stores cannot express one mixed
  list.
- **Order kept in the browser.** Each device would show a different list, and a
  new device would start from scratch.
- **Key by workspace id.** A remote listing carries no id this gateway owns, and
  an id is not stable across a restore. The route segment is what the user sees
  and what links use.
- **Order synced across gateways.** It needs a protocol between gateways for a
  cosmetic preference. Out of scope.
