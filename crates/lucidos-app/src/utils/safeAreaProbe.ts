import { postClientLog } from './clientLog';
import type { SafeAreaInsets } from './safeAreaFloor';

/** Logs what an installed iOS app reads for its top edge, and where the header lands.
 *
 *  A DIAGNOSTIC, registered in `docs/temporary-measures.md` § 1. It confirms
 *  the header's place under the status bar (ADR 0553): whether iOS moved the
 *  page below the bar, whether `env()` reports the inset, and whether the floor
 *  added it back. It also logs the shell's written height beside WebKit's own
 *  readings, for a shell drawn at the wrong height (ADR 0610). It lands in the
 *  gateway log (`docs/client-logs.md`).
 *
 *  One line per change of the inset state, plus one snapshot once the header
 *  has painted, so a session costs a handful of lines. */

const SETTLED_DELAY_MS = 3000;
/** Up to a minute of waiting for the header. */
const SETTLED_ATTEMPTS = 20;

let last: { key: string; reading: SafeAreaInsets; floor: SafeAreaInsets | null } | null = null;

/** Reset the change filter. Test seam only. */
export function resetSafeAreaProbe(): void {
  last = null;
}

export function reportSafeAreaReading(reading: SafeAreaInsets, floor: SafeAreaInsets | null): void {
  const key = JSON.stringify([reading, floor, window.innerHeight]);
  if (key === last?.key) return;
  last = { key, reading, floor };
  logGeometry('reading', reading, floor);
}

/** Logs the latest reading again once the header has painted. A slow boot
 *  mounts the header late, so it waits for it, a bounded number of times. */
export function scheduleSafeAreaSnapshot(attemptsLeft = SETTLED_ATTEMPTS): void {
  setTimeout(() => {
    const headerMounted = document.querySelector('.app-header') !== null;
    if (!headerMounted && attemptsLeft > 1) {
      scheduleSafeAreaSnapshot(attemptsLeft - 1);
      return;
    }
    if (last) logGeometry('settled', last.reading, last.floor);
  }, SETTLED_DELAY_MS);
}

function logGeometry(moment: string, reading: SafeAreaInsets, floor: SafeAreaInsets | null): void {
  const header = document.querySelector('.app-header');
  const geometry = {
    env: reading,
    floor,
    viewport: [window.innerWidth, window.innerHeight],
    screen: [window.screen.width, window.screen.height],
    visualViewport: window.visualViewport
      ? [window.visualViewport.offsetTop, window.visualViewport.height]
      : null,
    cssViewport: cssViewportHeights(),
    header: rect(header),
    headerPaddingTop: header ? getComputedStyle(header).paddingTop : null,
    firstButton: rect(header?.querySelector('button')),
  };
  // The shell writes `--app-height` from its own listener for the same event,
  // which runs after this one. So the write is read once every listener has run.
  setTimeout(() => {
    const appHeight = document.documentElement.style.getPropertyValue('--app-height');
    postClientLog('safe-area', moment, { ...geometry, appHeight: appHeight || null });
  }, 0);
}

/** What WebKit's CSS viewport units resolve to, in CSS pixels: `[svh, lvh,
 *  dvh]` at 100 each. Tells a wrong `innerHeight` from a wrong CSS viewport
 *  (ADR 0610). */
function cssViewportHeights(): [number, number, number] {
  const box = document.createElement('div');
  box.style.cssText = 'position:fixed;top:0;left:0;width:0;visibility:hidden;pointer-events:none';
  document.body.appendChild(box);
  const heights = (['100svh', '100lvh', '100dvh'] as const).map((h) => {
    box.style.height = h;
    return Math.round(box.getBoundingClientRect().height);
  }) as [number, number, number];
  box.remove();
  return heights;
}

/** An element's top and height, in CSS pixels. */
function rect(el: Element | null | undefined): [number, number] | null {
  if (!el) return null;
  const box = el.getBoundingClientRect();
  return [Math.round(box.top), Math.round(box.height)];
}
