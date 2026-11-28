# 0128: An OAuth authorization page always opens outside the app, whatever the in-app browser toggle says

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

The desktop app has an experimental in-app browser, a Tauri child webview behind
a Settings toggle. While it is on, `openUrl` mounts every URL in the url-preview
panel. `openOAuthAuthorizationUrl` called `openUrl`, so a sign-in page landed
there too.

A user connecting a Google account got a panel that rendered nothing. The header
still read the hostname, because `headerHelpers.ts` falls back to the hostname
when the live page title is null. So the panel looked loaded and was blank.

## Decision

An authorization URL goes to `openUrlOutsideApp`, never `openUrl`. The in-app
browser toggle gets no vote on a sign-in page.

## Rationale

The provider owns the sign-in page, and several refuse to serve it inside an
embedded webview. Google's OAuth policy says so outright. We cannot fix a
refusal from our side, and it can arrive as a blank render rather than an error.

The callback is a loopback redirect the engine listens on. So even a page that
rendered would end with the user staring at a dead callback page inside the app.

A browsing preference is about where the user reads links. It says nothing about
where they want to sign in to somebody else's account.

## Consequences

`handleOAuthAccountConnected` no longer closes a panel, because an authorization
never opens one. `OAuthAuthFlow` dropped its `url` field with it: nothing matches
a panel any more, so the marker only records that this page started the flow.

A desktop user with the toggle on now gets the OS browser for a sign-in, and the
in-app panel for every other link.

This reverses half of `fix(oauth): open the authorization page the way the user
configured`. The other half stands. The opener is still not a bare
`window.open`, so an installed iOS PWA keeps its Safari escape and a blocked
popup still toasts.

## Alternatives considered

**Keep `openUrl` and carve the panel out for OAuth.** Rejected: the panel is the
exact thing the provider refuses. A carve-out inside it changes nothing.

**Detect the refusal and fall back to the OS browser.** Rejected: there is no
reliable signal. A curl carrying the app's user agent got HTTP 200 and a full
sign-in page. So the status code says nothing, and the refusal can surface as a
blank render carrying no error event.

**Send a desktop user agent from the child webview.** Rejected: that is evasion
against a stated provider policy, and it breaks whenever they tighten the check.

**Turn the toggle off for the duration of a flow.** Rejected: it rewrites a user
preference behind their back, and two windows authorizing at once would race it.
