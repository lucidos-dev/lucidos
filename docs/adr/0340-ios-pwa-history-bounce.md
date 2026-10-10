# 0340: The installed iOS app steps forward off a page it reached by going back

- **Status**: Accepted
- **Date**: 2026-10-01

## Context

A swipe right from the left screen edge is iOS's back gesture. On the iOS PWA,
a swipe from the thread drawer took the user back to a bare boot splash.

The app already keeps its own navigations off the back stack. Every move between
Lucidos documents replaces the current entry
(`docs/plans/2026-08-21-workspace-navigation-never-pushes-history.md`). That plan
relied on a flat stack, because the edge guard (`shouldSuppressEdgeNavigation`)
has holes by construction: it skips buttons so taps still fire, and it never sees
a touch on the fixed mobile header.

One push is outside our control. An iOS notification tap is a cross-document
navigation, the only channel WebKit applies (temporary measure "Cross-document
notification-tap reload on iOS"). Safari runs that navigation, so it pushes. The
page the user was on stays one entry back, and a stray swipe lands on it. That
page reloads into its boot splash.

## Decision

In a Safari web app (`navigator.standalone`: the iOS home-screen app, or a macOS
Dock one), a page reached by a history traversal steps forward again at once. An inline `<head>` script in
`crates/lucidos-app/index.html` runs `history.forward()` on a `back_forward` load
and on a back-forward cache restore.

## Rationale

The back stack cannot be kept flat, so the fix is to make a traversal into it
inert. A Safari web app has no back-to-exit, and the iOS one has no back button.
So a back traversal there is never something the user asked for. Stepping forward returns
them to the page they were on, which is what the gesture should have left alone.

The script sits in `<head>` so it runs before the bundle loads and before the
stale page boots. It sets `window.lucidosHistoryBounce`, and the picker's
cold-start redirect stands down on it. A redirect scheduled after the step would
replace it, and leave a second copy of the workspace on the stack.

Where the Navigation API reports nothing forward, the script does nothing. A
restored session reloads its current entry as a traversal, and flagging it would
stand the picker's redirect down for a step that never happens.
Without that API the script cannot tell, and still sets the flag. That cannot
cost the redirect, because the redirect replaces its own entry. So a picker page
that would redirect is never an entry a session restores. The picker pages that
do stay (`?pick`, `?pair`, nothing remembered) never redirect anyway.

## Consequences

- A leaked edge swipe now costs a brief flash: iOS animates back, then the page
  steps forward again. If the live page left the back-forward cache, it reboots.
- Back keeps working everywhere else. A browser tab, an Android install and the
  desktop app are untouched, since there back is the user's.
- The guard's holes stay as they are, by the 2026-08-21 plan's reasoning.
- If WebKit ships a reload-free notification channel, the stack goes flat again
  and this script becomes a backstop. It stays: any future push lands here too.

## Alternatives considered

- **Close the guard's holes.** Cancelling the touchstart on a button also cancels
  its emulated click, so taps would have to be re-synthesized on touchend. That
  touches every edge control, the long-press menus and focus rules, for a guard
  that would still miss a touch landing outside the container.
- **A popstate sentinel entry.** Popstate fires only for same-document entries,
  so it never sees a traversal into another document. It would also trap a
  browser-tab user, as the 2026-08-21 plan notes.
- **Bounce in every standalone install.** On Android, back at the oldest entry
  closes the app. Bouncing there would leave a user with older entries unable to
  leave by back at all.
