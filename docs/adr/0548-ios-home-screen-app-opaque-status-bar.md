# 0548: The iOS home-screen app runs below an opaque status bar, never black-translucent, since iOS 26+ blurs the page under a translucent one

- **Status**: Accepted
- **Date**: 2026-10-22

## Context

The installed iPhone app declared `apple-mobile-web-app-status-bar-style:
black-translucent`, so the page ran under the status bar and the header
painted behind it. On iOS 27 the header turned blurry: icons, badges, and
any menu open over it. The blur stops about 40pt below the status bar.

iOS draws that blur, not the page. It is iOS's scroll edge effect, a layer
above the web view, so it also blurs content that stacks over the header.

Three page-side fixes each failed on the device:

1. Moving a transparent fixed layer off the top edge.
2. Giving the header a solid `background-color` under its gradient.
3. A fixed, solid strip under the status bar from first paint.

## Decision

The app declares `default`, an opaque status bar. The page starts below it,
and `theme-color` (the header's top colour) tints it.

## Rationale

WebKit hides the edge effect only behind a *fixed colour extension*: a view
it paints over the obscured top inset, in the colour of the fixed box at the
top edge (`WKWebView _updateFixedColorExtensionViews`). It builds that view
only when the top obscured inset is above zero. Under `black-translucent`
that inset is 0, since the page itself runs under the status bar. So the
extension never exists and the blur never hides, whatever the page paints.
That is why all three fixes above failed.

Under an opaque bar nothing of the page sits under the status bar. Projects
that hit the same blur confirmed on iOS 27 devices that the opaque style
clears it.

## Consequences

- iOS reads the style when the app is added to the home screen. An install
  made before this change keeps the blur until it is removed and added again.
  The new install has its own storage, so it is a new device and pairs again.
  Nothing in the app tells an existing install to do this.
- The header no longer reaches up behind the status bar.
  `env(safe-area-inset-top)` is 0, so `--safe-area-top` and the header's top
  padding collapse on their own.
- `theme-color` now carries `--header-bar-top`, not `--bg-primary`, on every
  platform, so the bar above the page continues the header. The shell's boot
  script sets it before first paint, from a theme's literal token or the
  mode's default (`THEME_MODE_HEADER_TOP`). `paintThemeColor` then resolves
  the painted token on every mode, theme or override change.
- The gateway's splash page lifts the tag from `index.html`, so the bar keeps
  its colour across the hand-over.
- Screens without the header (the picker, the pairing screen and both boot
  splashes) show the header's colour above the brand gradient. An opaque bar
  cannot continue a radial gradient, so some seam there is unavoidable.

## Alternatives considered

- **Keep `black-translucent` and paint the top edge.** It cannot work, as the
  Rationale shows. It is the approach that failed three times.
- **Keep `black-translucent` and push the header's content below the blur**,
  about 40pt of clearance under the inset. It needs no reinstall, but the top
  of every screen becomes a tall, blurred, empty band.
- **Accept the blur.** It makes the header controls hard to read.
- **Restore a solid `background-color` under the header's gradient**, in case
  iOS samples it for the opaque bar's edge. A theme may set `--header-gradient`
  to a flat colour. A second colour in the shorthand is then invalid, and a
  longhand paints over the theme's colour.
