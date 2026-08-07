import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { lucidos } from '@lucidos/sdk';

// Theme is stored device-scoped (see crates/lucidos-app/src/store/actions/preferences.ts:141).
// `lucidos.ui.applyPreferences()` runs in app iframes and must read the SAME device id
// the parent uses (`lucidos-device-id` in localStorage), otherwise `/api/v1/preferences`
// returns only globally-scoped prefs (no `theme` key) and the iframe defaults to dark.
describe('lucidos.ui.applyPreferences — device-scoped fetch', () => {
  let fetchSpy: ReturnType<typeof vi.fn>;
  let originalFetch: typeof globalThis.fetch;

  function fetchedUrl(): URL {
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    return new URL(String(fetchSpy.mock.calls[0][0]), 'http://test');
  }

  beforeEach(() => {
    localStorage.clear();
    originalFetch = globalThis.fetch;
    fetchSpy = vi.fn(async () => new Response(
      JSON.stringify({ preferences: { theme: 'light' } }),
      { status: 200, headers: { 'Content-Type': 'application/json' } },
    ));
    globalThis.fetch = fetchSpy as unknown as typeof globalThis.fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('passes the lucidos-device-id from localStorage to GET /api/v1/preferences', async () => {
    localStorage.setItem('lucidos-device-id', 'device-abc');

    await lucidos.ui.applyPreferences();

    const url = fetchedUrl();
    expect(url.pathname).toBe('/api/v1/preferences');
    expect(url.searchParams.get('device_id')).toBe('device-abc');
  });

  it('falls back to no device_id when localStorage has none', async () => {
    await lucidos.ui.applyPreferences();

    const url = fetchedUrl();
    expect(url.pathname).toBe('/api/v1/preferences');
    expect(url.searchParams.get('device_id')).toBeNull();
  });

  it('keeps inline --bg-primary in sync with the resolved theme', async () => {
    // The sdk-prefs.js inline FOUC seeds --bg-primary on first paint from
    // localStorage. applyPreferences runs later (after the SDK loads) and
    // must update the inline value to match the SSE-resolved theme so the
    // body's `var(--bg-primary, ...)` paints the right color even though
    // inline styles win over the stylesheet cascade.
    const inlineProps: Record<string, string> = { '--bg-primary': '#07172e' };
    const realStyle = (document as any).documentElement.style;
    (document as any).documentElement.style = {
      setProperty: (k: string, v: string) => { inlineProps[k] = v; },
      getPropertyValue: (k: string) => inlineProps[k] ?? '',
      removeProperty: (k: string) => { delete inlineProps[k]; },
      background: '',
    };

    try {
      // beforeEach mocks fetch to return theme=light.
      await lucidos.ui.applyPreferences();
    } finally {
      (document as any).documentElement.style = realStyle;
    }

    expect(inlineProps['--bg-primary']).toBe('#ffffff');
  });

  it('sets inline html.style.background so the iframe paints with a bg before sdk-iframe.css loads', async () => {
    // See packages/lucidos-sdk/src/ui.ts for why — iOS PWA cold-restart flash.
    const inlineProps: Record<string, string> = {};
    const realStyle = (document as any).documentElement.style;
    const styleMock: any = {
      setProperty: (k: string, v: string) => { inlineProps[k] = v; },
      getPropertyValue: (k: string) => inlineProps[k] ?? '',
      removeProperty: (k: string) => { delete inlineProps[k]; },
      background: '',
    };
    (document as any).documentElement.style = styleMock;

    try {
      // beforeEach mocks fetch to return theme=light.
      await lucidos.ui.applyPreferences();
    } finally {
      (document as any).documentElement.style = realStyle;
    }

    expect(styleMock.background).toBe('#ffffff');
  });

  it('publishes ligatures to the iframe as OFF for text and ON for code', async () => {
    // The OFF value is the explicit zeros, never `normal`: liga and calt are
    // default-ON in CSS, so `normal` means "the font's defaults" and renders
    // byte-identically to `1`, which is how an earlier attempt at this fix
    // shipped as a no-op. The values render nothing until the two rules in the
    // engine's api/sdk_iframe.css consume them. Mirrors the host-side
    // assertion in actions/preferences.test.ts.
    globalThis.fetch = vi.fn(async () => new Response(
      JSON.stringify({ preferences: { theme: 'light', 'font-family': 'fira-code' } }),
      { status: 200, headers: { 'Content-Type': 'application/json' } },
    )) as unknown as typeof globalThis.fetch;

    const inlineProps: Record<string, string> = {};
    const realStyle = (document as any).documentElement.style;
    (document as any).documentElement.style = {
      setProperty: (k: string, v: string) => { inlineProps[k] = v; },
      getPropertyValue: (k: string) => inlineProps[k] ?? '',
      removeProperty: (k: string) => { delete inlineProps[k]; },
      background: '',
    };
    // fira-code is a Google font, so applyPreferences appends a <link>; the
    // test-setup document stub has no `head`.
    const realHead = (document as any).head;
    (document as any).head = { appendChild: () => {} };

    try {
      await lucidos.ui.applyPreferences();
    } finally {
      (document as any).documentElement.style = realStyle;
      (document as any).head = realHead;
    }

    expect(inlineProps['--font-features-text']).toBe('"liga" 0, "calt" 0');
    expect(inlineProps['--font-features-code']).toBe('"liga" 1, "calt" 1');
    expect(inlineProps['font-feature-settings']).toBeUndefined();
  });

  it('resolves both properties to normal for a font that ships no ligatures', async () => {
    // beforeEach's mock returns no font-family, so the SDK falls back to
    // monospace: both must clear rather than leave a stale value behind.
    const inlineProps: Record<string, string> = {};
    const realStyle = (document as any).documentElement.style;
    (document as any).documentElement.style = {
      setProperty: (k: string, v: string) => { inlineProps[k] = v; },
      getPropertyValue: (k: string) => inlineProps[k] ?? '',
      removeProperty: (k: string) => { delete inlineProps[k]; },
      background: '',
    };

    try {
      await lucidos.ui.applyPreferences();
    } finally {
      (document as any).documentElement.style = realStyle;
    }

    expect(inlineProps['--font-features-text']).toBe('normal');
    expect(inlineProps['--font-features-code']).toBe('normal');
  });
});

// The systemic theme bug: `applyPreferences()` ran AFTER sdk-prefs.js (which
// synchronously seeds data-theme/--font-ui/--user-ui-scale from localStorage),
// then overwrote that correct value with a hard default whenever the active
// device had no server-scoped pref. The iPhone-PWA case (only `ui-scale` stored
// server-side, no `theme`) flipped every app iframe to dark even though the
// host shell stayed light. The invariant these tests pin: a MISSING server
// value must never clobber the client value sdk-prefs.js already resolved.
//
// The node test env stubs <html> with a no-op setAttribute and an always-''
// style (src/test-setup.ts has no real DOM), so we record what applyPreferences
// writes — mirroring the existing --bg-primary test's style swap above.
describe('lucidos.ui.applyPreferences — client value wins when the server lacks the key', () => {
  let originalFetch: typeof globalThis.fetch;

  function mockPrefs(prefs: Record<string, string>) {
    globalThis.fetch = vi.fn(async () => new Response(
      JSON.stringify({ preferences: prefs }),
      { status: 200, headers: { 'Content-Type': 'application/json' } },
    )) as unknown as typeof globalThis.fetch;
  }

  function captureWrites(initialDataTheme?: string) {
    const attrs: Record<string, string> = {};
    if (initialDataTheme !== undefined) attrs['data-theme'] = initialDataTheme;
    const props: Record<string, string> = {};
    const el = document.documentElement as any;
    const real = { setAttribute: el.setAttribute, getAttribute: el.getAttribute, style: el.style };
    el.setAttribute = (k: string, v: string) => { attrs[k] = v; };
    el.getAttribute = (k: string) => (k in attrs ? attrs[k] : null);
    el.style = {
      setProperty: (k: string, v: string) => { props[k] = v; },
      getPropertyValue: (k: string) => props[k] ?? '',
      removeProperty: (k: string) => { delete props[k]; },
      background: '',
    };
    return {
      attrs,
      props,
      restore() {
        el.setAttribute = real.setAttribute;
        el.getAttribute = real.getAttribute;
        el.style = real.style;
      },
    };
  }

  async function applyWithCapture(initialDataTheme?: string) {
    const cap = captureWrites(initialDataTheme);
    try {
      await lucidos.ui.applyPreferences();
    } finally {
      cap.restore();
    }
    return cap;
  }

  beforeEach(() => {
    localStorage.clear();
    originalFetch = globalThis.fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('keeps the localStorage theme when the server has no device-scoped theme', async () => {
    // The reported iPhone-PWA case: only ui-scale stored server-side, no theme.
    localStorage.setItem('lucidos-theme', 'light');
    mockPrefs({ 'ui-scale': '125' });

    const cap = await applyWithCapture();

    expect(cap.attrs['data-theme']).toBe('light');
  });

  it('does not flip to dark when the server returns no theme at all', async () => {
    localStorage.setItem('lucidos-theme', 'light');
    mockPrefs({});

    const cap = await applyWithCapture();

    expect(cap.attrs['data-theme']).toBe('light');
  });

  it('falls back to the data-theme attribute sdk-prefs.js applied when localStorage is empty', async () => {
    mockPrefs({});

    // sdk-prefs.js already resolved + applied data-theme=light synchronously.
    const cap = await applyWithCapture('light');

    expect(cap.attrs['data-theme']).toBe('light');
  });

  it('still lets a present server theme win over a stale localStorage value', async () => {
    localStorage.setItem('lucidos-theme', 'dark');
    mockPrefs({ theme: 'light' });

    const cap = await applyWithCapture();

    expect(cap.attrs['data-theme']).toBe('light');
  });

  it('ignores an invalid localStorage theme and falls back to the default', async () => {
    localStorage.setItem('lucidos-theme', 'chartreuse');
    mockPrefs({});

    const cap = await applyWithCapture();

    expect(cap.attrs['data-theme']).toBe('dark');
  });

  it('keeps the localStorage font when the server has no font-family', async () => {
    localStorage.setItem('lucidos-font-family', 'system');
    mockPrefs({});

    const cap = await applyWithCapture();

    expect(cap.props['--font-ui']).toContain('system-ui');
  });

  it('keeps the localStorage ui-scale when the server has none', async () => {
    localStorage.setItem('lucidos-ui-scale', '125');
    mockPrefs({});

    const cap = await applyWithCapture();

    expect(cap.props['--user-ui-scale']).toBe('125%');
  });
});
