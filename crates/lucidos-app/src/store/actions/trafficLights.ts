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
 * It is also live, so the push FOLLOWS THE RENDERED BAND. Four things move that
 * band: the UI scale, a Style Remote retune, the desktop-to-mobile switch, and
 * the first frame it is laid out. Each was once a call site to remember. A
 * measurement taken in a layout the window then left stuck for the rest of the
 * session, because nothing measured again.
 *
 * `watchTitlebarBand` is the one mechanism now, and it is what stops the lights
 * centring on a bar that is not on screen.
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
 *  ONE read, of the rendered band's bottom edge, rather than a sum of two
 *  tokens or a `3rem` restated in TypeScript: under the overlay build the
 *  viewport's top edge IS the window's top edge, so the distance down to the
 *  bottom of the band is exactly `--titlebar-inset + --app-header-height`,
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

/** How a surface DECLARES the band the lights centre on.
 *
 *  Declared, never a class name this module knows. The app shell puts it on
 *  `.app-header`; the picker, which mounts no shell, renders its own strip.
 *  Keyed on `.app-header` instead, the picker measured nothing and pushed
 *  nothing. Its window then wore whichever bar some other page had persisted,
 *  which is not even the same size: that page runs at the user's UI scale, and
 *  the picker runs at the browser default. */
export const TITLEBAR_BAND_SELECTOR = '[data-titlebar-band]';

export function measureHeaderBarHeight(): number | null {
  const header = document.querySelector(TITLEBAR_BAND_SELECTOR) as HTMLElement | null;
  if (!header) return null;
  const bottom = header.getBoundingClientRect().bottom;
  if (!Number.isFinite(bottom) || bottom <= 0) return null;
  return bottom < header.offsetHeight - ROUNDING_SLACK_PX ? null : bottom;
}

/** Whether this window wears the OS buttons. `data-titlebar-overlay` is stamped
 *  pre-paint by `titlebar_inset_script` and exists nowhere else, so it is the one
 *  signal that means "this window has native lights". A build without them has
 *  nothing to place and must not call the command at all. */
function hasNativeLights(): boolean {
  return isTauri() && document.documentElement.hasAttribute('data-titlebar-overlay');
}

/** Keep the shell's placement on the band for as long as this surface is
 *  mounted. Returns the teardown, so an effect can hand it straight back.
 *
 *  TWO SIGNALS, because the lights centre on the band's BOTTOM and that moves in
 *  two ways. A `ResizeObserver` catches the band's own box, which is what a UI
 *  scale or a retuned token changes. Its first delivery is the boot push, so a
 *  band laid out late still reports itself. A window resize catches everything
 *  ABOVE the band: the reclaimed strip is a flow sibling on desktop, and the
 *  band covers it on mobile. Crossing the breakpoint therefore moves the bottom
 *  without resizing the band.
 *
 *  Gated on the CLIENT rather than on the lights, which is weaker than the push
 *  and deliberately so. `data-titlebar-overlay` arrives by an injected eval, and
 *  a watch declined for a missing attribute is declined for the life of the
 *  page: the effect runs once. The push re-reads the attribute per observation
 *  instead, so a late stamp costs one observation rather than every one. */
export function watchTitlebarBand(): () => void {
  const band = document.querySelector(TITLEBAR_BAND_SELECTOR);
  if (!band || !isTauri()) return () => {};
  const push = (): void => pushTrafficLightOffset();
  const observer = new ResizeObserver(push);
  // BORDER-BOX, because that is the box whose bottom edge is being pushed. The
  // default content box misses a change to the band's own padding or border,
  // and `.app-header` pads by a safe-area inset. `useHideOnScroll` observes the
  // same element the same way.
  observer.observe(band, { box: 'border-box' });
  window.addEventListener('resize', push);
  return () => {
    observer.disconnect();
    window.removeEventListener('resize', push);
  };
}

/** Push the measured bar height to the shell, so it re-centres the traffic
 *  lights on it. The one writer, called by [`watchTitlebarBand`] whenever the
 *  band could have moved.
 *
 *  Best-effort telemetry carve-out (.claude/rules/frontend.md): nothing here is
 *  user-initiated, and a failure self-heals the next time the band moves, with
 *  the shell meanwhile holding the last position it was given. A toast would
 *  report an invisible cosmetic miss to a user who did not ask for anything. */
export function pushTrafficLightOffset(): void {
  if (!hasNativeLights()) return;
  const barHeightPx = measureHeaderBarHeight();
  if (barHeightPx === null || barHeightPx === lastPushedPx) return;
  lastPushedPx = barHeightPx;
  setTrafficLightOffset(barHeightPx).catch((e) => {
    // Give the de-duplication back, or the self-healing above is a lie: a
    // transient IPC failure would otherwise be remembered as a successful push,
    // and every later observation measuring the SAME bar would skip. The lights
    // would stay at the last position the shell managed to apply, for the rest
    // of the session. The next observation retries instead.
    lastPushedPx = 0;
    console.warn('[titlebar] traffic-light placement failed', e);
  });
}
