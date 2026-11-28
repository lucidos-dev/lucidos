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
    listenerCount() {
      let n = 0;
      for (const s of docListeners.values()) n += s.size;
      for (const s of winListeners.values()) n += s.size;
      return n;
    },
    background() {
      doc.visibilityState = 'hidden';
      fire(docListeners, 'visibilitychange');
      fire(winListeners, 'pagehide');
    },
    foreground() {
      doc.visibilityState = 'visible';
      fire(docListeners, 'visibilitychange');
      fire(winListeners, 'focus');
      fire(winListeners, 'pageshow');
    },
    windowFocus() {
      fire(winListeners, 'focus');
    },
    pageshow() {
      fire(winListeners, 'pageshow');
    },
    restore() {
      (globalThis as any).document = prevDoc;
      (globalThis as any).window = prevWin;
    },
  };
}
