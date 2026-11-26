import { describe, it, expect, vi, afterEach, onTestFinished } from 'vitest';
import {
  hasDeepLinkParams,
  parseDeepLinkFromUrl,
} from '../store/actions/notification-deeplink';
import { THREAD_HASH_RE } from '../store/actions/cross-workspace';
import { normalizeBasePath } from './basePath';
import { STATUS_SWAP_MS } from './bootSplash';
import { SHELL_CACHE_PREFIX } from '../hooks/sw-update';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));


function installFakeSplash(present: boolean, initialClasses: string[] = []) {
  const statusClasses = new Set<string>();
  const statusEl = {
    textContent: '',
    classList: { toggle: (c: string, on: boolean) => { on ? statusClasses.add(c) : statusClasses.delete(c); } },
  };
  const listeners: Record<string, Array<(e: { target: unknown }) => void>> = {};
  let removed = false;
  const classes = new Set<string>(initialClasses);
  const splashEl = {
    classList: { add: (c: string) => classes.add(c), contains: (c: string) => classes.has(c) },
    addEventListener: (type: string, fn: (e: { target: unknown }) => void) => { (listeners[type] ??= []).push(fn); },
    removeEventListener: (type: string, fn: (e: { target: unknown }) => void) => {
      listeners[type] = (listeners[type] ?? []).filter(f => f !== fn);
    },
    remove: () => { removed = true; },
    fire: (type: string, target: unknown) => { for (const fn of [...(listeners[type] ?? [])]) fn({ target }); },
  };
  const childEl = {};
  const prev = (globalThis as any).document.querySelector;
  (globalThis as any).document.querySelector = (sel: string) => {
    if (!present) return null;
    if (sel === '.boot-splash') return splashEl;
    if (sel.includes('.boot-splash-status')) return statusEl;
    const state = /^\.boot-splash\.(boot-splash-[a-z-]+)$/.exec(sel);
    if (state) return classes.has(state[1]) ? splashEl : null;
    return null;
  };
  return {
    statusEl,
    statusShown: () => statusClasses.has('boot-splash-status-shown'),
    statusIsReport: () => statusClasses.has('boot-splash-status-report'),
    statusSwapping: () => statusClasses.has('boot-splash-status-swap'),
    hasLeaving: () => classes.has('boot-splash-leaving'),
    isRemoved: () => removed,
    fireAnimationEnd: () => splashEl.fire('animationend', splashEl),
    fireChildAnimationEnd: () => splashEl.fire('animationend', childEl),
    restore: () => { (globalThis as any).document.querySelector = prev; },
  };
}

async function freshController() {
  vi.resetModules();
  return import('./bootSplash');
}

function installActionSplash() {
  type FakeButton = {
    type: string; className: string; textContent: string; removed: boolean;
    onClick?: () => void; addEventListener: (type: string, fn: () => void) => void; remove: () => void;
  };
  const buttons: FakeButton[] = [];
  const attributes: Record<string, string> = { 'aria-hidden': 'true' };
  const shown = () => buttons.find((b) => !b.removed) ?? null;
  const status = { after: (b: FakeButton) => { buttons.push(b); } };
  const splash = {
    querySelector: (sel: string) => (sel === '.boot-splash-status' ? status : sel === '.boot-splash-action' ? shown() : null),
    setAttribute: (name: string, value: string) => { attributes[name] = value; },
  };
  const doc = (globalThis as any).document;
  const prev = { querySelector: doc.querySelector, createElement: doc.createElement };
  doc.querySelector = (sel: string) => (sel === '.boot-splash' ? splash : null);
  doc.createElement = () => {
    const button: FakeButton = {
      type: '', className: '', textContent: '', removed: false,
      addEventListener: (_type, fn) => { button.onClick = fn; },
      remove: () => { button.removed = true; },
    };
    return button;
  };
  onTestFinished(() => Object.assign(doc, prev));
  return { buttons, shown, attributes };
}

describe('setBootAction', () => {
  it('adds one button under the status, keeps it across re-sends, and removes it', async () => {
    const fake = installActionSplash();
    const c = await freshController();
    const onPress = vi.fn();

    c.setBootAction({ label: 'Trust certificate', onPress });
    c.setBootAction({ label: 'Trust certificate', onPress });
    expect(fake.buttons).toHaveLength(1);
    expect(fake.shown()).toMatchObject({ type: 'button', className: 'boot-splash-action', textContent: 'Trust certificate' });
    expect(fake.attributes['aria-hidden']).toBe('false');
    fake.shown()?.onClick?.();
    expect(onPress).toHaveBeenCalledOnce();

    c.setBootAction({ label: 'Try again', onPress });
    expect(fake.shown()?.textContent).toBe('Try again');
    expect(fake.buttons).toHaveLength(2);

    c.setBootAction(null);
    expect(fake.shown()).toBeNull();
  });
});

describe('bootSplash controller', () => {
  let fake: ReturnType<typeof installFakeSplash>;
  afterEach(() => fake?.restore());

  it('reports presence, and absence when the node is gone', async () => {
    fake = installFakeSplash(true);
    const c = await freshController();
    expect(c.bootSplashPresent()).toBe(true);

    fake.restore();
    fake = installFakeSplash(false);
    const c2 = await freshController();
    expect(c2.bootSplashPresent()).toBe(false);
  });

  it('setBootStatus updates the status line and reveals it; empty text hides it', async () => {
    fake = installFakeSplash(true);
    const c = await freshController();
    c.setBootStatus('Opening your workspace…');
    expect(fake.statusEl.textContent).toBe('Opening your workspace…');
    expect(fake.statusShown()).toBe(true);
    c.setBootStatus('');
    expect(fake.statusShown()).toBe(false);
  });

  it('a multi-line label switches the status into the wrapping report state', async () => {
    fake = installFakeSplash(true);
    const c = await freshController();
    c.setBootStatus(
      'The background service is not starting. It has started 3 times without coming up.\n' +
        'LUCIDOS_ENGINE_BIN does not exist: /R/lucidos-engine\n' +
        'Lucidos keeps trying. Log: /L/engine-service.err.log',
    );
    expect(fake.statusIsReport()).toBe(true);
    expect(fake.statusShown()).toBe(true);

    vi.useFakeTimers();
    try {
      c.setBootStatus('Waiting for the background service… (42s)');
      vi.advanceTimersByTime(STATUS_SWAP_MS);
      expect(fake.statusIsReport()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('crossfades a new label: out, swap while invisible, back in', async () => {
    vi.useFakeTimers();
    try {
      fake = installFakeSplash(true);
      const c = await freshController();
      c.setBootStatus('Opening your workspace…');
      c.setBootStatus('Connecting…');
      expect(fake.statusSwapping()).toBe(true);
      expect(fake.statusEl.textContent).toBe('Opening your workspace…');
      vi.advanceTimersByTime(STATUS_SWAP_MS - 1);
      expect(fake.statusEl.textContent).toBe('Opening your workspace…');
      vi.advanceTimersByTime(1);
      expect(fake.statusEl.textContent).toBe('Connecting…');
      expect(fake.statusSwapping()).toBe(false);
      expect(fake.statusShown()).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('lets a later label supersede one still fading out', async () => {
    vi.useFakeTimers();
    try {
      fake = installFakeSplash(true);
      const c = await freshController();
      c.setBootStatus('Opening your workspace…');
      c.setBootStatus('Connecting…');
      vi.advanceTimersByTime(STATUS_SWAP_MS - 50);
      c.setBootStatus('Loading…');
      vi.advanceTimersByTime(STATUS_SWAP_MS);
      expect(fake.statusEl.textContent).toBe('Loading…');
      vi.runAllTimers();
      expect(fake.statusEl.textContent).toBe('Loading…');
    } finally {
      vi.useRealTimers();
    }
  });

  it('ticks a counter in place, so the line never blinks once a second', async () => {
    fake = installFakeSplash(true);
    const c = await freshController();
    c.setBootStatus('Waiting for the background service… (9s)');
    c.setBootStatus('Waiting for the background service… (10s)');
    expect(fake.statusSwapping()).toBe(false);
    expect(fake.statusEl.textContent).toBe('Waiting for the background service… (10s)');
  });

  it('fades only a real change of words', async () => {
    const c = await freshController();
    expect(c.statusChangeFades('Opening your workspace…', 'Connecting…')).toBe(true);
    expect(c.statusChangeFades('Starting engine…', 'Recovering sessions…')).toBe(true);
    expect(c.statusChangeFades('Waiting… (59s)', 'Waiting… (1m 00s)')).toBe(true);
    expect(c.statusChangeFades('Waiting… (1m 05s)', 'Waiting… (1m 06s)')).toBe(false);
    expect(c.statusChangeFades('Connecting…', 'Connecting…')).toBe(false);
    expect(c.statusChangeFades('', 'Connecting…')).toBe(false);
    expect(c.statusChangeFades('Connecting…', '')).toBe(false);
  });

  it('dismiss adds the leaving class and removes the node on animationend', async () => {
    fake = installFakeSplash(true);
    const c = await freshController();
    c.dismissBootSplash();
    expect(fake.hasLeaving()).toBe(true);
    expect(fake.isRemoved()).toBe(false);
    fake.fireAnimationEnd();
    expect(fake.isRemoved()).toBe(true);
  });

  it('ignores an animationend bubbling up from a child of the splash', async () => {
    fake = installFakeSplash(true);
    const c = await freshController();
    c.dismissBootSplash();
    fake.fireChildAnimationEnd();
    expect(fake.isRemoved()).toBe(false);
    fake.fireAnimationEnd();
    expect(fake.isRemoved()).toBe(true);
  });

  it('dismiss removes via the timeout fallback when no animationend fires', async () => {
    vi.useFakeTimers();
    try {
      fake = installFakeSplash(true);
      const c = await freshController();
      c.dismissBootSplash();
      vi.advanceTimersByTime(700);
      expect(fake.isRemoved()).toBe(false);
      vi.advanceTimersByTime(200);
      expect(fake.isRemoved()).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('dismiss is idempotent and marks the splash absent', async () => {
    fake = installFakeSplash(true);
    const c = await freshController();
    c.dismissBootSplash();
    fake.fireAnimationEnd();
    expect(c.bootSplashPresent()).toBe(false);
    expect(() => c.dismissBootSplash()).not.toThrow();
  });

  it('reveals the escape through the inline document, and reports what happened', async () => {
    fake = installFakeSplash(true);
    const c = await freshController();
    const win = globalThis as unknown as { __lucidosGatewayEscape?: () => unknown };

    expect(c.revealBootEscape()).toBe(false);

    win.__lucidosGatewayEscape = () => null;
    expect(c.revealBootEscape()).toBe(false);

    win.__lucidosGatewayEscape = () => ({ href: 'https://example.com:5251/myws/' });
    expect(c.revealBootEscape()).toBe(true);
    delete win.__lucidosGatewayEscape;
  });

  it('dismiss fades the canvas to the app background on the veil curve, then hands it back', async () => {
    fake = installFakeSplash(true);
    const doc = (globalThis as any).document;
    const gradient = '#145eb9 radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%) no-repeat fixed';
    doc.documentElement.style.background = gradient;
    doc.body = { style: { background: gradient } };
    onTestFinished(() => { delete doc.body; });
    const prevGetComputedStyle = (globalThis as any).getComputedStyle;
    (globalThis as any).getComputedStyle = () => fake.hasLeaving()
      ? { animationDuration: '0.15s', animationTimingFunction: 'ease' }
      : { animationDuration: '0s', animationTimingFunction: 'ease' };
    onTestFinished(() => { (globalThis as any).getComputedStyle = prevGetComputedStyle; });
    const c = await freshController();
    c.dismissBootSplash();
    for (const style of [doc.documentElement.style, doc.body.style]) {
      expect(style.background).toBe('var(--bg-primary)');
      expect(style.transition).toBe('background-color 0.15s ease');
    }
    fake.fireAnimationEnd();
    for (const style of [doc.documentElement.style, doc.body.style]) {
      expect(style.background).toBe('');
      expect(style.transition).toBe('');
    }
  });

  it('dismiss starts the idle prefetch only once the splash is gone', async () => {
    vi.useFakeTimers();
    try {
      fake = installFakeSplash(true);
      const c = await freshController();
      const { prefetchWhenIdle } = await import('./idlePrefetch');
      const surface = { preload: vi.fn(() => Promise.resolve()) };
      prefetchWhenIdle(surface);
      c.dismissBootSplash();
      vi.advanceTimersByTime(0);
      expect(surface.preload).not.toHaveBeenCalled();
      fake.fireAnimationEnd();
      vi.advanceTimersByTime(0);
      expect(surface.preload).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('reports both no-reveal documents, and a plain launch as playing one', async () => {
    fake = installFakeSplash(true, ['boot-splash-formed']);
    expect((await freshController()).bootSplashPlaysNoReveal(), 'formed').toBe(true);

    fake.restore();
    fake = installFakeSplash(true);
    const root = (globalThis as any).document.documentElement;
    const hasAttribute = root.hasAttribute;
    root.hasAttribute = (name: string) => name === 'data-boot-splash-quiet';
    onTestFinished(() => { root.hasAttribute = hasAttribute; });
    expect((await freshController()).bootSplashPlaysNoReveal(), 'quiet').toBe(true);
    root.hasAttribute = hasAttribute;

    fake.restore();
    fake = installFakeSplash(true);
    const launch = await freshController();
    expect(launch.bootSplashPlaysNoReveal()).toBe(false);
  });

  it('dismiss reverts the root background immediately when the splash node is already gone', async () => {
    fake = installFakeSplash(false);
    const doc = (globalThis as any).document;
    doc.documentElement.style.background =
      '#145eb9 radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%) no-repeat fixed';
    const c = await freshController();
    c.dismissBootSplash();
    expect(doc.documentElement.style.background).toBe('');
  });
});

describe('index.html inline boot splash', () => {
  const html = readFileSync(resolve(__dirname, '../../index.html'), 'utf-8');
  const reducedMotionRules = () => (html.match(/:root\[data-motion="reduce"\][^{]*\{[^}]*\}/g) ?? []).join('\n');
  const bootHostSrc = readFileSync(
    resolve(__dirname, '../../../../packages/lucidos-sdk/src/boot/host.ts'), 'utf-8',
  );

  type RetryMark = 'none' | 'both' | 'url-only' | 'storage-only';

  interface DocContext {
    base?: string | null;
    gatewayPort?: string | null;
    workspaceId?: string | null;
    caches?: { keys(): Promise<string[]>; delete(name: string): Promise<boolean> };
    online?: boolean;
  }

  function runInlineWatchdog(initialRetry: boolean | RetryMark = false, doc: DocContext = {}) {
    const source = html.match(
      /\/\* lucidos-boot-watchdog-start[\s\S]*?\*\/([\s\S]*?)\/\* lucidos-boot-watchdog-end \*\//,
    )?.[1];
    if (!source) throw new Error('inline boot watchdog not found');

    const mark: RetryMark =
      initialRetry === true ? 'both' : initialRetry === false ? 'none' : initialRetry;
    const values = new Map<string, string>();
    if (mark === 'both' || mark === 'storage-only') values.set('lucidos-boot-retry', '1');
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
      removeItem: (key: string) => values.delete(key),
    };
    const statusClasses = new Set<string>();
    const status = {
      textContent: 'Opening your workspace…',
      classList: { add: (name: string) => statusClasses.add(name) },
    };
    let click: (() => void) | undefined;
    const splashClasses = new Set<string>();
    const escape = { href: '', textContent: '', hidden: true };
    const splash = {
      querySelector: (selector: string) =>
        selector === '.boot-splash-escape' ? escape : status,
      classList: { add: (name: string) => splashClasses.add(name) },
      setAttribute: vi.fn(),
      addEventListener: (_type: string, fn: () => void) => { click = fn; },
    };
    const metas: Record<string, string | null | undefined> = {
      'lucidos-gateway-port': doc.gatewayPort,
      'lucidos-workspace-id': doc.workspaceId,
    };
    const location = {
      protocol: 'https:',
      hostname: 'example.com',
      href: mark === 'both' || mark === 'url-only'
        ? 'https://example.com/?thread=abc&_boot_retry=1'
        : 'https://example.com/?thread=abc',
      replace: vi.fn(),
      reload: vi.fn(),
    };
    const history = { state: null, replaceState: vi.fn() };
    const listeners = new Map<string, Set<(e: unknown) => void>>();
    const fakeWindow: Record<string, unknown> = {
      setTimeout: window.setTimeout.bind(window),
      clearTimeout: window.clearTimeout.bind(window),
      addEventListener: (type: string, fn: (e: unknown) => void) => {
        if (!listeners.has(type)) listeners.set(type, new Set());
        listeners.get(type)!.add(fn);
      },
      removeEventListener: (type: string, fn: (e: unknown) => void) => {
        listeners.get(type)?.delete(fn);
      },
    };
    const fakeDocument = {
      querySelector: (selector: string) => {
        if (selector === 'base') {
          return doc.base == null ? null : { getAttribute: () => doc.base };
        }
        const meta = selector.match(/^meta\[name="(.+)"\]$/);
        if (meta) {
          const content = metas[meta[1]];
          return content == null ? null : { getAttribute: () => content };
        }
        return splash;
      },
    };
    new Function(
      'window', 'document', 'sessionStorage', 'location', 'history', 'URL', 'caches', 'navigator', source,
    )(
      fakeWindow, fakeDocument, storage, location, history, URL, doc.caches,
      { onLine: doc.online ?? true },
    );
    const fireError = (target: unknown) => {
      for (const fn of listeners.get('error') ?? []) fn({ target });
    };
    return {
      escape,
      splash,
      fakeWindow,
      fireError,
      fireEntryModuleError: () => fireError({ tagName: 'SCRIPT', type: 'module' }),
      history,
      location,
      splashClasses,
      status,
      statusClasses,
      storage,
      click: () => click?.(),
    };
  }

  it('ships the splash node so it paints before the JS bundle loads', () => {
    const splashIdx = html.indexOf('class="boot-splash"');
    const moduleIdx = html.indexOf('<script type="module"');
    expect(splashIdx).toBeGreaterThan(-1);
    expect(moduleIdx).toBeGreaterThan(-1);
    expect(splashIdx).toBeLessThan(moduleIdx);
  });

  it('carries the status line and the brand mark inline', () => {
    expect(html).toContain('boot-splash-status');
    expect(html).toContain('class="boot-splash-mark"');
    expect(html).toContain('pointer-events: none');
  });

  it('paints the brand gradient on both canvas layers so the iOS bottom safe-area strip is covered', () => {
    expect(bootHostSrc).toMatch(
      /SPLASH_BACKGROUND\s*=\s*\n?\s*['"]#145eb9 radial-gradient\([^'"]*\) no-repeat fixed['"]/,
    );
    expect(bootHostSrc).toMatch(/documentElement\.style\.background = SPLASH_BACKGROUND/);
    expect(html).toMatch(
      /<body style="background:#145eb9 radial-gradient\([^";]*\) no-repeat fixed">/,
    );
  });

  it('bases the canvas on the gradient colour AT the seam, not on its end stop', () => {
    const BASE = '#145eb9';
    const END_STOP = '#0a4ea8';
    const stops = ['#2d83e0', END_STOP];
    const [start, end] = [
      [0x2d, 0x83, 0xe0],
      [0x0a, 0x4e, 0xa8],
    ];
    const at = (p: number) =>
      start.map((s, i) => Math.round(s + (end[i] - s) * p));
    const base = [1, 3, 5].map(i => parseInt(BASE.slice(i, i + 2), 16));

    for (const progress of [0.624, 0.669, 0.7, 0.75, 0.8385]) {
      at(progress).forEach((channel, i) => {
        expect(Math.abs(channel - base[i])).toBeLessThanOrEqual(10);
      });
    }
    for (const stop of stops) expect(html).toContain(stop);
  });

  it('owns boot from the inline document and bounds a missing-module hang', () => {
    const watchdogIdx = html.indexOf('lucidos-boot-watchdog-start');
    const moduleIdx = html.indexOf('<script type="module"');
    expect(watchdogIdx).toBeGreaterThan(-1);
    expect(watchdogIdx).toBeLessThan(moduleIdx);
    expect(html).toContain('__lucidosBootLoaded');
    expect(html).toContain('lucidos-boot-retry');
    expect(html).toContain('Tap to retry');
    expect(html).toContain('window.setTimeout(recover, 15000)');
  });

  it('reloads once with a cache-busting query when the module never takes ownership', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog();
      vi.advanceTimersByTime(15_000);
      expect(watchdog.storage.getItem('lucidos-boot-retry')).toBe('1');
      expect(watchdog.location.replace).toHaveBeenCalledWith(
        'https://example.com/?thread=abc&_boot_retry=1',
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it('stops retry-looping and offers a tap after the guarded reload also fails', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog(true);
      vi.advanceTimersByTime(15_000);
      expect(watchdog.location.replace).not.toHaveBeenCalled();
      expect(watchdog.status.textContent).toBe('Tap to retry');
      expect(watchdog.splashClasses.has('boot-splash-stalled')).toBe(true);
      watchdog.click();
      expect(watchdog.storage.getItem('lucidos-boot-retry')).toBe(null);
      expect(watchdog.location.reload).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  const SHELL_CACHE_ABC = `${SHELL_CACHE_PREFIX}abc`;
  function fakeCaches(names: string[], opts: { hang?: boolean } = {}) {
    const deleted: string[] = [];
    return {
      deleted,
      api: {
        keys: () => (opts.hang ? new Promise<string[]>(() => {}) : Promise.resolve(names)),
        delete: (name: string) => { deleted.push(name); return Promise.resolve(true); },
      },
    };
  }

  it('drops the bundle caches, and only those, before its automatic retry', async () => {
    vi.useFakeTimers();
    try {
      const cache = fakeCaches([SHELL_CACHE_ABC, 'lucidos-blob-v1']);
      const watchdog = runInlineWatchdog(false, { caches: cache.api });
      watchdog.fireEntryModuleError();
      expect(watchdog.location.replace).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(0);
      expect(cache.deleted).toEqual([SHELL_CACHE_ABC]);
      expect(watchdog.location.replace).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it('drops the bundle caches before a tap reloads', async () => {
    vi.useFakeTimers();
    try {
      const cache = fakeCaches([SHELL_CACHE_ABC]);
      const watchdog = runInlineWatchdog(true, { caches: cache.api });
      watchdog.fireEntryModuleError();
      watchdog.click();
      expect(watchdog.location.reload).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(0);
      expect(cache.deleted).toEqual([SHELL_CACHE_ABC]);
      expect(watchdog.location.reload).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps the bundle caches when the browser reports offline', async () => {
    vi.useFakeTimers();
    try {
      const cache = fakeCaches([SHELL_CACHE_ABC]);
      const watchdog = runInlineWatchdog(true, { caches: cache.api, online: false });
      watchdog.fireEntryModuleError();
      watchdog.click();
      expect(watchdog.location.reload).toHaveBeenCalledOnce();
      await vi.advanceTimersByTimeAsync(0);
      expect(cache.deleted).toEqual([]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('drops its automatic retry when the module takes boot over during the eviction', async () => {
    vi.useFakeTimers();
    try {
      const cache = fakeCaches([SHELL_CACHE_ABC], { hang: true });
      const watchdog = runInlineWatchdog(false, { caches: cache.api });
      vi.advanceTimersByTime(15_000);
      (watchdog.fakeWindow.__lucidosBootLoaded as () => void)();
      await vi.advanceTimersByTimeAsync(5_000);
      expect(watchdog.location.replace).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('still reloads, once, when the Cache API hangs', async () => {
    vi.useFakeTimers();
    try {
      const cache = fakeCaches([], { hang: true });
      const watchdog = runInlineWatchdog(true, { caches: cache.api });
      watchdog.fireEntryModuleError();
      watchdog.click();
      await vi.advanceTimersByTimeAsync(1_999);
      expect(watchdog.location.reload).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      expect(watchdog.location.reload).toHaveBeenCalledOnce();
      await vi.advanceTimersByTimeAsync(10_000);
      expect(watchdog.location.reload).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it('names the same bundle cache prefix as the service worker', () => {
    const sw = readFileSync(resolve(__dirname, '../../public/sw.js'), 'utf-8');
    const swPrefix = sw.match(/const SHELL_CACHE = '([^']+)' \+ BUILD_ID/)?.[1];
    expect(swPrefix).toBe(SHELL_CACHE_PREFIX);
    expect(html).toContain(`var SHELL_CACHE_PREFIX = '${swPrefix}';`);
  });

  it('recovers the moment the entry module reports it cannot load, not 15s later', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog();
      watchdog.fireEntryModuleError();
      expect(watchdog.location.replace).toHaveBeenCalledWith(
        'https://example.com/?thread=abc&_boot_retry=1',
      );
      vi.advanceTimersByTime(15_000);
      expect(watchdog.location.replace).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it('goes straight to the tap action when the retried document also fails to load', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog(true);
      watchdog.fireEntryModuleError();
      expect(watchdog.location.replace).not.toHaveBeenCalled();
      expect(watchdog.status.textContent).toBe('Tap to retry');
      expect(watchdog.splashClasses.has('boot-splash-stalled')).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('ignores errors that are not the entry module failing to load', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog();
      watchdog.fireError(undefined);
      watchdog.fireError({ tagName: 'IMG' });
      watchdog.fireError({ tagName: 'SCRIPT', type: 'text/javascript' });
      expect(watchdog.location.replace).not.toHaveBeenCalled();
      vi.advanceTimersByTime(15_000);
      expect(watchdog.location.replace).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it('spends its one retry on the URL mark alone, so a storage-less browser cannot loop', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog('url-only');
      vi.advanceTimersByTime(15_000);
      expect(watchdog.location.replace).not.toHaveBeenCalled();
      expect(watchdog.status.textContent).toBe('Tap to retry');
    } finally {
      vi.useRealTimers();
    }
  });

  it('stops boot ownership from being taken back by a later module error', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog();
      const loaded = watchdog.fakeWindow.__lucidosBootLoaded as () => void;
      loaded();
      watchdog.fireEntryModuleError();
      vi.advanceTimersByTime(15_000);
      expect(watchdog.location.replace).not.toHaveBeenCalled();
      expect(watchdog.status.textContent).toBe('Opening your workspace…');
    } finally {
      vi.useRealTimers();
    }
  });

  const GATEWAY = { gatewayPort: '5251' };

  it('sends a stopped direct-port workspace to itself on the gateway, which starts it', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog(true, { ...GATEWAY, workspaceId: 'myws' });
      watchdog.fireEntryModuleError();
      expect(watchdog.escape.href).toBe('https://example.com:5251/myws/');
      expect(watchdog.escape.hidden).toBe(false);
      expect(watchdog.escape.textContent).toBe('Start this workspace');
      expect(watchdog.status.textContent).toBe("Can't reach this workspace");
      expect(watchdog.splash.setAttribute).toHaveBeenCalledWith('aria-hidden', 'false');
    } finally {
      vi.useRealTimers();
    }
  });

  it('falls back to the workspace list when the shell predates the id stamp', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog(true, GATEWAY);
      watchdog.fireEntryModuleError();
      expect(watchdog.escape.href).toBe('https://example.com:5251/~/?pick');
      expect(watchdog.escape.textContent).toBe('Back to workspaces');
    } finally {
      vi.useRealTimers();
    }
  });

  it('leaves exactly one tap target: the escape replaces tap-to-retry', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog(true, { ...GATEWAY, workspaceId: 'myws' });
      watchdog.fireEntryModuleError();
      expect(watchdog.splashClasses.has('boot-splash-stalled')).toBe(false);
      watchdog.click();
      expect(watchdog.location.reload).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('offers nothing to escape to when there is no gateway', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog(true);
      watchdog.fireEntryModuleError();
      expect(watchdog.escape.hidden).toBe(true);
      expect(watchdog.status.textContent).toBe('Tap to retry');
      expect(watchdog.splashClasses.has('boot-splash-stalled')).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('offers no escape from a document already behind the gateway', () => {
    vi.useFakeTimers();
    try {
      for (const base of ['/myws/', '/~/']) {
        const watchdog = runInlineWatchdog(true, { ...GATEWAY, workspaceId: 'myws', base });
        watchdog.fireEntryModuleError();
        expect(watchdog.escape.hidden).toBe(true);
        expect(watchdog.status.textContent).toBe('Tap to retry');
      }
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps the escape out of a healthy boot entirely', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog(false, { ...GATEWAY, workspaceId: 'myws' });
      const loaded = watchdog.fakeWindow.__lucidosBootLoaded as () => void;
      loaded();
      vi.advanceTimersByTime(15_000);
      expect(watchdog.escape.hidden).toBe(true);
      expect(watchdog.escape.href).toBe('');
    } finally {
      vi.useRealTimers();
    }
  });

  it('exposes the reveal so the application can offer the same escape', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog(false, { ...GATEWAY, workspaceId: 'myws' });
      const loaded = watchdog.fakeWindow.__lucidosBootLoaded as () => void;
      loaded();
      const reveal = watchdog.fakeWindow.__lucidosGatewayEscape as () => unknown;
      expect(typeof reveal).toBe('function');
      expect(reveal()).not.toBeNull();
      expect(watchdog.escape.href).toBe('https://example.com:5251/myws/');
      expect(watchdog.escape.hidden).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('cancels recovery and cleans its retry query when the module loads', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog(true);
      const loaded = watchdog.fakeWindow.__lucidosBootLoaded as () => void;
      loaded();
      vi.advanceTimersByTime(15_000);
      expect(watchdog.location.replace).not.toHaveBeenCalled();
      expect(watchdog.history.replaceState).toHaveBeenCalledWith(
        null,
        '',
        'https://example.com/?thread=abc',
      );
      expect(watchdog.storage.getItem('lucidos-boot-retry')).toBe(null);
    } finally {
      vi.useRealTimers();
    }
  });

  it('bakes a default, shown status so it never vanishes across the reload', () => {
    expect(html).toContain('boot-splash-status-shown');
    expect(html).toMatch(/boot-splash-status[^>]*>[^<]*\S[^<]*<\/div>/);
  });

  it('hangs the status and the escape link below the mark, out of flow', () => {
    expect(html).toMatch(/\.boot-splash-foot\s*\{[^}]*position:\s*absolute/);
    expect(html).toMatch(
      /\.boot-splash-foot\s*\{[^}]*top:\s*calc\(50% \+ var\(--boot-mark-size\) \/ 2 \+ var\(--boot-splash-gap\)\)/,
    );
    const foot = html.split('<div class="boot-splash-foot">')[1]?.split('</div>\n    </div>')[0];
    expect(foot).toContain('class="boot-splash-status');
    expect(foot).toContain('class="boot-splash-escape"');
    expect(html).toMatch(
      /:root\[data-boot-splash-quiet\]\s+\.boot-splash-foot\s*\{\s*position:\s*static/,
    );
    const recentre = /((?:[^{}]*\.boot-splash-foot,?\s*)+)\{\s*position:\s*static;\s*\}/g;
    const selectors = [...html.matchAll(recentre)].map((m) => m[1]).join(' ');
    expect(selectors).toContain('.boot-splash:has(.boot-splash-status-report) .boot-splash-foot');
    expect(selectors).toContain('.boot-splash:has(.boot-splash-escape:not([hidden])) .boot-splash-foot');
    expect(html).toMatch(
      /@media \(max-height: 520px\) \{\s*\.boot-splash-foot\s*\{\s*position:\s*static;\s*\}/,
    );
  });

  it('fades a status change out before the words swap, on both surfaces', () => {
    const swap = /\.boot-splash-status\.boot-splash-status-swap\s*\{([^}]*)\}/.exec(html)?.[1];
    expect(swap).toBeTruthy();
    expect(swap).toMatch(/opacity:\s*0/);
    const swapSeconds = String(STATUS_SWAP_MS / 1000).replace('.', '\\.');
    expect(swap).toMatch(
      new RegExp(`transition-duration:\\s*calc\\(${swapSeconds}s \\* var\\(--duration-scale, 1\\)\\)`),
    );
    expect(html.indexOf('.boot-splash-status.boot-splash-status-swap')).toBeGreaterThan(
      html.indexOf('.boot-splash-status.boot-splash-status-shown'),
    );
  });

  it('keeps the status box one fixed width, centring the words inside it', () => {
    const status = /\n {6}\.boot-splash-status \{([^}]*)\}/.exec(html)?.[1];
    expect(status).toBeTruthy();
    expect(status).toMatch(/(^|\s)width:\s*80vw;/);
    expect(status).toMatch(/text-align:\s*center/);
  });

  it('reserves a constant status size so the escape link never shifts', () => {
    expect(html).toMatch(/\.boot-splash-status\s*\{[^}]*height:\s*1\.4em/);
    expect(html).toMatch(/\.boot-splash-status\s*\{[^}]*white-space:\s*nowrap/);
  });

  it('pins the splash geometry in px so it cannot ride the UI scale', () => {
    expect(html).toMatch(
      /\.boot-splash\s*\{[^}]*--boot-mark-size:\s*min\(46vmin,\s*240px\)/,
    );
    expect(html).toMatch(/\.boot-splash\s*\{[^}]*--boot-splash-gap:\s*24px/);
    expect(html).toMatch(/\.boot-splash-mark\s*\{[^}]*width:\s*var\(--boot-mark-size\)/);
    expect(html).toMatch(/\.boot-splash-mark\s*\{[^}]*height:\s*var\(--boot-mark-size\)/);
    expect(html).toMatch(/\.boot-splash\s*\{[^}]*font-size:\s*15px/);
    expect(html).toMatch(/\.boot-splash\s*\{[^}]*font-family:\s*ui-monospace/);
    expect(html).not.toMatch(/\.boot-splash[^{]*\{[^}]*font-family:\s*var\(--font-ui\)/);
    const splashCss = (html.match(/<style>([\s\S]*?)<\/style>/)?.[1] ?? '').replace(
      /\/\*[\s\S]*?\*\//g,
      '',
    );
    expect(splashCss).toContain('.boot-splash');
    expect(splashCss).not.toMatch(/\d\s*rem/);
  });

  it('smooths the splash type exactly as the app shell does', () => {
    const baseCss = readFileSync(resolve(__dirname, '../styles/global/base.css'), 'utf-8');
    const shell = /^body \{([^}]*)\}/m.exec(baseCss)?.[1];
    expect(shell, 'the body rule in styles/global/base.css').toBeTruthy();
    const splash = /\.boot-splash \{([\s\S]*?)\n {6}\}/.exec(html)?.[1];
    expect(splash, 'the .boot-splash rule in index.html').toBeTruthy();

    const declared = (css: string, prop: string) =>
      new RegExp(`(?:^|;|\\*/)\\s*${prop}:\\s*([^;]+);`, 'm').exec(css)?.[1].trim();

    const smoothing = [
      ...shell!.matchAll(/(-webkit-font-smoothing|-moz-osx-font-smoothing|text-rendering):/g),
    ].map((m) => m[1]);
    expect(smoothing.length, 'base.css body still declares font smoothing').toBeGreaterThan(0);
    for (const prop of smoothing) {
      expect(declared(splash!, prop), prop).toBe(declared(shell!, prop));
    }
  });

  it('normalizes an absolute base href in every hand-rolled key derivation', () => {
    const guards = html.match(/if\s*\(.*?\/\^https\?:\\\/\\\/\/i\.test\(/g) ?? [];
    expect(guards.length, 'one absolute-base guard per wsKey derivation').toBe(2);
    expect(html.match(/function wsKey\(/g)?.length).toBe(2);
    expect(html.match(/\(seg === '' \|\| seg === '~'\) \? null : seg/g)?.length).toBe(2);
  });

  it('keeps the markers the gateway splash lifts this stylesheet and mark out by', () => {
    for (const marker of [
      '/* lucidos-boot-splash-css-start */',
      '/* lucidos-boot-splash-css-end */',
      '<!-- lucidos-boot-splash-mark-start -->',
      '<!-- lucidos-boot-splash-mark-end -->',
    ]) {
      expect(html.split(marker).length - 1, marker).toBe(1);
    }
    const css = html.split('/* lucidos-boot-splash-css-start */')[1]?.split(
      '/* lucidos-boot-splash-css-end */',
    )[0];
    expect(css).toContain('.boot-splash-mark');
    expect(css).toContain('@keyframes boot-mark-reveal');
    const mark = html.split('<!-- lucidos-boot-splash-mark-start -->')[1]?.split(
      '<!-- lucidos-boot-splash-mark-end -->',
    )[0];
    expect(mark).toContain('<svg class="boot-splash-mark"');
    expect(mark).toContain('</svg>');
  });

  it('redirects to the last workspace in <head>, before the bundle, to skip the picker render', () => {
    const redirectIdx = html.indexOf('location.replace');
    const moduleIdx = html.indexOf('<script type="module"');
    expect(redirectIdx).toBeGreaterThan(-1);
    expect(redirectIdx).toBeLessThan(moduleIdx);
    expect(html).toContain("localStorage.getItem('lucidos-last-workspace')");
    expect(html).toContain("has('pick')");
    expect(html).toContain("baseHref === '/~/'");
  });

  describe('height pin', () => {
    function runPin() {
      const source = html
        .split('<script>')
        .find((block: string) => block.split('</script>')[0].includes('--boot-splash-height'))
        ?.split('</script>')[0];
      if (!source) throw new Error('inline splash height pin not found');
      const props = new Map<string, string>();
      const viewport = { width: 390, height: 810 };
      const splash = {
        isConnected: true,
        style: {
          setProperty: (name: string, value: string) => props.set(name, value),
          removeProperty: (name: string) => props.delete(name),
        },
        getBoundingClientRect: () => ({
          height: props.has('--boot-splash-height')
            ? parseFloat(props.get('--boot-splash-height')!)
            : viewport.height,
        }),
      };
      const listeners = new Set<() => void>();
      const win = {
        get innerWidth() { return viewport.width; },
        addEventListener: (_type: string, fn: () => void) => listeners.add(fn),
        removeEventListener: (_type: string, fn: () => void) => listeners.delete(fn),
      };
      new Function('document', 'window', source)({ querySelector: () => splash }, win);
      const resize = (width: number, height: number) => {
        viewport.width = width;
        viewport.height = height;
        for (const fn of [...listeners]) fn();
      };
      return { props, resize, splash, listeners };
    }

    it('pins the box to its first-paint height', () => {
      const { props } = runPin();
      expect(props.get('--boot-splash-height')).toBe('810px');
    });

    it('holds the pin when only the height changes', () => {
      const { props, resize } = runPin();
      resize(390, 844);
      expect(props.get('--boot-splash-height')).toBe('810px');
    });

    it('re-measures on a rotation, which changes the width', () => {
      const { props, resize } = runPin();
      resize(844, 390);
      expect(props.get('--boot-splash-height')).toBe('390px');
    });

    it('follows the height while a rotation settles, then holds again', () => {
      vi.useFakeTimers();
      try {
        const { props, resize } = runPin();
        resize(844, 420);
        resize(844, 390);
        expect(props.get('--boot-splash-height')).toBe('390px');
        vi.advanceTimersByTime(600);
        resize(844, 430);
        expect(props.get('--boot-splash-height')).toBe('390px');
      } finally {
        vi.useRealTimers();
      }
    });

    it('stops listening once the splash is gone', () => {
      const { resize, splash, listeners } = runPin();
      splash.isConnected = false;
      resize(844, 390);
      expect(listeners.size).toBe(0);
    });

    it('lays the box out against the pin, falling back to the viewport', () => {
      expect(html).toMatch(/\.boot-splash\s*\{[^}]*inset:\s*0 0 auto;/);
      expect(html).toMatch(/\.boot-splash\s*\{[^}]*height:\s*var\(--boot-splash-height,\s*100%\)/);
      expect(html).toMatch(/\.boot-splash\s*\{[^}]*max-height:\s*100%;/);
    });

    it('covers the whole viewport whatever the pin holds', () => {
      const cover = html.match(/\.boot-splash::before\s*\{([^}]*)\}/)?.[1] ?? '';
      expect(cover).toMatch(/position:\s*fixed;/);
      expect(cover).toMatch(/inset:\s*0;/);
      expect(cover).toMatch(/background:\s*radial-gradient\(125% 125% at 30% 22%/);
      expect(html).not.toMatch(/\.boot-splash\s*\{[^}]*background:/);
    });
  });

  describe('gateway handover (boot-splash-formed)', () => {
    function runHandover(flag: string | null) {
      const source = html
        .split('<script>')
        .find((block: string) => block.includes("removeItem('lucidos-splash-mark-formed')"))
        ?.split('</script>')[0];
      if (!source) throw new Error('inline splash handover script not found');
      const values = new Map<string, string>();
      if (flag !== null) values.set('lucidos-splash-mark-formed', flag);
      const storage = {
        getItem: (key: string) => values.get(key) ?? null,
        removeItem: (key: string) => values.delete(key),
      };
      const classes = new Set<string>();
      const splash = { classList: { add: (name: string) => classes.add(name) } };
      new Function('document', 'sessionStorage', source)(
        { querySelector: () => splash },
        storage,
      );
      return { classes, values };
    }

    it('skips the reveal when the gateway splash already built the mark', () => {
      const { classes } = runHandover('1');
      expect(classes.has('boot-splash-formed')).toBe(true);
    });

    it('consumes the flag so it can only ever suppress that one rebuild', () => {
      const { values } = runHandover('1');
      expect(values.has('lucidos-splash-mark-formed')).toBe(false);
    });

    it('plays the reveal normally when no gateway splash preceded this document', () => {
      const { classes } = runHandover(null);
      expect(classes.size).toBe(0);
    });

    it('settles straight into the breathe, and stays still under reduced motion', () => {
      expect(html).toMatch(
        /\.boot-splash-formed\s+\.boot-splash-mark\s*\{[^}]*animation:\s*boot-mark-breathe/,
      );
      expect(reducedMotionRules()).toMatch(
        /:root\[data-motion="reduce"\]\s+\.boot-splash-formed\s+\.boot-splash-mark[^{}]*\{\s*animation:\s*none/,
      );
    });
  });

  describe('notification tap (boot-splash-quiet)', () => {
    const blockWith = (needle: string) =>
      html
        .split('<script>')
        .find((block: string) => block.includes(needle))
        ?.split('</script>')[0];
    const source = blockWith("setAttribute('data-boot-splash-quiet'");
    const applySource = blockWith("classList.add('boot-splash-quiet')");

    function runQuietBoot(
      url: string,
      opts: {
        bgVar?: string;
        theme?: string;
        formed?: boolean;
        refreshed?: boolean;
        base?: string | null;
      } = {},
    ) {
      if (!source) throw new Error('inline quiet decision script not found');
      if (!applySource) throw new Error('inline quiet apply script not found');
      const parsed = new URL(url);
      const GRADIENT = '#145eb9 radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%) no-repeat fixed';
      const classes = new Set<string>();
      const statusClasses = new Set<string>(['boot-splash-status-shown']);
      const status = {
        textContent: 'Opening your workspace…',
        classList: { remove: (name: string) => statusClasses.delete(name) },
      };
      const splash = {
        classList: {
          add: (name: string) => classes.add(name),
          contains: (name: string) => classes.has(name),
        },
        querySelector: () => status,
      };
      const attributes = new Map<string, string>([['data-theme-mode', opts.theme ?? 'dark']]);
      const documentElement = {
        style: {
          background: GRADIENT,
          getPropertyValue: (key: string) =>
            key === '--bg-primary' ? (opts.bgVar ?? '#07172e') : '',
        },
        getAttribute: (key: string) => attributes.get(key) ?? null,
        setAttribute: (key: string, value: string) => attributes.set(key, value),
        hasAttribute: (key: string) => attributes.has(key),
      };
      const body = { style: { background: GRADIENT } };
      const basePath = opts.base == null ? '' : normalizeBasePath(opts.base);
      const slug = basePath === '' || basePath === '/~' ? null : basePath.slice(1);
      const flagKey = slug ? `ws:${slug}:lucidos-splash-quiet` : 'lucidos-splash-quiet';
      const HANDOVER_KEY = 'lucidos-splash-mark-formed';
      const stored = new Map<string, string>();
      if (opts.refreshed) stored.set(flagKey, '1');
      if (opts.formed) stored.set(HANDOVER_KEY, '1');
      const sessionStorage = {
        getItem: (key: string) => stored.get(key) ?? null,
        removeItem: (key: string) => stored.delete(key),
      };
      const base = opts.base == null ? null : { getAttribute: () => opts.base };
      new Function('document', 'location', 'sessionStorage', source)(
        { querySelector: (sel: string) => (sel === 'base' ? base : null), documentElement },
        { search: parsed.search, hash: parsed.hash },
        sessionStorage,
      );
      const handoverFlagLeft = stored.has(HANDOVER_KEY);
      new Function('document', applySource)({ querySelector: () => splash, documentElement, body });
      const quiet = attributes.has('data-boot-splash-quiet');
      expect(classes.has('boot-splash-quiet'), url).toBe(quiet);
      return {
        quiet,
        handoverFlagLeft,
        status,
        statusShown: () => statusClasses.has('boot-splash-status-shown'),
        rootBackground: () => documentElement.style.background,
        bodyBackground: () => body.style.background,
        flagLeft: () => stored.has(flagKey),
        leftoverKeys: () => [...stored.keys()].filter((key) => key !== HANDOVER_KEY),
        gradient: GRADIENT,
      };
    }

    it('decides in <head>, before the splash markup can paint', () => {
      const head = html.split('</head>')[0];
      expect(source && head.includes(source)).toBe(true);
      expect(html).toMatch(
        /:root\[data-boot-splash-quiet\]\s+\.boot-splash::before\s*\{[^}]*background:\s*var\(--bg-primary/,
      );
      expect(html).toMatch(/:root\[data-boot-splash-quiet\]\s+\.boot-splash-mark\s*\{[^}]*display:\s*none/);
      expect(html).toMatch(
        /:root\[data-boot-splash-quiet\]\s+\.boot-splash:not\(\.boot-splash-quiet\)\s+\.boot-splash-status\s*\{[^}]*visibility:\s*hidden/,
      );
    });

    it('quiets the splash and flattens both canvas layers on a tap', () => {
      const boot = runQuietBoot('https://host/myws/?notification=n1&thread=t1&tap=%7B%7D');
      expect(boot.quiet).toBe(true);
      expect(boot.status.textContent).toBe('');
      expect(boot.statusShown()).toBe(false);
      expect(boot.rootBackground()).toBe('#07172e');
      expect(boot.bodyBackground()).toBe('#07172e');
    });

    it('reads the deep link out of the hash as well as the query', () => {
      expect(runQuietBoot('https://host/myws/#notification=n1&thread=t1').quiet).toBe(true);
    });

    it('quiets a user-requested refresh, which carries no deep link at all', () => {
      const boot = runQuietBoot('https://host/myws/', { refreshed: true });
      expect(boot.quiet).toBe(true);
      expect(boot.status.textContent).toBe('');
      expect(boot.rootBackground()).toBe('#07172e');
      expect(boot.bodyBackground()).toBe('#07172e');
    });

    it('consumes the refresh flag as it reads it', () => {
      expect(runQuietBoot('https://host/myws/', { refreshed: true }).flagLeft()).toBe(false);
      expect(runQuietBoot('https://host/myws/').quiet).toBe(false);
    });

    it('reads the flag under the SAME per-workspace key the app writes', () => {
      const boot = runQuietBoot('https://host/myws/', { refreshed: true, base: '/myws/' });
      expect(boot.quiet).toBe(true);
      expect(boot.leftoverKeys()).toEqual([]);
      for (const base of ['/~/', '/', null]) {
        expect(runQuietBoot('https://host/', { refreshed: true, base }).quiet, String(base))
          .toBe(true);
      }
      const abs = runQuietBoot('https://host/myws/', {
        refreshed: true,
        base: 'https://host/myws/',
      });
      expect(abs.quiet).toBe(true);
      expect(abs.leftoverKeys()).toEqual([]);
    });

    it('consumes the flag even on a URL the deep-link gate would return early on', () => {
      const boot = runQuietBoot(
        'https://host/myws/#thread=1e6a2f14-0000-4000-8000-000000000000',
        { refreshed: true },
      );
      expect(boot.flagLeft()).toBe(false);
      expect(boot.quiet).toBe(true);
    });

    const routerDispatchesDeepLink = (url: string) =>
      !THREAD_HASH_RE.test(new URL(url).hash) &&
      hasDeepLinkParams(parseDeepLinkFromUrl(new URL(url)));

    it('goes quiet exactly when the page-side router would dispatch a deep link', () => {
      for (const url of [
        'https://host/myws/?notification=n1',
        'https://host/myws/?notification=n1&thread=t1&event=e1&tap=%7B%22kind%22%3A%22modal%22%7D',
        'https://host/myws/#notification=n1&thread=t1',
        'https://host/myws/?thread=t1&event=e1',
        'https://host/myws/?notification=',
        'https://host/myws/#notification=',
        'https://host/myws/?notification=n1#notification=',
        'https://host/myws/?notification=#notification=n1',
        'https://host/myws/#thread=1e6a2f14-0000-4000-8000-000000000000',
        'https://host/myws/?notification=n1#thread=1e6a2f14-0000-4000-8000-000000000000',
        'https://host/myws/',
        'https://host/myws/?_boot_retry=1',
      ]) {
        expect(runQuietBoot(url).quiet, url).toBe(routerDispatchesDeepLink(url));
      }
    });

    it('stands down when the gateway handed over a mark that is already standing', () => {
      for (const opts of [
        { formed: true },
        { formed: true, refreshed: true },
      ]) {
        const boot = runQuietBoot('https://host/myws/?notification=n1&thread=t1', opts);
        expect(boot.quiet, JSON.stringify(opts)).toBe(false);
        expect(boot.rootBackground()).toBe(boot.gradient);
        expect(boot.bodyBackground()).toBe(boot.gradient);
        expect(boot.statusShown()).toBe(true);
        expect(boot.flagLeft()).toBe(false);
        expect(boot.handoverFlagLeft).toBe(true);
      }
    });

    it('leaves a launch document completely alone', () => {
      const boot = runQuietBoot('https://host/myws/');
      expect(boot.quiet).toBe(false);
      expect(boot.status.textContent).toBe('Opening your workspace…');
      expect(boot.statusShown()).toBe(true);
      expect(boot.rootBackground()).toBe(boot.gradient);
      expect(boot.bodyBackground()).toBe(boot.gradient);
    });

    it('follows the resolved theme, and falls back to it when the FOUC script could not run', () => {
      expect(runQuietBoot('https://host/?notification=n1', { bgVar: '#ffffff' }).rootBackground())
        .toBe('#ffffff');
      expect(
        runQuietBoot('https://host/?notification=n1', { bgVar: '', theme: 'light' }).rootBackground(),
      ).toBe('#ffffff');
      expect(
        runQuietBoot('https://host/?notification=n1', { bgVar: '', theme: 'dark' }).rootBackground(),
      ).toBe('#07172e');
    });

    it('reads the URL and never writes it', () => {
      expect(source).toBeTruthy();
      expect(source).not.toMatch(/location\.(replace|assign|reload)|location\.href\s*=|history\./);
    });

    it('leaves faster than a launch splash', () => {
      expect(html).toMatch(
        /:root\[data-boot-splash-quiet\]\s+\.boot-splash-leaving\s*\{[^}]*animation-duration:\s*calc\(0\.2s \* var\(--duration-scale, 1\)\)/,
      );
    });

    it('keeps the status and the escape legible on a light-theme quiet cover', () => {
      expect(html).toMatch(
        /:root\[data-theme-mode="light"\]\[data-boot-splash-quiet\]\s+\.boot-splash-status\s*\{[^}]*color:/,
      );
      expect(html).toMatch(
        /:root\[data-theme-mode="light"\]\[data-boot-splash-quiet\]\s+\.boot-splash-escape\s*\{[^}]*color:/,
      );
    });

    it('does not let the quiet fade outrank reduced motion', () => {
      expect(reducedMotionRules()).toMatch(
        /:root\[data-motion="reduce"\]\s+\.boot-splash-leaving,\s*:root\[data-motion="reduce"\]\[data-boot-splash-quiet\]\s+\.boot-splash-leaving\s*\{[^}]*animation-duration:\s*0\.15s/,
      );
    });
  });

  it('plays the mark reveal in the final doc but hides the mark in the picker', () => {
    expect(html).toContain('@keyframes boot-mark-reveal');
    expect(html).toMatch(/\.boot-splash-mark\s*\{[^}]*animation:\s*boot-mark-reveal/);
    expect(html).toMatch(/\.boot-splash-reload\s+\.boot-splash-mark\s*\{[^}]*visibility:\s*hidden/);
    expect(html).toContain("getAttribute('href') !== '/~/'");
  });

  describe('leaving choreography', () => {
    it('lets the status and the mark leave inside the veil fade', () => {
      const veil = /\.boot-splash-leaving\s*\{[^}]*animation:\s*boot-splash-out\s+calc\(([\d.]+)s/.exec(html);
      expect(veil).toBeTruthy();
      const veilMs = parseFloat(veil![1]) * 1000;
      const status = /\.boot-splash-leaving\s+\.boot-splash-status\s*\{[^}]*opacity:\s*0;[^}]*transition:\s*opacity\s+calc\(([\d.]+)s/.exec(html);
      expect(status).toBeTruthy();
      expect(html).toContain('@keyframes boot-mark-out');
      const mark = /\.boot-splash-leaving\s+\.boot-splash-mark\s*\{[^}]*boot-mark-out\s+calc\(([\d.]+)s/.exec(html);
      expect(mark).toBeTruthy();
      expect(parseFloat(mark![1]) * 1000).toBeLessThan(veilMs);
      expect(parseFloat(status![1]) * 1000).toBeLessThan(veilMs);
    });

    it('appends the exit to each mark state list instead of replacing it', () => {
      const leaving = /\.boot-splash-leaving\s+\.boot-splash-mark\s*\{([^}]*)\}/.exec(html)?.[1];
      expect(leaving).toBeTruthy();
      expect(leaving).toMatch(/boot-mark-reveal[\s\S]*boot-mark-breathe[\s\S]*boot-mark-out/);
      const formed = /\.boot-splash-formed\.boot-splash-leaving\s+\.boot-splash-mark\s*\{([^}]*)\}/.exec(html)?.[1];
      expect(formed).toBeTruthy();
      expect(formed).toMatch(/boot-mark-breathe[\s\S]*boot-mark-out/);
      expect(formed).not.toMatch(/boot-mark-reveal/);
    });

    it('silences the exit under reduced motion', () => {
      const reduced = reducedMotionRules();
      expect(reduced).toMatch(/\.boot-splash-leaving\s+\.boot-splash-mark[^{}]*\{\s*animation:\s*none/);
      expect(reduced).toMatch(/\.boot-splash-formed\.boot-splash-leaving\s+\.boot-splash-mark[^{}]*\{\s*animation:\s*none/);
      expect(reduced).toMatch(/\.boot-splash-leaving\s+\.boot-splash-status[^{}]*\{\s*transition:\s*none/);
    });

    it('stops taking pointer input once it is leaving', () => {
      expect(html).toMatch(
        /\.boot-splash\.boot-splash-leaving,\s*\.boot-splash-leaving\s+\.boot-splash-escape,\s*\.boot-splash-leaving\s+\.boot-splash-action\s*\{[^}]*pointer-events:\s*none/,
      );
    });
  });
});
