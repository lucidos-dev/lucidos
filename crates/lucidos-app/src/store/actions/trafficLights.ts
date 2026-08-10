/**
 * Telling the shell where our header bar ends, so it can centre the macOS
 * traffic lights on it.
 *
 * The lights are OS chrome floating over the webview under
 * `titleBarStyle: "Overlay"`, and the shell places them itself
 * (`src/traffic_lights.rs`) rather than leaving them where AppKit put them. It
 * can compute the x on its own, but not the y: the bar is `--titlebar-inset`
 * plus `--app-header-height`, and the second is `3rem`, so the ROOT FONT SIZE
 * decides it (48px at 100% UI scale, 72px at 150%). That number exists only
 * here.
 *
 * It is also live. UI scale is a preference, and the Style Remote can retune the
 * tokens the bar is built from over SSE, so this is pushed again on every apply
 * rather than once at boot. Both call sites already exist for exactly the same
 * reason: `applyUiScale` and `applyStyleOverrides` in `./preferences.ts` each
 * call `clampThreadDrawerWidth()` because the drawer's floor is rem-authored
 * too, and this sits beside it.
 */
import { isTauri } from '../../utils/platform';
import { setTrafficLightOffset } from '../../utils/tauri';

/** The last height we pushed, so a re-apply that measures the same bar (the
 *  common case: `applyUiScale` runs on every preferences load) does not spend an
 *  IPC round trip saying nothing. `0` is "nothing pushed yet" and can never be a
 *  measurement, since a bar with no height would mean no header. */
let lastPushedPx = 0;

/** Reset the de-duplication. Test seam only: the module-level cache would
 *  otherwise leak the first test's value into the next one. */
export function resetTrafficLightPush(): void {
  lastPushedPx = 0;
}

/** The height of the bar the lights have to centre on, in CSS px, or `null`
 *  when there is nothing mounted to measure.
 *
 *  ONE read, of the rendered header's bottom edge, rather than a sum of two
 *  tokens or a `3rem` restated in TypeScript: under the overlay build the
 *  viewport's top edge IS the window's top edge, so the distance down to the
 *  bottom of `.app-header` is exactly `--titlebar-inset + --app-header-height`,
 *  whatever those resolve to. `.titlebar-strip` is a static flow sibling above
 *  the header on desktop, and on a narrow (mobile-layout) window the header is
 *  fixed at `top: 0` and covers the strip, so the same read is right in both
 *  layouts. A custom property would not do: `getComputedStyle` returns an
 *  unregistered custom property's substituted token sequence, so
 *  `--app-header-height` comes back as the literal string `calc(3rem - 28px)`,
 *  not a length.
 *
 *  The rect is the PAINTED position, so it has to be rejected when the header is
 *  not where layout put it. It can be: `useHideOnScroll` translates the header
 *  up as the user scrolls, and while that is gated on the mobile layout, a
 *  packaged macOS window narrower than 769px gets that layout with real traffic
 *  lights still on it. Mid-hide the rect would report a short bar, the lights
 *  would centre on it, and the de-duplication would hold that reading until the
 *  scale changed again. Comparing against `offsetHeight`, which no transform
 *  touches, is what tells the two apart: the header is fixed at `top: 0` in that
 *  layout, so at rest its bottom IS its height, and anything less means it has
 *  been translated away. On desktop the strip above it puts the bottom strictly
 *  higher, so the comparison never fires.
 *
 *  The pixel of slack is `offsetHeight` being an INTEGER: at a fractional root
 *  font size it can round up past the rect's own bottom, and without the slack
 *  the rounding alone would reject an at-rest header. A hide-on-scroll translate
 *  moves by tens of pixels, so nothing it needs to catch is inside the slack. */
const ROUNDING_SLACK_PX = 1;

export function measureHeaderBarHeight(): number | null {
  const header = document.querySelector('.app-header') as HTMLElement | null;
  if (!header) return null;
  const bottom = header.getBoundingClientRect().bottom;
  if (!Number.isFinite(bottom) || bottom <= 0) return null;
  return bottom < header.offsetHeight - ROUNDING_SLACK_PX ? null : bottom;
}

/** Push the measured bar height to the shell, so it re-centres the traffic
 *  lights on it. No-op unless this is the packaged macOS build: the attribute is
 *  stamped pre-paint by `titlebar_inset_script` and exists nowhere else, so it
 *  is the one signal that means "this window has native lights". A build without
 *  them has nothing to place and must not call the command at all.
 *
 *  Best-effort telemetry carve-out (.claude/rules/frontend.md): nothing here is
 *  user-initiated, and a failure self-heals on the next scale or style apply,
 *  with the shell meanwhile holding the last position it was given. A toast
 *  would report an invisible cosmetic miss to a user who did not ask for
 *  anything. */
export function pushTrafficLightOffset(): void {
  if (!isTauri()) return;
  if (!document.documentElement.hasAttribute('data-titlebar-overlay')) return;
  const barHeightPx = measureHeaderBarHeight();
  if (barHeightPx === null || barHeightPx === lastPushedPx) return;
  lastPushedPx = barHeightPx;
  setTrafficLightOffset(barHeightPx).catch((e) => {
    // Give the de-duplication back, or the self-healing above is a lie: a
    // transient IPC failure would otherwise be remembered as a successful push,
    // and every later apply measuring the SAME bar would skip, leaving the
    // lights at the last position the shell managed to apply for the rest of
    // the session. The next apply retries instead.
    lastPushedPx = 0;
    console.warn('[titlebar] traffic-light placement failed', e);
  });
}
