// Browser globals needed by store modules that access localStorage/matchMedia/document at load time.
// These are minimal stubs — just enough to let modules initialize without jsdom.
if (typeof (globalThis as any).window === 'undefined') {
  (globalThis as any).window = globalThis;
}
// Default to desktop viewport; tests can override window.innerWidth as needed.
if (typeof (globalThis as any).innerWidth === 'undefined') {
  (globalThis as any).innerWidth = 1024;
}
if (typeof globalThis.document === 'undefined') {
  const docListeners: Record<string, Function[]> = {};
  (globalThis as any).document = {
    createElement: (tag: string) => ({
      tagName: tag.toUpperCase(),
      textContent: '',
      innerHTML: '',
      get innerText() { return (this as any).textContent; },
    }),
    querySelector: () => null,
    querySelectorAll: () => [],
    // Default to "page is active" so isPageActive() returns true for tests
    // that don't explicitly simulate a blurred window. Tests that want to
    // exercise the inactive branch override document.hasFocus per-test.
    hasFocus: () => true,
    visibilityState: 'visible',
    addEventListener: (type: string, fn: Function) => {
      (docListeners[type] ??= []).push(fn);
    },
    removeEventListener: (type: string, fn: Function) => {
      const fns = docListeners[type];
      if (fns) docListeners[type] = fns.filter(f => f !== fn);
    },
    dispatchEvent: (event: any) => {
      for (const fn of docListeners[event.type] ?? []) fn(event);
      return true;
    },
    documentElement: {
      style: { setProperty: () => {}, getPropertyValue: () => '', removeProperty: () => {} },
      toggleAttribute: () => {},
      setAttribute: () => {},
      removeAttribute: () => {},
      hasAttribute: () => false,
    },
  };
}
function makeStorage(): Storage {
  const store: Record<string, string> = {};
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, val: string) => { store[key] = val; },
    removeItem: (key: string) => { delete store[key]; },
    clear: () => { for (const k of Object.keys(store)) delete store[k]; },
    get length() { return Object.keys(store).length; },
    key: (i: number) => Object.keys(store)[i] ?? null,
  };
}
// Node ≥22 ships an experimental Web Storage global that throws on access
// unless the process was started with `--localstorage-file <path>`. A bare
// `typeof === 'undefined'` guard leaves that broken global in place, so every
// test importing a store module crashes at import with
// "localStorage.getItem is not a function". Install the in-memory stub
// whenever the real one is missing OR non-functional.
function storageWorks(s: unknown): boolean {
  try {
    if (!s || typeof (s as Storage).getItem !== 'function') return false;
    (s as Storage).getItem('__probe__');
    return true;
  } catch {
    return false;
  }
}
if (!storageWorks((globalThis as any).localStorage)) {
  (globalThis as any).localStorage = makeStorage();
}
if (!storageWorks((globalThis as any).sessionStorage)) {
  (globalThis as any).sessionStorage = makeStorage();
}
// Minimal EventTarget on window — enough for addEventListener/dispatchEvent in tests.
if (typeof (globalThis as any).addEventListener === 'undefined') {
  const listeners: Record<string, Function[]> = {};
  (globalThis as any).addEventListener = (type: string, fn: Function) => {
    (listeners[type] ??= []).push(fn);
  };
  (globalThis as any).removeEventListener = (type: string, fn: Function) => {
    const fns = listeners[type];
    if (fns) listeners[type] = fns.filter(f => f !== fn);
  };
  (globalThis as any).dispatchEvent = (event: any) => {
    for (const fn of listeners[event.type] ?? []) fn(event);
    return true;
  };
}
if (typeof (globalThis as any).PopStateEvent === 'undefined') {
  (globalThis as any).PopStateEvent = class PopStateEvent {
    type = 'popstate';
    state: any;
    constructor(_type: string, init?: { state?: any }) { this.state = init?.state ?? null; }
  };
}
if (typeof (globalThis as any).history === 'undefined') {
  (globalThis as any).history = {
    state: null,
    pushState: (_state: any, _title: string, _url?: string) => {},
    replaceState: (_state: any, _title: string, _url?: string) => {},
    back: () => {},
    forward: () => {},
    go: () => {},
    length: 1,
  };
}
if (typeof globalThis.matchMedia === 'undefined') {
  (globalThis as any).matchMedia = () => ({
    matches: false,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  });
}
if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: FrameRequestCallback) => { cb(0); return 0; };
  (globalThis as any).cancelAnimationFrame = () => {};
}
