# 0816: Dismissed windows leave the arrangement

- **Status**: Accepted
- **Date**: 2026-11-26

## Context

[ADR 0718](0718-ordinary-closes-retain-every-app-window.md) retained the page when
a user closed a packaged app window. The window session still recorded that
window, so an update relaunch brought it back.

## Decision

Cmd-W and the red close button dismiss the individual window. The client keeps
its page hidden and drops it from the arrangement. A park retains the
arrangement under [ADR 0141](0141-a-park-hides-its-windows.md). Teardown leaves
membership alone.

## Rationale

A park hides an arrangement the user expects to get back. Closing an individual
window changes that arrangement. Workspace selection and notification taps can
still reuse the retained page. Showing it again restores its membership.

The window session keeps geometry for dismissed windows. The write gate
considers every live app window, so closing the last one can write an empty
arrangement. The dismissal flag only lives as long as the retained page.

## Consequences

A Dock or tray reopen skips dismissed windows. If every window was dismissed,
a reopen can show the workspace picker. A relaunch restores the recorded
workspace windows. If the main window has no workspace to restore, it opens
the picker with auto-open disabled.

## Alternatives considered

- Record only visible windows. A parked arrangement would disappear on relaunch.
- Destroy individually closed windows. The retained page and connection would be lost.
