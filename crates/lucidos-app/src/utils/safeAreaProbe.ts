import { postClientLog } from './clientLog';
import type { SafeAreaInsets } from './safeAreaFloor';

/** Logs what an installed iOS app reads for its top edge, and where the header lands.
 *
 *  A DIAGNOSTIC, registered in `docs/temporary-measures.md` § 1. It confirms
 *  the header's place under the status bar (ADR 0553): whether iOS moved the
 *  page below the bar, whether `env()` reports the inset, and whether the floor
 *  added it back. It lands in the gateway log
 *  (`docs/client-logs.md`).
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
  postClientLog('safe-area', moment, {
    env: reading,
    floor,
    viewport: [window.innerWidth, window.innerHeight],
    screen: [window.screen.width, window.screen.height],
    visualViewport: window.visualViewport
      ? [window.visualViewport.offsetTop, window.visualViewport.height]
      : null,
    header: rect(header),
    headerPaddingTop: header ? getComputedStyle(header).paddingTop : null,
    firstButton: rect(header?.querySelector('button')),
  });
}

/** An element's top and height, in CSS pixels. */
function rect(el: Element | null | undefined): [number, number] | null {
  if (!el) return null;
  const box = el.getBoundingClientRect();
  return [Math.round(box.top), Math.round(box.height)];
}
