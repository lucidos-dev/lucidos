// @ts-expect-error Node APIs are available in Vitest, outside the app type environment.
import { randomUUID } from 'node:crypto';
import { afterAll, afterEach, beforeAll, expect } from 'vitest';
import { ensureNavigator } from './test-navigator';
import { evaluatedFiles, lateMarkdownMessage, reachesMarkdown } from './test-markdown-preload';

const broadcastChannelScope = randomUUID();
globalThis.BroadcastChannel = class extends BroadcastChannel {
  constructor(name: string) {
    super(`${broadcastChannelScope}:${name}`);
  }
};

ensureNavigator(globalThis);

if (typeof (globalThis as any).window === 'undefined') {
  (globalThis as any).window = globalThis;
}
if (typeof globalThis.BroadcastChannel === 'function') {
  const Shared = globalThis.BroadcastChannel;
  const scope = crypto.randomUUID();
  globalThis.BroadcastChannel = class extends Shared {
    constructor(name: string) {
      super(`${scope}:${name}`);
    }
  };
}
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
      getAttribute: () => null,
      removeAttribute: () => {},
      hasAttribute: () => false,
    },
    head: { appendChild: () => {} },
  };
}
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
function installStorage(name: 'localStorage' | 'sessionStorage'): void {
  Object.defineProperty(globalThis, name, {
    value: makeStorage(),
    writable: true,
    configurable: true,
  });
}
installStorage('localStorage');
installStorage('sessionStorage');
if (typeof (globalThis as any).addEventListener === 'undefined') {
  const listeners: Record<string, Array<{ fn: Function; once: boolean }>> = {};
  (globalThis as any).addEventListener = (type: string, fn: Function, options?: boolean | AddEventListenerOptions) => {
    (listeners[type] ??= []).push({ fn, once: typeof options === 'object' && options.once === true });
  };
  (globalThis as any).removeEventListener = (type: string, fn: Function) => {
    const entries = listeners[type];
    if (entries) listeners[type] = entries.filter(e => e.fn !== fn);
  };
  (globalThis as any).dispatchEvent = (event: any) => {
    const entries = listeners[event.type] ?? [];
    listeners[event.type] = entries.filter(e => !e.once);
    for (const { fn } of entries) fn(event);
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
if (typeof (globalThis as any).Element !== 'undefined'
  && typeof (globalThis as any).Element.prototype.scrollIntoView !== 'function') {
  (globalThis as any).Element.prototype.scrollIntoView = () => {};
}
if (typeof (globalThis as any).ResizeObserver === 'undefined') {
  (globalThis as any).ResizeObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  };
}
if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: FrameRequestCallback) => { cb(0); return 0; };
  (globalThis as any).cancelAnimationFrame = () => {};
}
if (typeof (globalThis as any).CSS === 'undefined') {
  const hexEscape = (ch: string): string => `\\${ch.codePointAt(0)!.toString(16)} `;
  (globalThis as any).CSS = {
    escape(value: string): string {
      const s = String(value);
      let out = '';
      for (let i = 0; i < s.length; i++) {
        const ch = s[i];
        const code = s.charCodeAt(i);
        if (code === 0) { out += '�'; continue; }
        const isDigit = code >= 0x30 && code <= 0x39;
        if ((code >= 0x01 && code <= 0x1f) || code === 0x7f
          || (i === 0 && isDigit)
          || (i === 1 && isDigit && s.charCodeAt(0) === 0x2d)) {
          out += hexEscape(ch);
          continue;
        }
        if (i === 0 && code === 0x2d && s.length === 1) { out += `\\${ch}`; continue; }
        if (code >= 0x80 || code === 0x2d || code === 0x5f || isDigit
          || (code >= 0x41 && code <= 0x5a) || (code >= 0x61 && code <= 0x7a)) {
          out += ch;
          continue;
        }
        out += `\\${ch}`;
      }
      return out;
    },
  };
}

if (typeof (globalThis as any).requestAnimationFrame === 'function') {
  const realSetTimeout = globalThis.setTimeout;
  const pageDocument = document;
  const VisibilityEvent = Event;
  afterAll(() => {
    Object.defineProperty(pageDocument, 'visibilityState', { configurable: true, value: 'hidden' });
    pageDocument.dispatchEvent(new VisibilityEvent('visibilitychange'));
    return new Promise<void>(resolve => realSetTimeout(resolve, 40));
  });
}

const drawsHostMarkdown = (testPath: string): boolean => !testPath.includes('/packages/lucidos-sdk/');
let markdownPreloaded = false;
beforeAll(async () => {
  const testPath = expect.getState().testPath ?? '';
  if (!drawsHostMarkdown(testPath) || !reachesMarkdown(evaluatedFiles(), testPath)) return;
  markdownPreloaded = true;
  const [{ provideMarkdownEngine }, engine] = await Promise.all([
    import('./utils/renderMarkdown'),
    import('./utils/markdownEngine'),
  ]);
  await engine.loadMarkdownLibraries();
  provideMarkdownEngine(engine);
  const { loadSyntaxHighlighter } = await import('./utils/syntaxHighlight');
  await loadSyntaxHighlighter();
});
const refuseLateMarkdown = (): void => {
  const testPath = expect.getState().testPath ?? '';
  if (markdownPreloaded || !drawsHostMarkdown(testPath)) return;
  if (reachesMarkdown(evaluatedFiles(), testPath)) throw new Error(lateMarkdownMessage(testPath));
};
afterEach(refuseLateMarkdown);
afterAll(refuseLateMarkdown);
