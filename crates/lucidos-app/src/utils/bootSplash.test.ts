import { describe, it, expect, vi, afterEach } from 'vitest';
import {
  hasDeepLinkParams,
  parseDeepLinkFromUrl,
} from '../store/actions/notification-deeplink';
import { THREAD_HASH_RE } from '../store/actions/cross-workspace';
import { normalizeBasePath } from './basePath';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

// ── Fake DOM ────────────────────────────────────────────────────────────────
// The controller manipulates the inline splash node (a sibling of #app, owned by
// no Preact tree). The test-setup stub document returns null for querySelector,
// so we install a richer fake just for these tests.

function installFakeSplash(present: boolean, initialClasses: string[] = []) {
  const statusClasses = new Set<string>();
  const statusEl = {
    textContent: '',
    classList: { toggle: (c: string, on: boolean) => { on ? statusClasses.add(c) : statusClasses.delete(c); } },
  };
  const listeners: Record<string, Array<() => void>> = {};
  let removed = false;
  const classes = new Set<string>(initialClasses);
  const splashEl = {
    classList: { add: (c: string) => classes.add(c), contains: (c: string) => classes.has(c) },
    addEventListener: (type: string, fn: () => void) => { (listeners[type] ??= []).push(fn); },
    remove: () => { removed = true; },
    fire: (type: string) => { for (const fn of listeners[type] ?? []) fn(); },
  };
  const prev = (globalThis as any).document.querySelector;
  (globalThis as any).document.querySelector = (sel: string) => {
    if (!present) return null;
    if (sel === '.boot-splash') return splashEl;
    if (sel.includes('.boot-splash-status')) return statusEl;
    // Compound state selectors (`.boot-splash.boot-splash-quiet`) resolve only
    // when the document actually carries that class, which is how the
    // controller's state predicates read the inline script's decision.
    const state = /^\.boot-splash\.(boot-splash-[a-z-]+)$/.exec(sel);
    if (state) return classes.has(state[1]) ? splashEl : null;
    return null;
  };
  return {
    statusEl,
    statusShown: () => statusClasses.has('boot-splash-status-shown'),
    hasLeaving: () => classes.has('boot-splash-leaving'),
    isRemoved: () => removed,
    fireAnimationEnd: () => splashEl.fire('animationend'),
    restore: () => { (globalThis as any).document.querySelector = prev; },
  };
}

// Fresh module per test so the internal `dismissed` latch doesn't leak across
// cases.
async function freshController() {
  vi.resetModules();
  return import('./bootSplash');
}

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

  it('dismiss adds the leaving class and removes the node on animationend', async () => {
    fake = installFakeSplash(true);
    const c = await freshController();
    c.dismissBootSplash();
    expect(fake.hasLeaving()).toBe(true);
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
      expect(fake.isRemoved()).toBe(false);
      vi.advanceTimersByTime(600);
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
    // Second call must not throw or re-remove.
    expect(() => c.dismissBootSplash()).not.toThrow();
  });

  // The application faces the same dead end as the pre-hydration document when
  // its bundle loaded but the engine is unreachable, so it calls through to the
  // inline document's reveal instead of rebuilding the href rule in TS.
  it('reveals the escape through the inline document, and reports what happened', async () => {
    fake = installFakeSplash(true);
    const c = await freshController();
    const win = globalThis as unknown as { __lucidosGatewayEscape?: () => unknown };

    // No hook: this document is behind the gateway, has no gateway to reach, or
    // the splash is already gone. Callers stay indifferent to which.
    expect(c.revealBootEscape()).toBe(false);

    // Hook present but nothing to offer.
    win.__lucidosGatewayEscape = () => null;
    expect(c.revealBootEscape()).toBe(false);

    win.__lucidosGatewayEscape = () => ({ href: 'https://example.com:5251/myws/' });
    expect(c.revealBootEscape()).toBe(true);
    delete win.__lucidosGatewayEscape;
  });

  it('dismiss reverts the boot background only AFTER the splash node is removed (not mid-fade)', async () => {
    fake = installFakeSplash(true);
    const doc = (globalThis as any).document;
    // Boot script (index.html) paints the brand gradient on <html> to cover the
    // iOS bottom safe-area strip; dismiss must revert it so no blue lingers
    // behind the app's own safe-area inset — but only once the splash is gone.
    doc.documentElement.style.background =
      '#145eb9 radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%) no-repeat fixed';
    const c = await freshController();
    c.dismissBootSplash();
    // Still painted through the `.boot-splash-leaving` fade — reverting now would
    // flash the dark safe-area strip while the splash is still visible.
    expect(doc.documentElement.style.background).not.toBe('');
    fake.fireAnimationEnd();
    // Reverted once the splash node is actually removed.
    expect(doc.documentElement.style.background).toBe('');
  });

  // The one thing a quiet cover or a gateway handover changes for the controller:
  // there is no reveal to hold a floor for. This is the predicate the hook calls,
  // so both no-reveal documents and the launch case are pinned here against the
  // real class names the inline scripts set.
  it('reports both no-reveal documents, and a plain launch as playing one', async () => {
    for (const cls of ['boot-splash-quiet', 'boot-splash-formed']) {
      fake?.restore();
      fake = installFakeSplash(true, [cls]);
      const c = await freshController();
      expect(c.bootSplashPlaysNoReveal(), cls).toBe(true);
    }

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

  /** The two independent marks a retry leaves: sessionStorage, and the URL
   *  param. Either one must spend the single attempt, so a browser that refuses
   *  storage cannot reload-loop. */
  type RetryMark = 'none' | 'both' | 'url-only' | 'storage-only';

  /** The document context the watchdog reads. Defaults describe a DIRECT engine
   *  port with no gateway: no `<base>` (the engine stamps none for `/`) and no
   *  metas, which is also the legacy no-gateway engine. */
  interface DocContext {
    /** `<base href>`: null = direct port, `/~/` = picker, `/<slug>/` = gateway. */
    base?: string | null;
    gatewayPort?: string | null;
    workspaceId?: string | null;
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
    // The escape anchor ships hidden with no href; the watchdog fills both in
    // only when this document has somewhere to send the user.
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
    // Capture-phase window listeners, so a test can fire the entry module's
    // load failure the way the browser does.
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
    new Function('window', 'document', 'sessionStorage', 'location', 'history', 'URL', source)(
      fakeWindow, fakeDocument, storage, location, history, URL,
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
    // The splash must come before the module script so first paint is the brand,
    // not an empty #app, regardless of connection speed.
    expect(splashIdx).toBeLessThan(moduleIdx);
  });

  it('carries the status line and the brand mark inline', () => {
    expect(html).toContain('boot-splash-status');
    expect(html).toContain('class="boot-splash-mark"');
    // Decorative — must never intercept pointer events.
    expect(html).toContain('pointer-events: none');
  });

  it('paints the brand gradient on both canvas layers so the iOS bottom safe-area strip is covered', () => {
    // A fixed `inset:0` .boot-splash does not reach the iOS standalone bottom
    // safe-area strip, so the boot script must paint the brand gradient (base
    // colour + fixed attachment) on <html> behind it. The body must carry the
    // same paint: its light-theme inline background otherwise owns the uncovered
    // strip and shows white despite the root paint.
    expect(html).toMatch(
      /d\.style\.background\s*=\s*['"]#145eb9 radial-gradient\([^'"]*\) no-repeat fixed['"]/,
    );
    expect(html).toMatch(
      /<body style="background:#145eb9 radial-gradient\([^";]*\) no-repeat fixed">/,
    );
  });

  it('bases the canvas on the gradient colour AT the seam, not on its end stop', () => {
    // The base colour is the only thing iOS paints into the bottom strip, right
    // up against the gradient. Pinning the exact value is the regression guard:
    // #0a4ea8 (the gradient's 100% stop) shipped here and read as a darker band,
    // because along the bottom edge the gradient has only travelled 0.62 (x=30%)
    // to 0.84 (x=100%) of the way to it. Both figures are aspect-independent,
    // since each radius is a percentage of its own axis, so one constant works
    // on every device. #145eb9 is the gradient at progress 0.70.
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

    // Every colour the gradient takes along the bottom edge is within 10/255 of
    // the base, so the seam cannot read as a band.
    for (const progress of [0.624, 0.669, 0.7, 0.75, 0.8385]) {
      at(progress).forEach((channel, i) => {
        expect(Math.abs(channel - base[i])).toBeLessThanOrEqual(10);
      });
    }
    // The gradient's own stops are untouched by this: only the base moved.
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

  it('recovers the moment the entry module reports it cannot load, not 15s later', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog();
      watchdog.fireEntryModuleError();
      // No timer advanced: a bundle that answered "I cannot load" has already
      // given the answer the 15s wait exists to obtain.
      expect(watchdog.location.replace).toHaveBeenCalledWith(
        'https://example.com/?thread=abc&_boot_retry=1',
      );
      // And the recovery is still the SAME single attempt: the timer must not
      // fire a second one behind it.
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
      watchdog.fireError(undefined); // a runtime exception carries no element
      watchdog.fireError({ tagName: 'IMG' }); // a broken image is not boot
      watchdog.fireError({ tagName: 'SCRIPT', type: 'text/javascript' }); // classic script
      expect(watchdog.location.replace).not.toHaveBeenCalled();
      // The timer is still the owner of the hang case.
      vi.advanceTimersByTime(15_000);
      expect(watchdog.location.replace).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it('spends its one retry on the URL mark alone, so a storage-less browser cannot loop', () => {
    vi.useFakeTimers();
    try {
      // sessionStorage empty (a browser that refuses it, or a fresh session),
      // but this document IS the retry: the reload put the mark on the URL.
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
      // A lazily loaded module failing hours into a session is the app's
      // problem; reloading the page under the user is never the answer.
      watchdog.fireEntryModuleError();
      vi.advanceTimersByTime(15_000);
      expect(watchdog.location.replace).not.toHaveBeenCalled();
      expect(watchdog.status.textContent).toBe('Opening your workspace…');
    } finally {
      vi.useRealTimers();
    }
  });

  // The dead end this exists for: a per-workspace PWA installed on a DIRECT
  // engine port. Nothing on that origin lazy-starts a stopped workspace, and the
  // gateway is a different origin, so without a link out the user is stuck on
  // the splash with no way to act.
  const GATEWAY = { gatewayPort: '5251' };

  it('sends a stopped direct-port workspace to itself on the gateway, which starts it', () => {
    vi.useFakeTimers();
    try {
      const watchdog = runInlineWatchdog(true, { ...GATEWAY, workspaceId: 'myws' });
      watchdog.fireEntryModuleError();
      // The deep link is what makes the gateway lazy-start the workspace; the
      // picker would be a tap further.
      expect(watchdog.escape.href).toBe('https://example.com:5251/myws/');
      expect(watchdog.escape.hidden).toBe(false);
      expect(watchdog.escape.textContent).toBe('Start this workspace');
      // The status names the real problem instead of inviting a pointless retry.
      expect(watchdog.status.textContent).toBe("Can't reach this workspace");
      // The splash is decorative (aria-hidden) until it carries the only action
      // on screen. Leaving it hidden would silence the status and leave a
      // focusable link inside an aria-hidden subtree.
      expect(watchdog.splash.setAttribute).toHaveBeenCalledWith('aria-hidden', 'false');
    } finally {
      vi.useRealTimers();
    }
  });

  it('falls back to the workspace list when the shell predates the id stamp', () => {
    vi.useFakeTimers();
    try {
      // A cached shell from before the engine stamped its slug: the gateway is
      // still addressable, the workspace is not.
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
      // No splash-wide reload handler: one tap must not both reload and
      // navigate, and a reload cannot start a stopped engine anyway.
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
      // A legacy no-gateway engine (and the e2e direct engine): no port meta, so
      // no escape exists and the plain retry stands.
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
      // Behind the gateway (base `/<slug>/`) the origin already IS the gateway,
      // which lazy-starts on its own; the same goes for the picker. Sending
      // either one "to the gateway" would be a no-op at best.
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
      // The app hits this dead end too when its bundle DID load but the engine
      // is unreachable (utils/bootSplash.ts revealBootEscape). It must reuse this
      // implementation, and the handover must NOT delete the hook.
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
    // The status div ships visible (shown class) with default text, so the
    // picker→workspace hop never shows an empty/disappearing status line.
    expect(html).toContain('boot-splash-status-shown');
    expect(html).toMatch(/boot-splash-status[^>]*>[^<]*\S[^<]*<\/div>/);
  });

  it('reserves a constant status size so the mark never shifts', () => {
    // A fixed single-line height (not min-height) keeps the box identical whether
    // the text is present, empty, or invisible.
    expect(html).toMatch(/\.boot-splash-status\s*\{[^}]*height:\s*1\.4em/);
    expect(html).toMatch(/\.boot-splash-status\s*\{[^}]*white-space:\s*nowrap/);
  });

  it('pins the splash geometry in px so it cannot ride the UI scale', () => {
    // This document's <html> font-size is var(--user-ui-scale) (base.css, and
    // 112.5% by default on mobile.css), so ANY rem length here resolves to a
    // different pixel size than the same value on the gateway splash, which is
    // an isolated document at the browser default. That is what made the mark
    // grow and the status slide down at the cold-boot to workspace seam.
    expect(html).toMatch(/\.boot-splash-mark\s*\{[^}]*width:\s*min\(46vmin,\s*240px\)/);
    expect(html).toMatch(/\.boot-splash-mark\s*\{[^}]*height:\s*min\(46vmin,\s*240px\)/);
    expect(html).toMatch(/\.boot-splash\s*\{[^}]*gap:\s*24px/);
    // Type is declared once on the container, so every line on either surface
    // (the status here, the gateway's escape link) inherits the same size and
    // stack. The stack must never be var(--font-ui): a not-yet-downloaded web
    // font would swap and reflow the splash.
    expect(html).toMatch(/\.boot-splash\s*\{[^}]*font-size:\s*15px/);
    expect(html).toMatch(/\.boot-splash\s*\{[^}]*font-family:\s*ui-monospace/);
    expect(html).not.toMatch(/\.boot-splash[^{]*\{[^}]*font-family:\s*var\(--font-ui\)/);
    // No rem in any DECLARATION of the splash stylesheet (comments stripped:
    // they name the rem values these px replaced, and must keep doing so).
    const splashCss = (html.match(/<style>([\s\S]*?)<\/style>/)?.[1] ?? '').replace(
      /\/\*[\s\S]*?\*\//g,
      '',
    );
    expect(splashCss).toContain('.boot-splash');
    expect(splashCss).not.toMatch(/\d\s*rem/);
  });

  // The gateway splash carries this stylesheet and NOTHING else: it renders
  // when no engine is reachable, so it cannot link the bundle css the app shell
  // gets. Any type property the shell sets from `body` and this block does not
  // therefore differs BETWEEN the two surfaces, on a seam the user crosses
  // mid-boot. Smoothing is the one that shipped that way: the gateway status
  // rendered at the macOS default (subpixel-antialiased, visibly heavier) and
  // the app document's grayscale, so the line changed weight on the same frame
  // its text changed. Read the shell's own rule rather than restating its
  // values, so editing base.css cannot silently reopen it.
  it('smooths the splash type exactly as the app shell does', () => {
    const baseCss = readFileSync(resolve(__dirname, '../styles/global/base.css'), 'utf-8');
    const shell = /^body \{([^}]*)\}/m.exec(baseCss)?.[1];
    expect(shell, 'the body rule in styles/global/base.css').toBeTruthy();
    const splash = /\.boot-splash \{([\s\S]*?)\n {6}\}/.exec(html)?.[1];
    expect(splash, 'the .boot-splash rule in index.html').toBeTruthy();

    const declared = (css: string, prop: string) =>
      new RegExp(`(?:^|;|\\*/)\\s*${prop}:\\s*([^;]+);`, 'm').exec(css)?.[1].trim();

    // Every smoothing/rendering property the shell declares must be declared
    // here too, with the same value. Discovered from the shell rather than
    // listed, so a fourth one added there fails this test instead of quietly
    // applying to the app and not to the splash.
    const smoothing = [
      ...shell!.matchAll(/(-webkit-font-smoothing|-moz-osx-font-smoothing|text-rendering):/g),
    ].map((m) => m[1]);
    expect(smoothing.length, 'base.css body still declares font smoothing').toBeGreaterThan(0);
    for (const prop of smoothing) {
      expect(declared(splash!, prop), prop).toBe(declared(shell!, prop));
    }
  });

  // Three inline scripts in this document derive the per-workspace key prefix by
  // hand (the FOUC theme/font/scale reader, the quiet-cover flag, the boot
  // watchdog's retry marker) because each runs before the app's storage override
  // exists. They read keys the APP writes, so a copy that normalizes differently
  // reads a key nobody wrote and fails silently. An absolute `<base href>` is the
  // case that splits them: `basePath.ts` takes its pathname, a naive slash-strip
  // does not. Count the guard instead of trusting three prose comments.
  it('normalizes an absolute base href in every hand-rolled key derivation', () => {
    const guards = html.match(/if\s*\(.*?\/\^https\?:\\\/\\\/\/i\.test\(/g) ?? [];
    expect(guards.length, 'one absolute-base guard per wsKey derivation').toBe(3);
    expect(html.match(/function wsKey\(/g)?.length).toBe(3);
    // And each strips THEN compares, so a slash-less `~` is null everywhere
    // rather than a `ws:~:` namespace in two copies and null in the third.
    expect(html.match(/\(seg === '' \|\| seg === '~'\) \? null : seg/g)?.length).toBe(3);
  });

  it('keeps the markers the gateway splash lifts this stylesheet and mark out by', () => {
    // The gateway serves its own boot splash on the same url this document
    // loads at, and it is THIS splash: proxy.rs `include_str!`s this file and
    // slices between these markers, so neither surface can drift from the
    // other. Losing a marker unstyles (or empties) the gateway splash, so both
    // sides pin them; the Rust half is `the_app_splash_stylesheet_and_mark_are_extractable`.
    for (const marker of [
      '/* lucidos-boot-splash-css-start */',
      '/* lucidos-boot-splash-css-end */',
      '<!-- lucidos-boot-splash-mark-start -->',
      '<!-- lucidos-boot-splash-mark-end -->',
    ]) {
      expect(html.split(marker).length - 1, marker).toBe(1);
    }
    // The slices must be in the right order and carry the real content.
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
    // The eager redirect must run from an inline <head> script BEFORE the module
    // bundle, so the picker never paints (no picker→workspace reload seam).
    const redirectIdx = html.indexOf('location.replace');
    const moduleIdx = html.indexOf('<script type="module"');
    expect(redirectIdx).toBeGreaterThan(-1);
    expect(redirectIdx).toBeLessThan(moduleIdx);
    // Reads the raw last-workspace key, and stands down on the `?pick` escape.
    expect(html).toContain("localStorage.getItem('lucidos-last-workspace')");
    expect(html).toContain("has('pick')");
    // Only on the picker context (stamped base href), never inside a workspace.
    expect(html).toContain("getAttribute('href') !== '/~/'");
  });

  describe('gateway handover (boot-splash-formed)', () => {
    // The gateway 503 splash and this document share one url, so the mark the
    // gateway already built is on screen when this document takes over. The
    // handover flag is what stops it from being torn down and rebuilt.
    function runHandover(flag: string | null) {
      const source = html
        .split('<script>')
        .find((block: string) => block.includes('lucidos-splash-mark-formed'))
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
      expect(html).toMatch(
        /@media \(prefers-reduced-motion: reduce\)\s*\{[\s\S]*?\.boot-splash-formed\s+\.boot-splash-mark\s*\{\s*animation:\s*none/,
      );
    });
  });

  describe('notification tap (boot-splash-quiet)', () => {
    // A push tap on an installed iOS PWA arrives as a full cross-document load of
    // `?notification=…` (WebKit offers no reload-free channel), so this splash is
    // on screen for every tap. It is a navigation inside a session the user was
    // already in, not a launch, so the document drops the launch ceremony.
    const source = html
      .split('<script>')
      .find((block: string) => block.includes("classList.add('boot-splash-quiet')"))
      ?.split('</script>')[0];

    /** Run the inline deep-link script against a URL, with a fake document whose
     *  canvas layers start on the brand gradient (as the real ones do). */
    function runQuietBoot(
      url: string,
      opts: {
        bgVar?: string;
        theme?: string;
        formed?: boolean;
        refreshed?: boolean;
        /** `<base href>`: a `/<slug>/` workspace behind the gateway namespaces the
         *  flag key; null (direct engine) leaves it raw. */
        base?: string | null;
      } = {},
    ) {
      if (!source) throw new Error('inline deep-link quiet script not found');
      const parsed = new URL(url);
      const GRADIENT = '#145eb9 radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%) no-repeat fixed';
      // `formed` is the gateway handover script (which runs earlier in the body)
      // having already tagged this document.
      const classes = new Set<string>(opts.formed ? ['boot-splash-formed'] : []);
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
      const documentElement = {
        style: {
          background: GRADIENT,
          getPropertyValue: (key: string) =>
            key === '--bg-primary' ? (opts.bgVar ?? '#07172e') : '',
        },
        getAttribute: (key: string) => (key === 'data-theme' ? (opts.theme ?? 'dark') : null),
      };
      const body = { style: { background: GRADIENT } };
      // `refreshed` is the one-shot flag `refreshClient` stamps before reloading,
      // stored under the SAME per-workspace key the app's storage override writes.
      // The expected key is built from the REAL `normalizeBasePath`, not a copy of
      // it, so this pins the inline script against the app's actual contract
      // (`WORKSPACE_ID` is `normalizeBasePath(baseHref).slice(1)`, and
      // workspaceStorage prefixes `ws:<id>:`).
      const basePath = opts.base == null ? '' : normalizeBasePath(opts.base);
      const slug = basePath === '' || basePath === '/~' ? null : basePath.slice(1);
      const flagKey = slug ? `ws:${slug}:lucidos-splash-quiet` : 'lucidos-splash-quiet';
      const stored = new Map<string, string>();
      if (opts.refreshed) stored.set(flagKey, '1');
      const sessionStorage = {
        getItem: (key: string) => stored.get(key) ?? null,
        removeItem: (key: string) => stored.delete(key),
      };
      const base = opts.base == null ? null : { getAttribute: () => opts.base };
      new Function('document', 'location', 'sessionStorage', source)(
        {
          querySelector: (sel: string) => (sel === 'base' ? base : splash),
          documentElement,
          body,
        },
        { search: parsed.search, hash: parsed.hash },
        sessionStorage,
      );
      return {
        quiet: classes.has('boot-splash-quiet'),
        status,
        statusShown: () => statusClasses.has('boot-splash-status-shown'),
        rootBackground: () => documentElement.style.background,
        bodyBackground: () => body.style.background,
        flagLeft: () => stored.has(flagKey),
        /** Anything left in storage, so a read of the WRONG key is visible as a
         *  flag that was never consumed rather than as a silent non-quiet. */
        leftoverKeys: () => [...stored.keys()],
        gradient: GRADIENT,
      };
    }

    it('quiets the splash and flattens both canvas layers on a tap', () => {
      const boot = runQuietBoot('https://host/myws/?notification=n1&thread=t1&tap=%7B%7D');
      expect(boot.quiet).toBe(true);
      // "Opening your workspace…" is a launch message; this is a navigation.
      expect(boot.status.textContent).toBe('');
      expect(boot.statusShown()).toBe(false);
      // A fixed inset:0 cover never reaches the iOS standalone bottom safe-area
      // strip, so leaving the gradient on either canvas layer would show a blue
      // band under a flat cover.
      expect(boot.rootBackground()).toBe('#07172e');
      expect(boot.bodyBackground()).toBe('#07172e');
    });

    it('reads the deep link out of the hash as well as the query', () => {
      expect(runQuietBoot('https://host/myws/#notification=n1&thread=t1').quiet).toBe(true);
    });

    // The other continuation: a refresh the user asked for. `refreshClient` stamps
    // the flag before reloading, so the next document knows it is coming back to
    // the same session rather than opening one, with no deep link on the URL.
    it('quiets a user-requested refresh, which carries no deep link at all', () => {
      const boot = runQuietBoot('https://host/myws/', { refreshed: true });
      expect(boot.quiet).toBe(true);
      expect(boot.status.textContent).toBe('');
      expect(boot.rootBackground()).toBe('#07172e');
      expect(boot.bodyBackground()).toBe('#07172e');
    });

    // One-shot, exactly like the gateway handover flag: a refresh whose reload
    // never happened must not quiet every load for the rest of the session.
    it('consumes the refresh flag as it reads it', () => {
      expect(runQuietBoot('https://host/myws/', { refreshed: true }).flagLeft()).toBe(false);
      // And a second load with no flag is a normal launch again.
      expect(runQuietBoot('https://host/myws/').quiet).toBe(false);
    });

    // `refreshClient` writes through the app's storage override, whose prototype
    // patch covers sessionStorage too, so behind the gateway the flag lands at
    // `ws:<slug>:…`. This script runs before that override exists and must build
    // the same key by hand. Reading the raw key instead would leave the written
    // one untouched and the cover would never appear in a real workspace, which
    // is invisible in a direct-engine test where both keys are the same string.
    it('reads the flag under the SAME per-workspace key the app writes', () => {
      const boot = runQuietBoot('https://host/myws/', { refreshed: true, base: '/myws/' });
      expect(boot.quiet).toBe(true);
      expect(boot.leftoverKeys()).toEqual([]);
      // Picker and legacy root have no slug, so the override no-ops and so does this.
      for (const base of ['/~/', '/', null]) {
        expect(runQuietBoot('https://host/', { refreshed: true, base }).quiet, String(base))
          .toBe(true);
      }
      // An ABSOLUTE base href is a supported value (normalizeBasePath tolerates
      // it), and the app namespaces off its PATHNAME. Stripping slashes off the
      // whole URL would look for `ws:https:/host/myws:…` and find nothing.
      const abs = runQuietBoot('https://host/myws/', {
        refreshed: true,
        base: 'https://host/myws/',
      });
      expect(abs.quiet).toBe(true);
      expect(abs.leftoverKeys()).toEqual([]);
    });

    // The flag is consumed BEFORE any other gate, so a refresh that lands on a
    // URL the deep-link branch would bail out of cannot leave it behind.
    it('consumes the flag even on a URL the deep-link gate would return early on', () => {
      const boot = runQuietBoot(
        'https://host/myws/#thread=1e6a2f14-0000-4000-8000-000000000000',
        { refreshed: true },
      );
      expect(boot.flagLeft()).toBe(false);
      // A refresh is a refresh: the landing hash does not un-quiet it.
      expect(boot.quiet).toBe(true);
    });

    // The detection is a hand-written mirror of the page-side router's gate, in a
    // classic inline script that cannot import it. Pin the two together directly:
    // a document that goes quiet but then behaves like a cold launch (or the
    // reverse) is the drift this catches. The oracle is the router's REAL branch
    // order, `THREAD_HASH_RE` first and `hasDeepLinkParams` second (see
    // handleHashLocation), not the deep-link gate alone.
    const routerDispatchesDeepLink = (url: string) =>
      !THREAD_HASH_RE.test(new URL(url).hash) &&
      hasDeepLinkParams(parseDeepLinkFromUrl(new URL(url)));

    it('goes quiet exactly when the page-side router would dispatch a deep link', () => {
      for (const url of [
        'https://host/myws/?notification=n1',
        'https://host/myws/?notification=n1&thread=t1&event=e1&tap=%7B%22kind%22%3A%22modal%22%7D',
        'https://host/myws/#notification=n1&thread=t1',
        // A bare thread/event pair resolves to noop page-side, so it is a launch.
        'https://host/myws/?thread=t1&event=e1',
        // Present but EMPTY: the router dispatches on the value, not the key, so a
        // key-presence check here would quiet a document that then routes nothing.
        'https://host/myws/?notification=',
        'https://host/myws/#notification=',
        // The router's `get` is `hash ?? query`, so an empty hash value beats a good
        // query one. Mirroring key-presence alone gets this pair backwards.
        'https://host/myws/?notification=n1#notification=',
        'https://host/myws/?notification=#notification=n1',
        // The cross-workspace landing channel keeps the launch splash: that hop
        // can lazy-start a stopped engine, where "Opening your workspace…" is true.
        'https://host/myws/#thread=1e6a2f14-0000-4000-8000-000000000000',
        // And it wins over a notification param when both are on the URL, because
        // the router checks it first. Nothing emits this shape today; the point is
        // that the mirror follows the router's branch ORDER, not a subset of its
        // conditions.
        'https://host/myws/?notification=n1#thread=1e6a2f14-0000-4000-8000-000000000000',
        'https://host/myws/',
        'https://host/myws/?_boot_retry=1',
      ]) {
        expect(runQuietBoot(url).quiet, url).toBe(routerDispatchesDeepLink(url));
      }
    });

    // The one deep-link document that is NOT a continuation of a live session:
    // a tap on a stopped workspace, which the gateway lazy-starts while serving
    // its own boot splash on this exact url (query intact, meta-refreshed). By
    // the time this document loads the user has watched a fully built mark for
    // seconds, and the handover script kept it standing. Quieting it there would
    // `display: none` that mark and snap the gradient flat in one frame, the
    // exact seam jump the handover was built to prevent.
    it('stands down when the gateway handed over a mark that is already standing', () => {
      // Both triggers lose to the handover: a refresh during an engine restart
      // crosses the same gateway splash as a tap on a stopped workspace does.
      for (const opts of [
        { formed: true },
        { formed: true, refreshed: true },
      ]) {
        const boot = runQuietBoot('https://host/myws/?notification=n1&thread=t1', opts);
        expect(boot.quiet, JSON.stringify(opts)).toBe(false);
        // And it must bail BEFORE the canvas repaint, or the standing mark is left
        // on a flattened background.
        expect(boot.rootBackground()).toBe(boot.gradient);
        expect(boot.bodyBackground()).toBe(boot.gradient);
        expect(boot.statusShown()).toBe(true);
        // The flag is still spent, so it cannot quiet a later load.
        expect(boot.flagLeft()).toBe(false);
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
      // Normal case: the FOUC script above already resolved --bg-primary.
      expect(runQuietBoot('https://host/?notification=n1', { bgVar: '#ffffff' }).rootBackground())
        .toBe('#ffffff');
      // A browser that refuses localStorage throws that script out entirely, so
      // the variable is unset. The canvas must still not keep the gradient under
      // a flat cover.
      expect(
        runQuietBoot('https://host/?notification=n1', { bgVar: '', theme: 'light' }).rootBackground(),
      ).toBe('#ffffff');
      expect(
        runQuietBoot('https://host/?notification=n1', { bgVar: '', theme: 'dark' }).rootBackground(),
      ).toBe('#07172e');
    });

    // Consuming the deep link belongs to handleHashLocation alone. A stray
    // location/history write here would strip the params before the router ever
    // sees them, which is exactly how a tap "goes nowhere".
    it('reads the URL and never writes it', () => {
      expect(source).toBeTruthy();
      expect(source).not.toMatch(/location\.(replace|assign|reload)|location\.href\s*=|history\./);
    });

    it('drops the mark and the gradient, and leaves faster than a launch splash', () => {
      expect(html).toMatch(/\.boot-splash-quiet\s*\{[^}]*background:\s*var\(--bg-primary/);
      expect(html).toMatch(/\.boot-splash-quiet\s+\.boot-splash-mark\s*\{[^}]*display:\s*none/);
      expect(html).toMatch(
        /\.boot-splash-quiet\.boot-splash-leaving\s*\{[^}]*animation-duration:\s*0\.2s/,
      );
    });

    // The splash's foregrounds are hardcoded white because the brand gradient is
    // always behind them. The quiet cover is the APP background instead, which is
    // #ffffff in light theme, so without a theme-aware colour the delayed status
    // and the watchdog's escape link ("Start this workspace", the only way out of
    // a stopped direct-port workspace) would be white on white at exactly the
    // moment boot has given up.
    it('keeps the status and the escape legible on a light-theme quiet cover', () => {
      expect(html).toMatch(
        /\[data-theme="light"\]\s+\.boot-splash-quiet\s+\.boot-splash-status\s*\{[^}]*color:/,
      );
      expect(html).toMatch(
        /\[data-theme="light"\]\s+\.boot-splash-quiet\s+\.boot-splash-escape\s*\{[^}]*color:/,
      );
    });

    // The quiet fade needs two-class specificity to beat the `animation`
    // shorthand on `.boot-splash-leaving`, and a media query adds none. So
    // without restating the quiet selector inside the reduced-motion block, the
    // 0.2s rule silently outranks the 0.15s an accessibility preference asked
    // for. Pin the restatement, not just the plain rule.
    it('does not let the quiet fade outrank prefers-reduced-motion', () => {
      const reduced = /@media \(prefers-reduced-motion: reduce\)\s*\{([\s\S]*?)\n {6}\}/.exec(html)?.[1];
      expect(reduced).toBeTruthy();
      expect(reduced).toMatch(
        /\.boot-splash-leaving,\s*\.boot-splash-quiet\.boot-splash-leaving\s*\{[^}]*animation-duration:\s*0\.15s/,
      );
    });
  });

  it('plays the mark reveal in the final doc but hides the mark in the picker', () => {
    // The whole-mark reveal animation exists and is applied to the mark — the
    // per-tile reveal (bs-tile / boot-tile-in) was dropped because iOS WebKit
    // doesn't GPU-composite SVG sub-element transforms, so it janked at boot
    // (see the index.html boot-splash comment). The mark builds up once…
    expect(html).toContain('@keyframes boot-mark-reveal');
    expect(html).toMatch(/\.boot-splash-mark\s*\{[^}]*animation:\s*boot-mark-reveal/);
    // …and the picker (boot-splash-reload) hides the mark so the reveal happens
    // only in the workspace document, set by the inline base-href check in the
    // body script that adds the class. (The FOUC script detects the picker too,
    // for its theme fallback, but off the NORMALIZED base path rather than the
    // raw attribute; see the absolute-base-href test above.)
    expect(html).toMatch(/\.boot-splash-reload\s+\.boot-splash-mark\s*\{[^}]*visibility:\s*hidden/);
    expect(html).toContain("getAttribute('href') !== '/~/'");
  });
});
