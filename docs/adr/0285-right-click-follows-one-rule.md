# 0285: Right-click follows one rule: objects open their ⋯ menu, content keeps the native menu, chrome shows none

- **Status**: Accepted
- **Date**: 2026-09-25

## Context

A right-click in the desktop app showed WebKit's own menu almost everywhere.
Over transcript text that is useful: Look Up, Translate, Copy, Speech. Over
empty header chrome it offered Reload, Inspect Element and AutoFill. Reload
throws the whole app away, and the other two mean nothing there.

A few surfaces had already claimed right-click, each on its own terms. A mobile
drawer row opened its ⋯ menu on a long press, but a desktop right-click on the
same row fell through to WebKit. The workspace switcher and notification rows
offered their alternate open mode. Nothing said what a new surface should do.

## Decision

A right-click resolves to one of three cases.

| Target | Menu |
|---|---|
| An object with actions: a thread row, a draft row | Our own menu, the same component as the object's ⋯ menu |
| Content: an input, a link, media, a selection, or the text of code, rendered markdown or the transcript | The native menu |
| Anything else: empty chrome | None |

The third case applies in the desktop app only. **Option+right-click always
shows the native menu.**

## Rationale

This is where desktop apps built on web views have converged, and it matches
what a Mac user expects from a native app. Each case follows from one question:
does the menu offer something useful here?

- **An object menu is a shortcut to actions the user already has.** Making it
  the ⋯ menu itself means the two can never drift, and there is one set of
  items to test. The ⋯ stays drawn, so the context menu is never the only path.
- **The native text menu is OS integration we cannot rebuild.** Dictionary,
  translation, Services and spelling come from macOS. A home-made copy would be
  worse on every one of them.
- **The native chrome menu offers nothing useful and one footgun.** Reload in a
  packaged app discards the running state behind the user's back.

A browser tab keeps its native menu over chrome. There the menu belongs to the
browser (Back, Reload, Save, Inspect), and a web page that removes it is
fighting its host. Option+right-click keeps Inspect Element within reach in a
dev build, and Reload within reach if the app is ever wedged.

## Consequences

- A new surface with a ⋯ menu wires right-click to the same menu. It passes
  the pointer position so the menu opens under the cursor, as a native one does.
- A new surface showing readable text opts in with `data-native-context-menu`.
  Without it, that text is chrome in the desktop app, and a right-click on it
  shows nothing. Inside any text region (code, rendered markdown, a marked
  region) only the text itself keeps the native menu. The space between it is
  chrome, since WebKit shows its page menu there. A menu raised from the
  keyboard or VoiceOver has no pointer, so a text region keeps it whole.
- A claimed right-click (any handler that calls `preventDefault`) is never
  overridden by the chrome rule. The workspace switcher's alternate-mode row
  keeps working unchanged.
- App iframes and preview iframes own their own right-click. A `contextmenu`
  event does not cross a document boundary.
- WebKit selects the word under a right-click before the event fires. When the
  menu is suppressed, that selection is undone, so no stray highlight remains.

## Alternatives considered

- **Leave the native menu everywhere.** Rejected: Reload over chrome is a
  footgun, and a right-click on a thread row doing nothing useful reads as a
  missing feature.
- **Suppress the native menu everywhere and build our own text menu.**
  Rejected: we would lose Look Up, Translate, Services and spelling, and gain a
  large surface to maintain.
- **A separate, shorter context menu per object.** Rejected: two menus for one
  object drift apart, and the user learns two layouts.
- **Suppress chrome menus in browsers too.** Rejected: the browser owns that
  menu, and suppressing it hides Back and Inspect from users who expect them.
- **Keep Inspect Element only in dev builds, keyed on the build mode.**
  Rejected: the build-watch always produces a production bundle, so a developer
  would lose it anyway. Option+right-click works in every build.
