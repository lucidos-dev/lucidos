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

  it('dismiss reverts the root background only AFTER the splash node is removed (not mid-fade)', async () => {
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

  it('paints the brand gradient on the root so the iOS bottom safe-area strip is covered', () => {
    // A fixed `inset:0` .boot-splash does not reach the iOS standalone bottom
    // safe-area strip, so the boot script must paint the brand gradient (solid
    // #0a4ea8 fallback + fixed attachment) on <html> behind it — otherwise the
    // dark var(--bg-primary) root shows as a black band under the splash. Mirrors
    // the gateway splash (crates/lucidos-gateway/src/proxy.rs).
    expect(html).toMatch(
      /d\.style\.background\s*=\s*['"]#0a4ea8 radial-gradient\([^'"]*\) no-repeat fixed['"]/,
    );
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
