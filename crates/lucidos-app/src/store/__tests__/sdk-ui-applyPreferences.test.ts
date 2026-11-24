import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { lucidos } from '@lucidos/sdk';

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
      JSON.stringify({ preferences: { 'theme-mode': 'light' } }),
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
    const inlineProps: Record<string, string> = { '--bg-primary': '#07172e' };
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

    expect(inlineProps['--bg-primary']).toBe('#ffffff');
  });

  it('sets inline html.style.background so the iframe paints with a bg before sdk-iframe.css loads', async () => {
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
      await lucidos.ui.applyPreferences();
    } finally {
      (document as any).documentElement.style = realStyle;
    }

    expect(styleMock.background).toBe('#ffffff');
  });

  it('publishes ligatures to the iframe as OFF for text and ON for code', async () => {
    globalThis.fetch = vi.fn(async () => new Response(
      JSON.stringify({ preferences: { 'theme-mode': 'light', 'font-family': 'fira-code' } }),
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
    try {
      await lucidos.ui.applyPreferences();
    } finally {
      (document as any).documentElement.style = realStyle;
    }

    expect(inlineProps['--font-features-text']).toBe('"liga" 0, "calt" 0');
    expect(inlineProps['--font-features-code']).toBe('"liga" 1, "calt" 1');
    expect(inlineProps['font-feature-settings']).toBeUndefined();
  });

  it('resolves both properties to normal for a font that ships no ligatures', async () => {
    globalThis.fetch = vi.fn(async () => new Response(
      JSON.stringify({ preferences: { 'theme-mode': 'light', 'font-family': 'inter' } }),
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

    try {
      await lucidos.ui.applyPreferences();
    } finally {
      (document as any).documentElement.style = realStyle;
    }

    expect(inlineProps['--font-features-text']).toBe('normal');
    expect(inlineProps['--font-features-code']).toBe('normal');
  });

  it.each([['vt323', 'none'], ['inter', 'face']])(
    'says on <html> whether %s has a bold face',
    async (font, mark) => {
      globalThis.fetch = vi.fn(async () => new Response(
        JSON.stringify({ preferences: { theme: 'dark', 'font-family': font } }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      )) as unknown as typeof globalThis.fetch;

      const attrs: Record<string, string> = {};
      const root = (document as any).documentElement;
      const realSetAttribute = root.setAttribute;
      root.setAttribute = (k: string, v: string) => { attrs[k] = v; };
      try {
        await lucidos.ui.applyPreferences();
      } finally {
        root.setAttribute = realSetAttribute;
      }
      expect(attrs['data-font-bold']).toBe(mark);
    },
  );
});

describe('lucidos.ui.applyPreferences — client value wins when the server lacks the key', () => {
  let originalFetch: typeof globalThis.fetch;

  function mockPrefs(prefs: Record<string, string>) {
    globalThis.fetch = vi.fn(async () => new Response(
      JSON.stringify({ preferences: prefs }),
      { status: 200, headers: { 'Content-Type': 'application/json' } },
    )) as unknown as typeof globalThis.fetch;
  }

  function captureWrites(initialDataThemeMode?: string) {
    const attrs: Record<string, string> = {};
    if (initialDataThemeMode !== undefined) attrs['data-theme-mode'] = initialDataThemeMode;
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

  async function applyWithCapture(initialDataThemeMode?: string) {
    const cap = captureWrites(initialDataThemeMode);
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
    localStorage.setItem('lucidos-theme-mode', 'light');
    mockPrefs({ 'ui-scale': '125' });

    const cap = await applyWithCapture();

    expect(cap.attrs['data-theme-mode']).toBe('light');
  });

  it('does not flip to dark when the server returns no theme at all', async () => {
    localStorage.setItem('lucidos-theme-mode', 'light');
    mockPrefs({});

    const cap = await applyWithCapture();

    expect(cap.attrs['data-theme-mode']).toBe('light');
  });

  it('falls back to the data-theme-mode attribute sdk-prefs.js applied when localStorage is empty', async () => {
    mockPrefs({});

    const cap = await applyWithCapture('light');

    expect(cap.attrs['data-theme-mode']).toBe('light');
  });

  it('still lets a present server theme win over a stale localStorage value', async () => {
    localStorage.setItem('lucidos-theme-mode', 'dark');
    mockPrefs({ 'theme-mode': 'light' });

    const cap = await applyWithCapture();

    expect(cap.attrs['data-theme-mode']).toBe('light');
  });

  it('ignores an invalid localStorage theme and falls back to the OS', async () => {
    localStorage.setItem('lucidos-theme-mode', 'chartreuse');
    mockPrefs({});

    const cap = await applyWithCapture();

    expect(cap.attrs['data-theme-mode']).toBe('dark');
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

describe('lucidos.ui.applyPreferences: the font a theme suggests', () => {
  let originalFetch: typeof globalThis.fetch;
  let inlineProps: Record<string, string>;
  let realStyle: unknown;

  function mockEngine(prefs: Record<string, string>, fonts: Record<string, string>) {
    globalThis.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const body = String(input).includes('/theme?id=')
        ? { id: prefs.theme, resolved: { dark: {}, light: {}, fonts } }
        : { preferences: prefs };
      return new Response(JSON.stringify(body), {
        status: 200, headers: { 'Content-Type': 'application/json' },
      });
    }) as unknown as typeof globalThis.fetch;
  }

  beforeEach(() => {
    localStorage.clear();
    originalFetch = globalThis.fetch;
    inlineProps = {};
    realStyle = (document as any).documentElement.style;
    (document as any).documentElement.style = {
      setProperty: (k: string, v: string) => { inlineProps[k] = v; },
      getPropertyValue: (k: string) => inlineProps[k] ?? '',
      removeProperty: (k: string) => { delete inlineProps[k]; },
      background: '',
    };
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    (document as any).documentElement.style = realStyle;
  });

  it('a device that never picked a font paints the theme font', async () => {
    mockEngine({ 'theme-mode': 'light', theme: 'harbour-one' }, { ui: 'geist' });
    await lucidos.ui.applyPreferences();
    expect(inlineProps['--font-ui']).toMatch(/^'Geist',/);
  });

  it('an explicit pick wins over the theme font', async () => {
    mockEngine({ 'theme-mode': 'light', theme: 'harbour-two', 'font-family': 'system' }, { ui: 'geist' });
    await lucidos.ui.applyPreferences();
    expect(inlineProps['--font-ui']).toMatch(/^system-ui,/);
  });

  it('a theme with no font falls back to Fira Code', async () => {
    mockEngine({ 'theme-mode': 'light', theme: 'harbour-three', 'font-family': 'theme' }, {});
    await lucidos.ui.applyPreferences();
    expect(inlineProps['--font-ui']).toMatch(/^'Fira Code',/);
  });
});
