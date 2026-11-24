# 0726: A remote workspace's app popout opens the workspace shell at an #app= landing

- **Status**: Accepted
- **Date**: 2026-11-17
- **Amends**: [0044](0044-sdk-surfaces-render-without-a-host.md)

## Context

ADR 0044 makes the app popout a bare app tab with no host around it. ADR 0723
then gave every relayed remote document an opaque sandbox. A bare popout of a
remote workspace's app is therefore opaque and has no parent frame:

- The SDK's `isBridged()` needs `window.parent !== window`, so the app fetches
  directly.
- The connecting gateway refuses that fetch, because its `Origin` is `null`.
- `localStorage` throws in an opaque document.

An app that only draws still works there. An app that reads data or uses
storage breaks.

## Decision

A remote workspace's popout opens that workspace's own shell at
`/<slug>@<gateway>/#app=<id>[&fragment=<encoded>]`. The shell opens the app in
pseudo-fullscreen, inside the ordinary app frame. A local workspace's popout
stays the bare app tab of ADR 0044.

`appPopoutHref()` in `store/actions/apps.ts` is the one place that chooses.
`appHash.ts` writes and parses the hash, and `hash-deeplink-router.ts` lands it.

## Rationale

The shell is the trusted host the opaque document lacks. Inside it the app has
the full host bridge: fetch, SSE, storage and the `lucidos.ui.*` surfaces.

This is ADR 0044's rejected "shell-carrying popout" alternative. It lost there
on one premise: it arranges for a shell to be nearby instead of removing the
dependency on one. For a remote workspace a shell must be nearby anyway, since
ADR 0723 grants a relayed document no authority of its own.

The hash is a frontend-only channel and never a `WorkspaceLanding` member. The
popout hands the URL to an anchor or the OS opener, so no Rust mirror changes.

## Consequences

ADR 0044 named two costs of this shape, and a remote popout now pays both:

- **The tab loads the whole shell**, with its own SSE connection, where a bare
  tab loads only the app.
- **A `NavigationRequested` can replace the app.** The app-iframe path always
  applies, so an app in another window can evict the popped-out one.

Also:

- A remote popout opens the live app. It carries no WIP preview thread and no
  widget params.
- The landing consumes its hash after one attempt, so a reload shows the shell.
- An unknown app id shows `openAppById`'s usual toast and opens nothing.
- The tab passes the push handler's shell test, so a notification deep link can
  route into it.

## Alternatives considered

**Keep the bare popout for remote workspaces.** Rejected: apps that read data
or use storage break in it, as the context shows.

**Relax the sandbox on a top-level load of a relayed document.** Rejected: that
hands the remote workspace's scripts the connecting gateway's browser authority,
which is the boundary ADR 0723 exists to keep.

**Hide the popout control on a remote workspace.** Rejected by the user: the
popout is worth its two costs, and hiding it loses the feature.

**Make `#app=` a `WorkspaceLanding` member.** Rejected: nothing in Rust reads
it, and a member would add a mirror to keep in step for no caller.
