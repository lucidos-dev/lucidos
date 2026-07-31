import { describe, it, expect, vi, afterEach } from 'vitest';
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

function installFakeSplash(present: boolean) {
  const statusClasses = new Set<string>();
  const statusEl = {
    textContent: '',
    classList: { toggle: (c: string, on: boolean) => { on ? statusClasses.add(c) : statusClasses.delete(c); } },
  };
  const listeners: Record<string, Array<() => void>> = {};
  let removed = false;
  const classes = new Set<string>();
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

  it('dismiss reverts the boot background only AFTER the splash node is removed (not mid-fade)', async () => {
    fake = installFakeSplash(true);
    const doc = (globalThis as any).document;
    // Boot script (index.html) paints the brand gradient on <html> to cover the
    // iOS bottom safe-area strip; dismiss must revert it so no blue lingers
    // behind the app's own safe-area inset — but only once the splash is gone.
    doc.documentElement.style.background =
      '#0a4ea8 radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%) no-repeat fixed';
    const c = await freshController();
    c.dismissBootSplash();
    // Still painted through the `.boot-splash-leaving` fade — reverting now would
    // flash the dark safe-area strip while the splash is still visible.
    expect(doc.documentElement.style.background).not.toBe('');
    fake.fireAnimationEnd();
    // Reverted once the splash node is actually removed.
    expect(doc.documentElement.style.background).toBe('');
  });

  it('dismiss reverts the root background immediately when the splash node is already gone', async () => {
    fake = installFakeSplash(false);
    const doc = (globalThis as any).document;
    doc.documentElement.style.background =
      '#0a4ea8 radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%) no-repeat fixed';
    const c = await freshController();
    c.dismissBootSplash();
    expect(doc.documentElement.style.background).toBe('');
  });
});

describe('index.html inline boot splash', () => {
  const html = readFileSync(resolve(__dirname, '../../index.html'), 'utf-8');

  function runInlineWatchdog(initialRetry = false) {
    const source = html.match(
      /\/\* lucidos-boot-watchdog-start[\s\S]*?\*\/([\s\S]*?)\/\* lucidos-boot-watchdog-end \*\//,
    )?.[1];
    if (!source) throw new Error('inline boot watchdog not found');

    const values = new Map<string, string>();
    if (initialRetry) values.set('lucidos-boot-retry', '1');
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
    const splash = {
      querySelector: () => status,
      classList: { add: (name: string) => splashClasses.add(name) },
      setAttribute: vi.fn(),
      addEventListener: (_type: string, fn: () => void) => { click = fn; },
    };
    const location = {
      href: initialRetry
        ? 'https://example.com/?thread=abc&_boot_retry=1'
        : 'https://example.com/?thread=abc',
      replace: vi.fn(),
      reload: vi.fn(),
    };
    const history = { state: null, replaceState: vi.fn() };
    const fakeWindow: Record<string, unknown> = {
      setTimeout: window.setTimeout.bind(window),
      clearTimeout: window.clearTimeout.bind(window),
    };
    const fakeDocument = {
      querySelector: (selector: string) => selector === 'base' ? null : splash,
    };
    new Function('window', 'document', 'sessionStorage', 'location', 'history', 'URL', source)(
      fakeWindow, fakeDocument, storage, location, history, URL,
    );
    return {
      fakeWindow,
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
    // safe-area strip, so the boot script must paint the brand gradient (solid
    // #0a4ea8 fallback + fixed attachment) on <html> behind it. The body must
    // carry the same paint: its light-theme inline background otherwise owns the
    // uncovered strip and shows white despite the root paint.
    expect(html).toMatch(
      /d\.style\.background\s*=\s*['"]#0a4ea8 radial-gradient\([^'"]*\) no-repeat fixed['"]/,
    );
    expect(html).toMatch(
      /<body style="background:#0a4ea8 radial-gradient\([^";]*\) no-repeat fixed">/,
    );
  });

  it('owns boot from the inline document and bounds a missing-module hang', () => {
    const watchdogIdx = html.indexOf('lucidos-boot-watchdog-start');
    const moduleIdx = html.indexOf('<script type="module"');
    expect(watchdogIdx).toBeGreaterThan(-1);
    expect(watchdogIdx).toBeLessThan(moduleIdx);
    expect(html).toContain('__lucidosBootLoaded');
    expect(html).toContain('lucidos-boot-retry');
    expect(html).toContain('Tap to retry');
    expect(html).toContain('}, 15000);');
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

  it('plays the mark reveal in the final doc but hides the mark in the picker', () => {
    // The whole-mark reveal animation exists and is applied to the mark — the
    // per-tile reveal (bs-tile / boot-tile-in) was dropped because iOS WebKit
    // doesn't GPU-composite SVG sub-element transforms, so it janked at boot
    // (see the index.html boot-splash comment). The mark builds up once…
    expect(html).toContain('@keyframes boot-mark-reveal');
    expect(html).toMatch(/\.boot-splash-mark\s*\{[^}]*animation:\s*boot-mark-reveal/);
    // …and the picker (boot-splash-reload) hides the mark so the reveal happens
    // only in the workspace document, set by the inline base-href check.
    expect(html).toMatch(/\.boot-splash-reload\s+\.boot-splash-mark\s*\{[^}]*visibility:\s*hidden/);
    expect(html).toContain("getAttribute('href') === '/~/'");
  });
});
