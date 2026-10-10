/** A stand-in for the two event targets `utils/pageVisit.ts` binds, so a test can
 *  fire the browser's real vocabulary (`visibilitychange` on the document,
 *  `pagehide` / `pageshow` / `focus` on the window) and drive `visibilityState`
 *  by hand.
 *
 *  Shared rather than duplicated, because the module is a SINGLETON over real
 *  `document` / `window` listeners: every suite that drives a background or a
 *  wake has to install the same fake and reset the same module state, and two
 *  copies of that setup drift the moment the event set changes.
 *
 *  Not a `.test.ts` file, so vitest does not try to collect it. */
export function installFakePage() {
  const docListeners = new Map<string, Set<() => void>>();
  const winListeners = new Map<string, Set<() => void>>();

  const add = (m: Map<string, Set<() => void>>) => (type: string, fn: () => void) => {
    if (!m.has(type)) m.set(type, new Set());
    m.get(type)!.add(fn);
  };
  const remove = (m: Map<string, Set<() => void>>) => (type: string, fn: () => void) => {
    m.get(type)?.delete(fn);
  };
  const fire = (m: Map<string, Set<() => void>>, type: string) => {
    for (const fn of [...(m.get(type) ?? [])]) fn();
  };

  const prevDoc = (globalThis as any).document;
  const prevWin = (globalThis as any).window;

  const doc = {
    visibilityState: 'visible' as 'visible' | 'hidden',
    addEventListener: add(docListeners),
    removeEventListener: remove(docListeners),
    // Present so a consumer that resolves the visible transcript by scanning the
    // document (`scrollState`'s `findVisibleThreadContent`) finds nothing rather
    // than throwing. Tests that need a container register it explicitly.
    querySelectorAll: () => [] as unknown[],
    querySelector: () => null,
  };
  const win = {
    addEventListener: add(winListeners),
    removeEventListener: remove(winListeners),
  };
  (globalThis as any).document = doc;
  (globalThis as any).window = win;

  return {
    doc,
    /** How many listeners the module currently holds, across both targets. */
    listenerCount() {
      let n = 0;
      for (const s of docListeners.values()) n += s.size;
      for (const s of winListeners.values()) n += s.size;
      return n;
    },
    /** The full burst one real background delivers. */
    background() {
      doc.visibilityState = 'hidden';
      fire(docListeners, 'visibilitychange');
      fire(winListeners, 'pagehide');
    },
    /** The full burst one real iOS wake delivers. */
    foreground() {
      doc.visibilityState = 'visible';
      fire(docListeners, 'visibilitychange');
      fire(winListeners, 'focus');
      fire(winListeners, 'pageshow');
    },
    /** Another window handing focus back, with this page never having gone away. */
    windowFocus() {
      fire(winListeners, 'focus');
    },
    /** The `pageshow` every page load fires, bfcache restore or not. */
    pageshow() {
      fire(winListeners, 'pageshow');
    },
    restore() {
      (globalThis as any).document = prevDoc;
      (globalThis as any).window = prevWin;
    },
  };
}
