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
    // Loading a web font appends a <link> here, and that is now on the DEFAULT
    // path: Fira Code is the default UI font, so `applyPreferences()` and
    // `applyFontFamily()` reach this on a run with no preferences set at all.
    // Without the stub they throw "Cannot read properties of undefined", in a
    // test that was only ever about the theme or the device id.
    head: { appendChild: () => {} },
  };
}
// There is no layout engine here, so nothing can be measured. Store modules
// still read the root font size at load (the thread drawer's width floor is
// derived from it), and an undefined global throws at import rather than
// degrading. The stub answers the browser default; the pure
// `computeMinDrawerWidth` is where the arithmetic is exercised at other roots.
if (typeof (globalThis as any).getComputedStyle === 'undefined') {
  (globalThis as any).getComputedStyle = () => ({
    fontSize: '16px',
    getPropertyValue: () => '',
  });
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
// Node ≥22 ships a Web Storage global that is unusable here: without a
// `--localstorage-file <path>` it throws (Node 22/23) or warns and degrades
// (Node 25) the moment it is touched, so every test importing a store module
// used to crash at import with "localStorage.getItem is not a function".
//
// Install the in-memory stub UNCONDITIONALLY. Tests want a deterministic,
// per-process store that starts empty and shares nothing with the machine —
// never a real on-disk one — so there is no case where keeping the platform's
// implementation is right, and probing to find out is itself the problem: the
// old code called `getItem('__probe__')` on the built-in, and Node's accessor
// emits "`--localstorage-file` was provided without a valid path" on that read.
// One warning per vitest worker meant 365 lines of noise in a full run (the
// 2026-07-26 nightly's BuildClean concern 2) — enough to bury a real warning.
//
// `Object.defineProperty`, not assignment: Node defines `localStorage` as a
// getter on `globalThis` (node:internal/webstorage), and a plain assignment to
// a getter-only accessor throws in the ESM module scope. Redefining the
// property replaces the accessor outright, so the built-in is never invoked.
function installStorage(name: 'localStorage' | 'sessionStorage'): void {
  Object.defineProperty(globalThis, name, {
    value: makeStorage(),
    writable: true,
    configurable: true,
  });
}
installStorage('localStorage');
installStorage('sessionStorage');
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
