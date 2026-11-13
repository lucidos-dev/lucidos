import { postClientLog } from './clientLog';
import type { ShellSnapBack } from './shellScrollGuard';

/** Logs each time the desktop shell had to be snapped back into place.
 *
 *  A DIAGNOSTIC, registered in `docs/temporary-measures.md` § 1. On the
 *  packaged macOS app the whole shell sometimes slid up its window, header
 *  first, until the window showed only the page background. Scrolling or a
 *  reopen put it back. Neither Chromium nor WebKit reproduces it under
 *  Playwright, so nothing names what scrolled it. The line does: which box
 *  moved, by how much, and what held focus. It lands in the gateway log
 *  (`docs/client-logs.md`).
 *
 *  Rate-limited, so a script fighting the guard costs one line a second and a
 *  count, never a flood. A burst's last snap-back is logged when its second
 *  ends, so the count never stays behind in memory. */

const MIN_GAP_MS = 1000;

let lastAt = -Infinity;
let held: ShellSnapBack | null = null;
let suppressed = 0;
let flushTimer: ReturnType<typeof setTimeout> | null = null;

/** Reset the rate limit. Test seam only. */
export function resetShellScrollProbe(): void {
  if (flushTimer) clearTimeout(flushTimer);
  flushTimer = null;
  lastAt = -Infinity;
  held = null;
  suppressed = 0;
}

export function reportShellSnapBack(snap: ShellSnapBack): void {
  const wait = lastAt + MIN_GAP_MS - performance.now();
  if (wait > 0) {
    if (held) suppressed++;
    held = snap;
    flushTimer ??= setTimeout(() => held && logSnapBack(held), wait);
    return;
  }
  logSnapBack(snap);
}

function logSnapBack(snap: ShellSnapBack): void {
  if (flushTimer) clearTimeout(flushTimer);
  flushTimer = null;
  if (held && held !== snap) suppressed++;
  held = null;
  lastAt = performance.now();
  const html = document.documentElement;
  postClientLog('shell-scroll', 'snap-back', {
    ...snap,
    suppressed,
    focus: describeElement(document.activeElement),
    viewport: [window.innerWidth, window.innerHeight],
    visibility: document.visibilityState,
    overlay: html.hasAttribute('data-titlebar-overlay'),
  });
  suppressed = 0;
}

/** A tag and its classes: enough to find the control, and no content. */
function describeElement(el: Element | null): string {
  if (!el || el === document.body) return 'body';
  const classes = typeof el.className === 'string' ? el.className.trim().split(/\s+/).slice(0, 3).join('.') : '';
  return classes ? `${el.tagName.toLowerCase()}.${classes}` : el.tagName.toLowerCase();
}
