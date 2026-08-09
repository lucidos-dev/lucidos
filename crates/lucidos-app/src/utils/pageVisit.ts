/** The page going AWAY and coming BACK, as two paired signals.
 *
 *  A background is not a close, and the browser says so in several voices at
 *  once: one iOS wake delivers `visibilitychange`, `focus` and `pageshow`
 *  together, and one background delivers `visibilitychange` and `pagehide`. A
 *  handler bound naively to all of them runs several times per transition.
 *
 *  ── Paired, not throttled ───────────────────────────────────────────────────
 *  The coalescing here is a STATE PAIRING (a hide arms the next wake, and only a
 *  wake disarms it), not a time window. `useStartup` throttles the same event
 *  set on a leading-edge gate and is right to: its work is a reconciliation
 *  fan-out that is wanted whenever the app is brought forward, so approximating
 *  "one wake" with a window costs nothing. The consumers here need the opposite
 *  guarantee, exactly one signal per real away-and-back, and a window gets that
 *  wrong in both directions. Backgrounding and returning inside the window would
 *  swallow the second transition, which for a save-on-hide means losing exactly
 *  the write it exists to protect. And a bare desktop window `focus` with no
 *  hide before it would count as a wake, which for a consumer that repositions
 *  the reader means moving them every time they click back into the window.
 *
 *  Pairing answers both exactly: a wake fires only if a hide preceded it, so a
 *  window focus, and the `pageshow` every fresh page load fires, both pass in
 *  silence.
 *
 *  `blur` is deliberately NOT a hide. It fires when another window takes focus
 *  while this page stays fully visible, so treating it as away would make the
 *  paired `focus` a wake and reintroduce the very thing pairing prevents.
 *
 *  ── Why not `onPageResume` ──────────────────────────────────────────────────
 *  `utils/pageResume.ts` already subscribes to the wake side, but it is iOS-only
 *  BY DESIGN: it also arms a click-swallow for the first tap after a resume, and
 *  arming that on desktop would eat a legitimate click-to-focus. These signals
 *  carry no such hazard and their consumers need them everywhere, so they are
 *  their own module rather than a flag on that one. */

type Listener = () => void;

const wakeListeners = new Set<Listener>();
const hideListeners = new Set<Listener>();

let installed = false;
/** Whether the page is currently considered AWAY. The whole pairing is this one
 *  bit: a hide sets it and fires the hide side, a wake clears it and fires the
 *  wake side, and an event that would not change it fires nothing. */
let away = false;

function fire(listeners: Set<Listener>): void {
  // Copied, so a listener that unsubscribes itself (or a sibling) mid-fire
  // cannot mutate the set being iterated.
  for (const cb of [...listeners]) cb();
}

function goAway(): void {
  if (away) return;
  away = true;
  fire(hideListeners);
}

function comeBack(): void {
  if (!away) return;
  away = false;
  fire(wakeListeners);
}

function onVisibilityChange(): void {
  if (typeof document === 'undefined') return;
  if (document.visibilityState === 'hidden') goAway();
  else comeBack();
}

/** The exact targets `install` bound to, so `uninstallIfIdle` removes from what
 *  it added to rather than from whatever the globals happen to be by then. The
 *  two differ in a suite that swaps in a fake page and restores it, which is the
 *  only way to drive these transitions at all: removing from the restored global
 *  would silently leave the fake's listeners attached and this module still
 *  wired to a page nobody can see. */
let boundDoc: typeof document | null = null;
let boundWin: typeof window | null = null;

function install(): void {
  if (installed) return;
  if (typeof document === 'undefined' || typeof window === 'undefined') return;
  if (typeof document.addEventListener !== 'function') return;
  if (typeof window.addEventListener !== 'function') return;
  installed = true;
  boundDoc = document;
  boundWin = window;
  // Seeded from the real state rather than assumed visible, so a subscriber that
  // arrives while the page is already hidden still gets its wake.
  away = document.visibilityState === 'hidden';
  boundDoc.addEventListener('visibilitychange', onVisibilityChange);
  boundWin.addEventListener('pagehide', goAway);
  boundWin.addEventListener('pageshow', comeBack);
  boundWin.addEventListener('focus', comeBack);
}

function uninstallIfIdle(): void {
  if (!installed || wakeListeners.size > 0 || hideListeners.size > 0) return;
  installed = false;
  boundDoc?.removeEventListener('visibilitychange', onVisibilityChange);
  boundWin?.removeEventListener('pagehide', goAway);
  boundWin?.removeEventListener('pageshow', comeBack);
  boundWin?.removeEventListener('focus', comeBack);
  boundDoc = null;
  boundWin = null;
}

function subscribe(listeners: Set<Listener>, cb: Listener): () => void {
  install();
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
    uninstallIfIdle();
  };
}

/** Called once each time the page comes back to the foreground after having
 *  gone away. Returns the unsubscribe. */
export function onPageWake(cb: Listener): () => void {
  return subscribe(wakeListeners, cb);
}

/** Called once each time the page goes to the background. The page is NOT being
 *  torn down, so a subscriber should commit whatever is pending and keep
 *  working: the same document carries on after the paired wake. Returns the
 *  unsubscribe. */
export function onPageHide(cb: Listener): () => void {
  return subscribe(hideListeners, cb);
}

/** Reset every listener and the away bit. Test-only: the module is a singleton
 *  over real `document` / `window` listeners, so a suite that leaves either
 *  behind leaks into the next one. */
export function _resetPageVisitForTesting(): void {
  wakeListeners.clear();
  hideListeners.clear();
  uninstallIfIdle();
  away = false;
}
